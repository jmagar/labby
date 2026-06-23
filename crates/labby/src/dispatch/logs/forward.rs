//! Syslog forwarding logic: reads journald or /var/log/syslog and batches
//! events to the master log ingest endpoint.
//!
//! This module is the shared dispatch-layer implementation; the CLI shim in
//! `cli/logs.rs` does only arg parsing and delegates here.

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::types::RawLogEvent;
use crate::node::master_client::MasterClient;

const MAX_FORWARD_RETRIES: u32 = 5;

/// Configuration for a syslog forward session.
///
/// Constructed by the CLI from parsed `ForwardArgs`, but defined here so any
/// surface (CLI, future API) can build and pass it without duplicating the
/// field set.
pub struct ForwardConfig {
    pub master_url: String,
    pub token: Option<String>,
    pub node_id: String,
    pub batch_size: usize,
    pub syslog_only: bool,
}

/// Resolve the node ID from the environment or hostname.
pub fn resolve_node_id(explicit: Option<String>) -> String {
    explicit
        .or_else(|| std::env::var("LAB_NODE_ID").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(hostname_fallback)
}

fn hostname_fallback() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Return `true` when `journalctl` is available on `PATH`.
pub fn journald_available() -> bool {
    std::process::Command::new("journalctl")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run the forward loop, choosing journald or syslog file based on config.
pub async fn run(config: ForwardConfig) -> Result<std::process::ExitCode> {
    let client = MasterClient::with_bearer_token(config.master_url, config.token)?;
    tracing::info!(node_id = config.node_id.as_str(), "starting syslog forward");

    if !config.syslog_only && journald_available() {
        forward_journald(&client, &config.node_id, config.batch_size).await
    } else {
        #[cfg(unix)]
        {
            forward_syslog_file(&client, &config.node_id, config.batch_size).await
        }
        #[cfg(not(unix))]
        {
            anyhow::bail!("syslog file forwarding is not supported on this platform")
        }
    }
}

async fn forward_journald(
    client: &MasterClient,
    node_id: &str,
    batch_size: usize,
) -> Result<std::process::ExitCode> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::{Child, Command};
    use tokio::time::{Duration, interval};

    // tokio::process::Command is used so kill_on_drop(true) terminates the
    // journalctl process if this future is dropped (e.g., on channel close or
    // early return), preventing an orphaned --follow process.
    let mut child: Child = Command::new("journalctl")
        // --lines=100 picks up recent history immediately before following new entries.
        .args(["--follow", "--output=json", "--lines=100"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn journalctl")?;

    let stdout = child
        .stdout
        .take()
        .context("failed to capture journalctl stdout")?;

    let mut lines = BufReader::new(stdout).lines();

    let mut batch: Vec<RawLogEvent> = Vec::with_capacity(batch_size);
    let mut flush_tick = interval(Duration::from_secs(2));
    let mut retry_count = 0;
    flush_tick.tick().await; // consume the immediate first tick

    let exit_ok = loop {
        tokio::select! {
            line = lines.next_line() => {
                match line {
                    Ok(Some(l)) => {
                        batch.push(parse_journald_line(&l));
                        if batch.len() >= batch_size {
                            flush_batch(client, node_id, &mut batch, &mut retry_count).await?;
                        }
                    }
                    // EOF or read error — flush, then collect child exit status.
                    _ => {
                        if !batch.is_empty() {
                            flush_batch(client, node_id, &mut batch, &mut retry_count).await?;
                        }
                        let status = child
                            .wait()
                            .await
                            .map_err(|e| anyhow::anyhow!("journalctl wait: {e}"))?;
                        if !status.success() {
                            tracing::warn!(
                                node_id,
                                code = status.code(),
                                "journalctl exited with non-zero status"
                            );
                        }
                        break status.success();
                    }
                }
            }
            _ = flush_tick.tick() => {
                if !batch.is_empty() {
                    flush_batch(client, node_id, &mut batch, &mut retry_count).await?;
                }
            }
        }
    };

    Ok(if exit_ok {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    })
}

fn parse_journald_line(line: &str) -> RawLogEvent {
    let Ok(obj) = serde_json::from_str::<Value>(line) else {
        return RawLogEvent {
            message: line.to_string(),
            source_kind: Some("syslog".to_string()),
            ..default_raw_event()
        };
    };

    let message = obj
        .get("MESSAGE")
        .and_then(|v| v.as_str())
        .unwrap_or(line)
        .to_string();

    // journald PRIORITY: 0=emerg 1=alert 2=crit 3=err 4=warning 5=notice 6=info 7=debug
    let level = obj.get("PRIORITY").and_then(|v| v.as_str()).map(|p| {
        match p {
            "0" | "1" | "2" | "3" => "error",
            "4" => "warn",
            "5" | "6" => "info",
            "7" => "debug",
            _ => "info",
        }
        .to_string()
    });

    // __REALTIME_TIMESTAMP is microseconds since epoch.
    let ts = obj
        .get("__REALTIME_TIMESTAMP")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<i64>().ok())
        .map(|us| us / 1000); // convert to ms

    let upstream_event_id = obj
        .get("_MACHINE_ID")
        .zip(obj.get("__CURSOR"))
        .and_then(|(mid, cur)| Some(format!("{}:{}", mid.as_str()?, cur.as_str()?)));

    let unit = obj
        .get("_SYSTEMD_UNIT")
        .or_else(|| obj.get("UNIT"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    RawLogEvent {
        message,
        level,
        ts,
        source_kind: Some("syslog".to_string()),
        ingest_path: Some("journald".to_string()),
        upstream_event_id,
        action: unit,
        ..default_raw_event()
    }
}

#[cfg(unix)]
async fn forward_syslog_file(
    client: &MasterClient,
    node_id: &str,
    batch_size: usize,
) -> Result<std::process::ExitCode> {
    use std::fs::File;
    use std::io::{BufRead, BufReader, Seek, SeekFrom};
    use std::os::unix::fs::MetadataExt;
    use tokio::time::{Duration, sleep};

    let path = "/var/log/syslog";

    let mut file = File::open(path).with_context(|| format!("cannot open {path}"))?;
    file.seek(SeekFrom::End(0))?;
    let mut inode = file.metadata().map(|m| m.ino()).unwrap_or(0);

    let mut reader = BufReader::new(file);
    let mut batch: Vec<RawLogEvent> = Vec::with_capacity(batch_size);
    let mut line = String::new();
    let mut retry_count = 0;

    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .context("error reading syslog")?;

        if n == 0 {
            // No new data — flush any partial batch, check rotation, then wait.
            if !batch.is_empty() {
                flush_batch(client, node_id, &mut batch, &mut retry_count).await?;
            }

            // Check for file rotation or truncation (logrotate copytruncate).
            let rotated = tokio::task::spawn_blocking({
                let inode_now = inode;
                move || -> bool {
                    let Ok(meta) = std::fs::metadata(path) else {
                        return false;
                    };
                    meta.ino() != inode_now || meta.len() == 0
                }
            })
            .await
            .unwrap_or(false);

            if rotated {
                let new_file = tokio::task::spawn_blocking(move || {
                    File::open(path).with_context(|| format!("cannot reopen {path}"))
                })
                .await
                .map_err(|e| anyhow::anyhow!("join: {e}"))??;
                inode = new_file.metadata().map(|m| m.ino()).unwrap_or(0);
                reader = BufReader::new(new_file);
                tracing::info!(node_id, "syslog file rotated; reopened");
                continue;
            }

            sleep(Duration::from_millis(250)).await;
            continue;
        }

        let event = parse_syslog_line(line.trim_end());
        batch.push(event);

        if batch.len() >= batch_size {
            flush_batch(client, node_id, &mut batch, &mut retry_count).await?;
        }
    }
}

/// Skip `n` whitespace-delimited tokens and return the rest of the string.
///
/// Handles RFC 3164 double-space dates like "Nov  2 10:30:00" (day < 10).
#[cfg(unix)]
fn skip_whitespace_tokens<'a>(s: &'a str, n: usize) -> &'a str {
    let mut rest = s;
    for _ in 0..n {
        rest = rest.trim_start_matches(|c: char| !c.is_ascii_whitespace());
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace());
    }
    rest
}

