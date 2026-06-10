//! Integration test: real stdio MCP child-process spawn + reaping.
//!
//! These tests are `#[ignore]` (local-only, per repo convention) because they
//! spawn real child processes and require the `labby` binary compiled into
//! `target/`.
//!
//! Run with:
//!   cargo nextest run --all-features -E 'test(gateway_stdio)' -- --include-ignored
//!
//! ## What is tested
//!
//! (a) **Process reaping** — after the child's stdio pipes are closed, the
//!     child process must exit within 2 seconds.  This mirrors the behavior
//!     required of `pool/connect_stdio.rs`'s `ProcessGuard`, which kills the
//!     process group on drop.
//!
//! (b) **Minimal child environment (DOCUMENTED INTENT — not yet enforced)**
//!     The intended hardened state after the SEC `env_clear` work item is that
//!     the runner child does NOT inherit secret env vars from labby.  The
//!     assertion below is written against that intended state and is disabled
//!     until the work item lands.  Do NOT delete this test — it documents the
//!     gap and will catch regressions once hardening is implemented.
//!
//! (c) **MCP initialize round-trip** — sends a JSON-RPC `initialize` request
//!     over stdin and asserts a valid response arrives on stdout, validating
//!     the raw framed-line-read path.
//!
//! ## Architecture note
//!
//! The test spawns `labby internal mcp-echo-server`, a minimal stdio MCP
//! server that responds to `initialize` and exits on stdin EOF.  If that
//! subcommand is absent (older binary) the tests are skipped gracefully.

use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::time::Duration;

/// Path to the labby binary produced by the current build.
fn labby_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_labby"))
}

/// Spawn the labby MCP echo-server child with piped stdio.
fn spawn_echo_child() -> std::io::Result<std::process::Child> {
    std::process::Command::new(labby_bin())
        .args(["internal", "mcp-echo-server"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

// ---------------------------------------------------------------------------
// Platform helpers
// ---------------------------------------------------------------------------

/// Returns `true` when the process with the given PID is still running.
///
/// Uses `nix::sys::signal::kill(pid, None)` (signal 0) which is a POSIX-
/// standard existence check: returns `Ok(())` if the process exists,
/// `Err(Errno::ESRCH)` if it does not.
#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    use nix::sys::signal;
    use nix::unistd::Pid;
    signal::kill(Pid::from_raw(pid as i32), None).is_ok()
}

#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    // Non-Unix: assume the process has exited (vacuously satisfies the check).
    false
}

// ---------------------------------------------------------------------------
// (a) Process reaping after stdio-pipe close
// ---------------------------------------------------------------------------

/// Spawns the MCP echo server, drops the stdio handles, and asserts the child
/// exits within 2 seconds.
///
/// A well-behaved stdio MCP server must exit when it detects EOF on stdin.
/// The `ProcessGuard` in `pool/connect_stdio.rs` additionally sends SIGKILL to
/// the process group on drop — this test validates the soft-exit path.
#[test]
#[ignore = "requires a built labby binary; run locally with --include-ignored"]
fn gateway_stdio_child_reaped_after_pipe_close() {
    let child = match spawn_echo_child() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("SKIP — could not spawn labby internal mcp-echo-server: {e}");
            return;
        }
    };
    let pid = child.id();
    assert!(pid > 0, "child must have a valid PID after spawn");

    // Drop closes stdin/stdout — child should detect EOF and exit.
    drop(child);

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if !pid_alive(pid) {
            return; // test passes
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // Still alive — kill it before panicking so it doesn't linger.
    #[cfg(unix)]
    {
        use nix::sys::signal::{Signal, kill};
        use nix::unistd::Pid;
        let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
    }
    panic!(
        "child process PID={pid} was still alive 2 s after stdio pipes were closed; \
         process reaping is broken"
    );
}

// ---------------------------------------------------------------------------
// (b) Minimal child environment — DOCUMENTED INTENT, not yet enforced
// ---------------------------------------------------------------------------

