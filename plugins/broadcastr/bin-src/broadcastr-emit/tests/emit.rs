use std::fs;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn emit_appends_one_jsonl_line_to_per_repo_bus() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(repo.join(".broadcastr")).unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_broadcastr-emit"))
        .env("CLAUDE_PROJECT_DIR", &repo)
        .env("BROADCASTR_HOME", &home)
        .env("BROADCASTR_GLOBAL_FEED", "0")
        .env("HOSTNAME", "testbox")
        .env("USER", "tester")
        .env_remove("CLAUDE_SESSION_ID")
        .env_remove("BROADCASTR_DISABLED")
        .args([
            "--category", "commit", "--tier", "info", "--summary", "test commit",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let bus = fs::read_to_string(repo.join(".broadcastr/events.jsonl")).unwrap();
    let lines: Vec<&str> = bus.trim().lines().collect();
    assert_eq!(lines.len(), 1);
    let evt: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(evt["category"], "commit");
    assert_eq!(evt["tier"], "info");
    assert_eq!(evt["summary"], "test commit");
    assert_eq!(evt["emitter"]["host"], "testbox");
    assert_eq!(evt["emitter"]["user"], "tester");
    assert!(evt["id"].as_str().unwrap().starts_with("evt_"));
}

#[test]
fn emit_writes_to_global_bus_when_enabled() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(repo.join(".broadcastr")).unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    Command::new(env!("CARGO_BIN_EXE_broadcastr-emit"))
        .env("CLAUDE_PROJECT_DIR", &repo)
        .env("BROADCASTR_HOME", &home)
        .env("BROADCASTR_GLOBAL_FEED", "1")
        .env("HOSTNAME", "testbox")
        .env("USER", "tester")
        .env_remove("BROADCASTR_DISABLED")
        .args(["--category", "commit", "--tier", "info", "--summary", "x"])
        .status()
        .unwrap();

    let global = fs::read_to_string(home.join("events.jsonl")).unwrap();
    assert_eq!(global.trim().lines().count(), 1);
}

#[test]
fn emit_wraps_malformed_data_with_parse_error_envelope() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(repo.join(".broadcastr")).unwrap();

    Command::new(env!("CARGO_BIN_EXE_broadcastr-emit"))
        .env("CLAUDE_PROJECT_DIR", &repo)
        .env("BROADCASTR_GLOBAL_FEED", "0")
        .env("HOSTNAME", "h")
        .env("USER", "u")
        .env_remove("BROADCASTR_DISABLED")
        .args([
            "--category", "commit", "--tier", "info", "--summary", "x",
            "--data", "not json at all",
        ])
        .status()
        .unwrap();

    let bus = fs::read_to_string(repo.join(".broadcastr/events.jsonl")).unwrap();
    let evt: serde_json::Value = serde_json::from_str(bus.trim()).unwrap();
    assert_eq!(evt["data"]["_raw"], "not json at all");
    assert!(
        evt["data"]["_parse_error"]
            .as_str()
            .unwrap_or("")
            .contains("invalid JSON")
    );
}

#[test]
fn emit_is_silent_when_disabled() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(repo.join(".broadcastr")).unwrap();

    Command::new(env!("CARGO_BIN_EXE_broadcastr-emit"))
        .env("CLAUDE_PROJECT_DIR", &repo)
        .env("BROADCASTR_DISABLED", "1")
        .args(["--category", "commit", "--tier", "info", "--summary", "x"])
        .status()
        .unwrap();

    assert!(!repo.join(".broadcastr/events.jsonl").exists());
}

#[test]
fn emit_rotates_when_bus_exceeds_max_bytes() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let bus_dir = repo.join(".broadcastr");
    fs::create_dir_all(&bus_dir).unwrap();
    let bus = bus_dir.join("events.jsonl");
    // Pre-fill with padding lines (each terminated by \n so they line-split cleanly)
    let padding = "x".repeat(63);
    let mut buf = String::new();
    for _ in 0..40 {
        buf.push_str(&padding);
        buf.push('\n');
    }
    fs::write(&bus, &buf).unwrap();

    Command::new(env!("CARGO_BIN_EXE_broadcastr-emit"))
        .env("CLAUDE_PROJECT_DIR", &repo)
        .env("BROADCASTR_GLOBAL_FEED", "0")
        .env("BROADCASTR_BUS_MAX_BYTES", "1024")
        .env("BROADCASTR_BUS_RETAIN", "3")
        .env("HOSTNAME", "h")
        .env("USER", "u")
        .env_remove("BROADCASTR_DISABLED")
        .args(["--category", "commit", "--tier", "info", "--summary", "x"])
        .status()
        .unwrap();

    assert!(
        bus_dir.join("events.jsonl.1").exists(),
        "rotated file should exist"
    );
    let new_bus = fs::read_to_string(&bus).unwrap();
    assert_eq!(new_bus.trim().lines().count(), 1);
}

#[test]
fn concurrent_rotation_does_not_clobber() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let bus_dir = repo.join(".broadcastr");
    fs::create_dir_all(&bus_dir).unwrap();
    // Pre-fill with padding lines terminated by \n so the test's line-based
    // parser can cleanly distinguish padding from real events.
    let padding = "x".repeat(63);
    let mut buf = String::new();
    for _ in 0..40 {
        buf.push_str(&padding);
        buf.push('\n');
    }
    fs::write(bus_dir.join("events.jsonl"), &buf).unwrap();

    let mut handles = vec![];
    for _ in 0..8 {
        let repo = repo.clone();
        handles.push(std::thread::spawn(move || {
            Command::new(env!("CARGO_BIN_EXE_broadcastr-emit"))
                .env("CLAUDE_PROJECT_DIR", &repo)
                .env("BROADCASTR_GLOBAL_FEED", "0")
                .env("BROADCASTR_BUS_MAX_BYTES", "1024")
                .env("BROADCASTR_BUS_RETAIN", "3")
                .env("HOSTNAME", "h")
                .env("USER", "u")
                .env_remove("BROADCASTR_DISABLED")
                .args(["--category", "commit", "--tier", "info", "--summary", "x"])
                .status()
                .unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    assert!(bus_dir.join("events.jsonl.1").exists());

    // The real invariant: under flock-guarded rotation, the rotation chain
    // is well-formed and at least one new event landed somewhere. (With
    // ~280-byte events and 1024-byte threshold, multiple rotations under
    // concurrent load are expected and correct.)
    let mut total_new_events = 0usize;
    for name in ["events.jsonl", "events.jsonl.1", "events.jsonl.2", "events.jsonl.3"] {
        let p = bus_dir.join(name);
        if !p.exists() {
            continue;
        }
        let content = fs::read_to_string(&p).unwrap();
        for line in content.lines() {
            // Skip the pre-fill padding
            if line.starts_with("x") {
                continue;
            }
            // Anything else valid JSON counts as a real event
            if serde_json::from_str::<serde_json::Value>(line).is_ok() {
                total_new_events += 1;
            }
        }
    }
    assert_eq!(
        total_new_events, 8,
        "all 8 emits must be preserved across the rotation chain; got {}",
        total_new_events
    );

    // Bound the rotation chain: with retain=3 we should have at most .3
    assert!(
        !bus_dir.join("events.jsonl.4").exists(),
        "retain=3 must drop rotations beyond .3"
    );
}