#[cfg(unix)]
fn parse_syslog_line(line: &str) -> RawLogEvent {
    // Best-effort RFC 3164 parse: "Mon DD HH:MM:SS host tag: message"
    // RFC 3164 uses a single space between fields, but some syslogd
    // implementations emit "Nov  2" (double space before single-digit days).
    // Skip 3 whitespace token groups so double spaces are absorbed correctly.
    let rest = skip_whitespace_tokens(line, 3);
    let message = (if rest.is_empty() { line } else { rest }).to_string();
    RawLogEvent {
        message,
        source_kind: Some("syslog".to_string()),
        ingest_path: Some("syslog_file".to_string()),
        ..default_raw_event()
    }
}

fn default_raw_event() -> RawLogEvent {
    RawLogEvent {
        ts: None,
        level: None,
        subsystem: Some("syslog".to_string()),
        surface: Some("core_runtime".to_string()),
        action: None,
        message: String::new(),
        request_id: None,
        session_id: None,
        correlation_id: None,
        trace_id: None,
        span_id: None,
        instance: None,
        auth_flow: None,
        outcome_kind: None,
        fields_json: Value::Object(Default::default()),
        source_kind: None,
        source_node_id: None,
        source_device_id: None,
        actor_key: None,
        ingest_path: None,
        upstream_event_id: None,
    }
}

