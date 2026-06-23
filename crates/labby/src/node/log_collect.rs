use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};

use crate::node::log_event::NodeLogEvent;

const BOOTSTRAP_LOG_LINE_LIMIT: usize = 128;
const BOOTSTRAP_LOG_BYTE_LIMIT: u64 = 256 * 1024;
const CANDIDATE_LOG_PATHS: &[&str] = &["/var/log/syslog", "/var/log/messages"];

pub fn collect_bootstrap_logs(node_id: &str) -> Result<Vec<NodeLogEvent>> {
    for candidate in CANDIDATE_LOG_PATHS {
        let path = Path::new(candidate);
        let Ok(metadata) = fs::metadata(path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }

        let raw = match read_tail(path, BOOTSTRAP_LOG_BYTE_LIMIT as usize) {
            Ok(raw) => raw,
            Err(error) => {
                tracing::debug!(path = %path.display(), error = %error, "skipping unreadable bootstrap log candidate");
                continue;
            }
        };
        let timestamp_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let events = String::from_utf8_lossy(&raw)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .rev()
            .take(BOOTSTRAP_LOG_LINE_LIMIT)
            .map(|line| NodeLogEvent {
                node_id: node_id.to_string(),
                source: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("syslog")
                    .to_string(),
                timestamp_unix_ms,
                level: None,
                message: line.to_string(),
                fields: Default::default(),
            })
            .collect::<Vec<_>>();
        if !events.is_empty() {
            return Ok(events.into_iter().rev().collect());
        }
    }

    bail!("no readable bootstrap log source found under /var/log/syslog or /var/log/messages")
}

fn read_tail(path: &Path, max_len: usize) -> std::io::Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len() as usize;
    let start = len.saturating_sub(max_len) as u64;
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity(len.saturating_sub(start as usize));
    file.read_to_end(&mut buf)?;
    Ok(buf)
}