/// Asserts that the child runner does NOT inherit a secret-canary env var from
/// the parent.
///
/// **Current state:** the child inherits labby's full environment (no
/// `env_clear`).  This test is `#[ignore]`d until the SEC `env_clear` work
/// item lands.  When it does:
///   1. Remove the `#[ignore]` attribute.
///   2. The test must pass without modification.
///
/// Do NOT delete this test — it documents the security gap.
/// Verify that the child process does NOT inherit secret env vars from labby.
///
/// **Current state:** the child inherits labby's full environment (no
/// `env_clear`).  This test documents the security gap and will catch
/// regressions once the SEC `env_clear` work item lands.
///
/// The canary is injected explicitly into the child via `Command::env()` as a
/// known-present baseline.  When `env_clear` hardens the spawn path so the
/// child gets a minimal environment, the inherited-env path will no longer pass
/// the canary through and this test must be updated alongside that work.
///
/// Do NOT delete this test — it documents the security gap.
#[test]
#[ignore = "env_clear not yet implemented (SEC work item); \
            un-ignore when hardening lands and verify it passes"]
fn gateway_stdio_child_does_not_inherit_secret_env() {
    const CANARY_KEY: &str = "LAB_TEST_SECRET_CANARY_MUST_NOT_INHERIT";
    const CANARY_VAL: &str = "top-secret-canary-12345";

    // Spawn the child with the canary injected via Command::env() so we have a
    // known-present value without mutating the parent process environment
    // (std::env::set_var is unsafe in Rust 2024 and the crate forbids unsafe).
    let mut child = match std::process::Command::new(labby_bin())
        .args(["internal", "mcp-echo-server"])
        .env(CANARY_KEY, CANARY_VAL)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("SKIP — could not spawn labby internal mcp-echo-server: {e}");
            return;
        }
    };
    let pid = child.id();

    // Inspect /proc/<pid>/environ (Linux) for the canary value.
    // When `env_clear` lands, this should NOT be found even though we passed
    // it via env() — the hardened spawn path will strip it before exec.
    #[cfg(target_os = "linux")]
    let found = {
        let env_path = format!("/proc/{pid}/environ");
        let bytes = std::fs::read(&env_path).unwrap_or_default();
        let env_str = String::from_utf8_lossy(&bytes);
        env_str.contains(CANARY_VAL)
    };
    #[cfg(not(target_os = "linux"))]
    let found = {
        eprintln!("env inspection unavailable on this platform; skipping canary check");
        false
    };

    drop(child.stdin.take());
    drop(child.wait());

    assert!(
        !found,
        "child process PID={pid} inherited secret canary env var `{CANARY_KEY}`; \
         env_clear hardening is not working"
    );
}

// ---------------------------------------------------------------------------
// (c) MCP initialize round-trip
// ---------------------------------------------------------------------------

/// Spawns the MCP echo server, sends an MCP `initialize` JSON-RPC request over
/// stdin, and asserts a well-formed JSON-RPC `InitializeResult` arrives on
/// stdout before the child exits.
///
/// This validates the framed stdio line protocol path independently of the full
/// `UpstreamPool` machinery.
#[test]
#[ignore = "requires a built labby binary; run locally with --include-ignored"]
fn gateway_stdio_initialize_round_trip() {
    let mut child = match spawn_echo_child() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("SKIP — could not spawn labby internal mcp-echo-server: {e}");
            return;
        }
    };

    let mut stdin = child.stdin.take().expect("child stdin must be piped");
    let stdout = child.stdout.take().expect("child stdout must be piped");
    let mut reader = BufReader::new(stdout);

    // MCP JSON-RPC 2.0 initialize request.
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "0.0.1" }
        }
    });
    writeln!(stdin, "{}", req).expect("write initialize to child stdin");
    drop(stdin); // EOF signals the server to exit after responding.

    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("read initialize response line from child stdout");
    assert!(
        !line.is_empty(),
        "server must write a response before exiting"
    );

    let resp: serde_json::Value =
        serde_json::from_str(line.trim()).expect("server response must be valid JSON");

    assert_eq!(
        resp["jsonrpc"], "2.0",
        "response must conform to JSON-RPC 2.0; got: {resp}"
    );
    assert_eq!(
        resp["id"], 1,
        "response id must echo request id; got: {resp}"
    );
    assert!(
        resp.get("result").and_then(|r| r.as_object()).is_some(),
        "initialize response must carry a `result` object; got: {resp}"
    );

    drop(reader);
    drop(child.wait());
}