async fn flush_batch(
    client: &MasterClient,
    node_id: &str,
    batch: &mut Vec<RawLogEvent>,
    retry_count: &mut u32,
) -> Result<()> {
    // Take ownership so we can return events to the batch on failure.
    let events = std::mem::take(batch);
    let payload = json!({
        "node_id": node_id,
        "events": &events,
    });
    match client.post_log_ingest(&payload).await {
        Ok(resp) => {
            *retry_count = 0;
            tracing::debug!(
                surface = "node",
                service = "logs",
                action = "logs.forward",
                event = "logs.forward.flush",
                node_id,
                accepted = resp.get("accepted").and_then(|v| v.as_u64()),
                dropped = resp.get("dropped").and_then(|v| v.as_u64()),
                "flushed log batch"
            );
        }
        Err(e) => {
            // Restore events so the next flush attempt can retry them.
            *retry_count = retry_count.saturating_add(1);
            let destination = client.base_url();
            if *retry_count >= MAX_FORWARD_RETRIES {
                tracing::error!(
                    surface = "node",
                    service = "logs",
                    action = "logs.forward",
                    event = "logs.forward.retry_exhausted",
                    kind = "forward_retry_exhausted",
                    node_id,
                    destination,
                    retry_count = *retry_count,
                    max_retries = MAX_FORWARD_RETRIES,
                    events = events.len(),
                    error = %e,
                    "log forward retry budget exhausted",
                );
                anyhow::bail!(
                    "failed to forward log batch to {destination} after {retry_count} retries: {e}"
                );
            }
            tracing::warn!(
                surface = "node",
                service = "logs",
                action = "logs.forward",
                event = "logs.forward.transient_failure",
                kind = "forward_failed",
                node_id,
                destination,
                retry_count = *retry_count,
                max_retries = MAX_FORWARD_RETRIES,
                events = events.len(),
                error = %e,
                "failed to flush log batch; will retry",
            );
            *batch = events;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn forward_retry_logs_include_retry_and_destination_fields() {
        let source = include_str!("forward.rs");
        for field in [
            "MAX_FORWARD_RETRIES",
            "event = \"logs.forward.transient_failure\"",
            "event = \"logs.forward.retry_exhausted\"",
            "kind = \"forward_failed\"",
            "kind = \"forward_retry_exhausted\"",
            "destination",
            "retry_count",
        ] {
            assert!(source.contains(field), "missing forward log field: {field}");
        }
    }
}
