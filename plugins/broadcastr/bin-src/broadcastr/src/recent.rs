use std::collections::HashSet;
use std::io;

use chrono::{Duration, Utc};
use serde_json::Value;

use crate::config::Config;

pub fn run(config: &Config, since: &str) -> io::Result<()> {
    let cutoff = parse_since(since);

    let mut buses = vec![config.per_repo_bus.clone()];
    if config.want_global {
        if let Some(g) = &config.global_bus {
            buses.push(g.clone());
        }
    }

    let mut events: Vec<Value> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for bus in &buses {
        let Ok(content) = std::fs::read_to_string(bus) else { continue };
        for line in content.lines() {
            let Ok(ev) = serde_json::from_str::<Value>(line) else { continue };
            if ev["ts"].as_str().is_some_and(|ts| ts >= cutoff.as_str()) {
                let id = ev["id"].as_str().unwrap_or("").to_string();
                if seen.insert(id) {
                    events.push(ev);
                }
            }
        }
    }

    events.sort_by(|a, b| {
        a["ts"].as_str().unwrap_or("").cmp(b["ts"].as_str().unwrap_or(""))
    });

    for ev in &events {
        println!("{}", serde_json::to_string(ev).unwrap_or_default());
    }
    Ok(())
}

fn parse_since(since: &str) -> String {
    let since = since.trim();
    // Split on last character as unit: "5m", "30s", "2h", "1d"
    let (num_str, unit) = since.split_at(since.len().saturating_sub(1));
    let n: i64 = num_str.parse().unwrap_or(5);
    let duration = match unit {
        "s" => Duration::seconds(n),
        "h" => Duration::hours(n),
        "d" => Duration::days(n),
        _   => Duration::minutes(n),
    };
    (Utc::now() - duration)
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}
