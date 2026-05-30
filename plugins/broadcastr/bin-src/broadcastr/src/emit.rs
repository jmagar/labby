use std::io;

use chrono::Utc;
use serde_json::{Value, json};
use ulid::Ulid;

use crate::bus;
use crate::config::Config;

pub fn run(
    config: &Config,
    category: &str,
    tier: &str,
    summary: &str,
    data: Option<&str>,
    source: &str,
    branch: Option<&str>,
    use_session_id: bool,
) -> io::Result<()> {
    if config.disabled {
        return Ok(());
    }

    let line = build_line(category, tier, summary, data, source, branch, use_session_id)?;

    bus::append(config, &config.per_repo_bus, &line)?;

    if config.want_global {
        if let Some(g) = &config.global_bus {
            bus::append(config, g, &line)?;
        }
    }
    Ok(())
}

fn build_line(
    category: &str,
    tier: &str,
    summary: &str,
    data: Option<&str>,
    source: &str,
    branch: Option<&str>,
    use_session_id: bool,
) -> io::Result<String> {
    let session_id = if use_session_id {
        std::env::var("CLAUDE_SESSION_ID").ok()
    } else {
        None
    };
    let agent = if session_id.is_some() { "claude-code" } else { "user" };
    let repo = std::env::var("CLAUDE_PROJECT_DIR")
        .or_else(|_| std::env::var("PWD"))
        .unwrap_or_else(|_| ".".into());
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".into());
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".into());

    let data_val: Value = data
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(json!({}));

    let mut event = json!({
        "ts":       Utc::now().format("%Y-%m-%dT%H:%M:%S%.9fZ").to_string(),
        "id":       format!("evt_{}", Ulid::new()),
        "tier":     tier,
        "category": category,
        "source":   source,
        "emitter": {
            "session_id": session_id,
            "agent":      agent,
            "host":       host,
            "user":       user,
        },
        "repo":    repo,
        "summary": summary,
        "data":    data_val,
    });

    if let Some(b) = branch {
        event["branch"] = json!(b);
    }

    serde_json::to_string(&event)
        .map_err(|e| io::Error::other(format!("serialize event: {e}")))
}
