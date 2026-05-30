use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use chrono::Utc;
use notify::{EventKind, RecursiveMode, Watcher};
use serde_json::Value;

use crate::bus::BusTailer;
use crate::config::Config;
use crate::emit;
use crate::format::format_event;

/// Start all background workers (fs watchers + alert gateway) then run the feed.
pub fn run(config: &Config) -> io::Result<()> {
    let c = config.clone();
    thread::spawn(move || watch_session_dirs(&c));

    let c = config.clone();
    thread::spawn(move || watch_plan_dirs(&c));

    let c = config.clone();
    thread::spawn(move || run_alert_gateway(&c));

    run_feed(config)
}

/// Feed only: tail bus, format events, print to stdout. Used by `broadcastr tail`.
pub fn run_feed(config: &Config) -> io::Result<()> {
    let startup = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    let mut bus_paths = vec![config.per_repo_bus.clone()];
    if config.want_global {
        if let Some(g) = &config.global_bus {
            bus_paths.push(g.clone());
        }
    }
    // Ensure bus files exist before tailing.
    for p in &bus_paths {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).ok();
        }
        let _ = fs::OpenOptions::new().create(true).append(true).open(p);
    }

    let mut tailer = BusTailer::new(bus_paths);
    // Seen-set deduplicates events that appear in both per-repo and global buses.
    let mut seen: HashSet<String> = HashSet::new();

    loop {
        for line in tailer.poll() {
            if let Ok(event) = serde_json::from_str::<Value>(&line) {
                // Dedup by event id; flush set at 10k to keep memory bounded.
                if let Some(id) = event["id"].as_str() {
                    if !seen.insert(id.to_string()) {
                        continue;
                    }
                    if seen.len() > 10_000 {
                        seen.clear();
                    }
                }
                if let Some(line) = format_event(&event, &config.session_id, &startup, &config.mute) {
                    println!("{line}");
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
}

// ── FS watchers ────────────────────────────────────────────────────────────────

fn watch_session_dirs(config: &Config) {
    let dirs = vec![config.repo.join("docs/sessions")];
    watch_and_emit(config, &dirs, "session-doc", |path| {
        let base = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let path_str = path.to_string_lossy();
        (
            format!("session doc: {base}"),
            format!(r#"{{"path":"{path_str}"}}"#),
        )
    });
}

fn watch_plan_dirs(config: &Config) {
    let dirs = vec![
        config.repo.join("docs/plans"),
        config.repo.join("docs/superpowers/plans"),
    ];
    watch_and_emit(config, &dirs, "plan", |path| {
        let base = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let path_str = path.to_string_lossy();
        (
            format!("plan edit: {base}"),
            format!(r#"{{"path":"{path_str}"}}"#),
        )
    });
}

/// Watch `dirs` for `.md` creates/modifies and emit an event for each.
/// `summary_fn` returns (summary_string, data_json_string) given the path.
///
/// Events emitted here use `use_session_id = false` so self-suppression in the
/// feed loop does not hide them — the file was written by another session, but
/// inotify fired in the monitor's process.
fn watch_and_emit<F>(config: &Config, dirs: &[PathBuf], category: &str, summary_fn: F)
where
    F: Fn(&std::path::Path) -> (String, String),
{
    for dir in dirs {
        fs::create_dir_all(dir).ok();
    }

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = match notify::recommended_watcher(move |res| { tx.send(res).ok(); }) {
        Ok(w) => w,
        Err(e) => { eprintln!("broadcastr: watcher init failed: {e}"); return; }
    };

    let mut armed = false;
    for dir in dirs {
        if watcher.watch(dir, RecursiveMode::NonRecursive).is_ok() {
            armed = true;
        }
    }
    if !armed {
        return;
    }

    for result in rx {
        let Ok(event) = result else { continue };
        if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
            continue;
        }
        for path in &event.paths {
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let (summary, data) = summary_fn(path);
            let _ = emit::run(config, category, "info", &summary,
                               Some(&data), "inotify", None,
                               false /* use_session_id */);
        }
    }
}

// ── Alert gateway ──────────────────────────────────────────────────────────────

fn run_alert_gateway(config: &Config) {
    if !config.apprise_enabled {
        return;
    }
    if !apprise_available() {
        eprintln!("broadcastr: apprise not found; alert gateway disabled");
        return;
    }

    let startup = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    // Alert gateway reads the global bus only.
    let bus_paths = config.global_bus.as_ref()
        .map(|g| vec![g.clone()])
        .unwrap_or_else(|| vec![config.per_repo_bus.clone()]);

    let mut tailer = BusTailer::new(bus_paths);

    loop {
        for line in tailer.poll() {
            let Ok(event) = serde_json::from_str::<Value>(&line) else { continue };
            if event["tier"].as_str() != Some("alert") { continue; }
            if event["ts"].as_str().is_some_and(|ts| ts <= startup.as_str()) { continue; }
            if let Some(msg) = event["summary"].as_str() {
                let _ = Command::new("apprise")
                    .args(["--tag", &config.apprise_tag, "--body", msg])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn apprise_available() -> bool {
    Command::new("apprise")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}
