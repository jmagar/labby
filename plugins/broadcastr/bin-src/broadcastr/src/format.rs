use serde_json::Value;

/// Format a bus event into a display line, or `None` if it should be suppressed.
pub fn format_event(
    event: &Value,
    session_id: &str,
    startup_ts: &str,
    mute: &[String],
) -> Option<String> {
    // Self-suppression: drop our own session's events.
    if !session_id.is_empty() {
        if let Some(sid) = event["emitter"]["session_id"].as_str() {
            if sid == session_id {
                return None;
            }
        }
    }

    // Drop events that predated monitor startup.
    if event["ts"].as_str().is_some_and(|ts| ts <= startup_ts) {
        return None;
    }

    // Muted categories.
    let category = event["category"].as_str().unwrap_or("");
    if mute.iter().any(|m| m == category) {
        return None;
    }

    let tier_icon = if event["tier"].as_str() == Some("alert") { "🚨" } else { "📡" };
    let glyph = category_glyph(category);
    let proj = event["repo"].as_str()
        .and_then(|r| r.rsplit('/').next())
        .unwrap_or("?");
    let summary = display_summary(event);

    Some(format!("{tier_icon} {glyph}[{proj}] {summary}"))
}

fn category_glyph(category: &str) -> &'static str {
    match category {
        "agent-presence"                              => "👤",
        "commit" | "push" | "pre-commit"
        | "branch" | "stash"                         => "🌿",
        "session-doc" | "plan" | "plan-exec"         => "📝",
        "bead"                                        => "🎯",
        _                                             => "•",
    }
}

fn agent_name(event: &Value) -> &str {
    // data.agent is set by session hooks (hook-on-session-start.sh etc.)
    if let Some(a) = event["data"]["agent"].as_str() {
        return a;
    }
    if event["emitter"]["agent"].as_str() == Some("claude-code") {
        return "Claude";
    }
    "Claude"
}

fn display_summary(event: &Value) -> String {
    let ag = agent_name(event);
    let category = event["category"].as_str().unwrap_or("");
    let summary = event["summary"].as_str().unwrap_or("");
    let branch = event["branch"].as_str().unwrap_or("?");

    match category {
        "agent-presence" => {
            if event["data"]["action"].as_str() == Some("left") {
                format!("{ag} left")
            } else {
                format!("{ag} joined")
            }
        }

        "session-doc" => {
            let fname = event["data"]["path"].as_str()
                .and_then(|p| p.rsplit('/').next())
                .unwrap_or_else(|| summary.strip_prefix("session doc: ").unwrap_or(summary));
            format!("{ag} saved: {fname}")
        }

        "plan" | "plan-exec" => {
            let fname = event["data"]["path"].as_str()
                .and_then(|p| p.rsplit('/').next())
                .unwrap_or_else(|| {
                    summary.strip_prefix("plan edit: ")
                        .or_else(|| summary.strip_prefix("plan-exec: "))
                        .unwrap_or(summary)
                });
            format!("{ag} edited: {fname}")
        }

        "commit" => {
            if event["data"]["subtype"].as_str() == Some("merge") {
                format!("{ag} merged · {branch}")
            } else {
                let rest = summary.strip_prefix("commit ").unwrap_or(summary);
                format!("{ag} made commit {rest}")
            }
        }

        "push" => match event["data"]["subtype"].as_str() {
            Some("attempt") => format!("{ag} pushing · {branch}"),
            Some("fail") => format!("{ag}'s push FAILED · {branch}"),
            _ if summary.to_ascii_uppercase().contains("FAIL") =>
                format!("{ag}'s push FAILED · {branch}"),
            _ => format!("{ag} pushed · {branch}"),
        },

        "pre-commit" => {
            let up = summary.to_ascii_uppercase();
            if event["tier"].as_str() == Some("alert") || up.contains("FAIL") {
                format!("{ag} pre-commit FAILED · {branch}")
            } else if up.contains("PASS") {
                format!("{ag} pre-commit ✓ · {branch}")
            } else {
                format!("{ag} pre-commit starting · {branch}")
            }
        }

        "branch" => {
            let name = event["data"]["branch"].as_str()
                .unwrap_or_else(|| summary.strip_prefix("checkout: ").unwrap_or(summary));
            format!("{ag} switched to · {name}")
        }

        "bead" => {
            let cmd = event["data"]["cmd"].as_str().unwrap_or(summary);
            let cmd = cmd.strip_prefix("bd: bd ").or_else(|| cmd.strip_prefix("bd ")).unwrap_or(cmd);
            let mut parts = cmd.split_whitespace();
            let verb = match parts.next().unwrap_or("?") {
                "close"  => "closed",
                "create" => "created",
                "update" => "updated",
                "reopen" => "reopened",
                other    => other,
            };
            match parts.find(|p| p.starts_with("beads-")) {
                Some(issue) => format!("{ag} {verb} {issue}"),
                None        => format!("{ag} {verb}"),
            }
        }

        "stash" => format!("{ag} stashed · {branch}"),

        _ => summary.to_string(),
    }
}
