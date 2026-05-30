use chrono::Utc;
use clap::Parser;
use serde::Serialize;
use serde_json::{Value, json};
use std::fs::{File, OpenOptions, create_dir_all};
use std::io::Write;
use std::path::{Path, PathBuf};
use ulid::Ulid;

#[derive(Parser)]
#[command(name = "broadcastr-emit")]
struct Args {
    #[arg(long)]
    category: String,
    #[arg(long)]
    tier: String,
    #[arg(long)]
    summary: String,
    #[arg(long, default_value = "cli")]
    source: String,
    #[arg(long)]
    data: Option<String>,
    #[arg(long)]
    branch: Option<String>,
}

#[derive(Serialize)]
struct Emitter {
    session_id: Option<String>,
    agent: String,
    host: String,
    user: String,
}

fn main() {
    // Hooks invoke this on every Bash tool call. Best-effort: never panic
    // out, never bubble a non-zero exit to the caller. Failures get logged
    // to stderr (which Claude Code typically discards for hooks) and we
    // exit 0 so the user's actual workflow is unaffected.
    if let Err(e) = try_main() {
        eprintln!("broadcastr-emit: {e}");
    }
}

fn try_main() -> std::io::Result<()> {
    if std::env::var("BROADCASTR_DISABLED").as_deref() == Ok("1") {
        return Ok(());
    }
    let args = Args::parse();

    let repo = std::env::var("CLAUDE_PROJECT_DIR")
        .or_else(|_| std::env::var("PWD"))
        .unwrap_or_else(|_| ".".to_string());
    let per_repo_bus = PathBuf::from(&repo).join(".broadcastr/events.jsonl");

    let global_bus = std::env::var("BROADCASTR_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".claude/broadcastr"))
        })
        .map(|h| h.join("events.jsonl"));

    let event = build_event(&args, &repo);
    let line = serde_json::to_string(&event)
        .map_err(|e| std::io::Error::other(format!("serialize event: {e}")))?;

    append_line(&per_repo_bus, &line)?;

    if std::env::var("BROADCASTR_GLOBAL_FEED").as_deref() != Ok("0")
        && let Some(g) = global_bus
    {
        append_line(&g, &line)?;
    }
    Ok(())
}

fn build_event(args: &Args, repo: &str) -> Value {
    let session_id = std::env::var("CLAUDE_SESSION_ID").ok();
    let agent = if session_id.is_some() { "claude-code" } else { "user" };
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    let ulid = Ulid::new().to_string();
    let ts = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    // Malformed --data is preserved in an envelope with a parse-error
    // marker, matching the bash fallback's contract. Silent coercion to
    // `{}` would mask hook bugs that produce bad JSON.
    let data: Value = match args.data.as_deref() {
        None => json!({}),
        Some(s) => match serde_json::from_str::<Value>(s) {
            Ok(v) => v,
            Err(e) => json!({
                "_parse_error": format!("invalid JSON in --data: {e}"),
                "_raw": s,
            }),
        },
    };
    let mut event = json!({
        "ts": ts,
        "id": format!("evt_{}", ulid),
        "tier": args.tier,
        "category": args.category,
        "source": args.source,
        "emitter": Emitter { session_id, agent: agent.to_string(), host, user },
        "repo": repo,
        "summary": args.summary,
        "data": data,
    });
    if let Some(b) = &args.branch {
        event["branch"] = json!(b);
    }
    event
}

fn append_line(bus: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = bus.parent() {
        create_dir_all(parent)?;
    }
    maybe_rotate(bus)?;
    let mut f = OpenOptions::new().create(true).append(true).open(bus)?;
    // Single combined buffer + write_all. Under O_APPEND, std's write_all
    // issues a single write(2) for small buffers (well under PIPE_BUF) and
    // loops on EINTR / short writes if they ever occur. Each kernel-level
    // write under O_APPEND atomically seeks to EOF, so concurrent emitters
    // cannot interleave bytes within a single write call.
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    f.write_all(buf.as_bytes())?;
    Ok(())
}

fn maybe_rotate(bus: &Path) -> std::io::Result<()> {
    let max_bytes: u64 = std::env::var("BROADCASTR_BUS_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5 * 1024 * 1024);
    let retain: u32 = std::env::var("BROADCASTR_BUS_RETAIN")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3)
        .max(1);

    let size = match std::fs::metadata(bus) {
        Ok(m) => m.len(),
        Err(_) => return Ok(()),
    };
    if size < max_bytes {
        return Ok(());
    }

    let base_name = bus
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "events.jsonl".to_string());
    let lock_path = bus.with_file_name(format!("{base_name}.rotate.lock"));
    let lock_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    // Non-blocking exclusive lock: losers skip rotation this turn and the
    // next emit retries. Lock is released when `lock_file` drops.
    if File::try_lock(&lock_file).is_err() {
        return Ok(());
    }

    // Re-check size under lock (TOCTOU)
    let size = match std::fs::metadata(bus) {
        Ok(m) => m.len(),
        Err(_) => return Ok(()),
    };
    if size < max_bytes {
        return Ok(());
    }

    // Shift rotations: .N-1 → .N for i from retain-1 down to 1.
    // Renames that fail (EACCES, no such file across rotations) are
    // intentionally swallowed: we cannot meaningfully recover, and the
    // upper bound on damage is "one rotation slot's worth of events lost"
    // which is already the documented best-effort contract.
    for i in (1..retain).rev() {
        let from = bus.with_file_name(format!("{base_name}.{i}"));
        let to = bus.with_file_name(format!("{base_name}.{}", i + 1));
        if from.exists() {
            let _ = std::fs::rename(&from, &to);
        }
    }
    let dot1 = bus.with_file_name(format!("{base_name}.1"));
    let _ = std::fs::rename(bus, &dot1);
    // Touch the new bus into existence WITHOUT truncating — concurrent
    // emitters may have already raced through O_CREAT|O_APPEND and written
    // events here; we must preserve them.
    let _ = OpenOptions::new().create(true).append(true).open(bus);
    Ok(())
}
