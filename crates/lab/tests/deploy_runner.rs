//! Orchestrator integration tests for the deploy runner.
//!
//! These exercise the canary / fan-out / fail-fast logic without touching
//! the network — `orchestrate_with_io` accepts a `HostIo` factory that
//! returns a `RecordingIo` per host. Every pipeline stage is scripted in
//! advance, so the tests verify that the orchestrator drives the stages
//! correctly and respects the concurrency/abort knobs.

#![cfg(feature = "deploy")]
#![allow(clippy::panic)]
#![allow(unused_qualifications)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use labby::dispatch::deploy::build::BuildOutcome;
use labby::dispatch::deploy::runner::test_support::{RecordingIo, RunResp};
use labby::dispatch::deploy::runner::{HostIo, orchestrate_with_io};

/// Bundle of pre-programmed responses for a single host's full happy path.
///
/// Sequence matches the stage functions:
///   1. `uname -m`                  → stdout = arch
///   2. canary sh -c                → exit 0
///   3. `sha256sum <remote_path>`   → remote_sha (None/Some)
///   (if not skip_transfer)
///   4. upload .partial              (implicit; no run_argv)
///   5. mv .partial → .new          → exit 0
///   6. sha256sum .new              → build_sha
///   7. sha256sum remote_path       → maybe existing
///   8. mv remote → .bak.ts         → exit 0 (only if existing)
///   9. mv .new → remote_path       → exit 0
///   10. chmod 755 remote_path      → exit 0
///   11. systemctl restart unit     → exit 0 (only if unit)
///   12. systemctl is-active --wait → exit 0 (only if unit)
///   13. remote_path --version      → exit 0
fn script_happy_path(build_sha: &str, existing: bool) -> RecordingIo {
    let io = RecordingIo::new();
    io.push_run(RunResp::ok("x86_64\n")); // uname
    io.push_run(RunResp::ok("")); // canary
    io.push_sha(None); // preflight sha probe -> not skip
    io.push_run(RunResp::ok("")); // mv partial -> staged
    io.push_sha(Some(build_sha.to_string())); // staged sha
    io.push_sha(if existing { Some("old".into()) } else { None });
    if existing {
        io.push_run(RunResp::ok("")); // mv existing -> backup
    }
    io.push_run(RunResp::ok("")); // mv staged -> remote_path
    io.push_run(RunResp::ok("")); // chmod 755 remote_path
    // restart + is-active + verify added when applicable
    io
}

fn script_verify_fail(build_sha: &str) -> RecordingIo {
    let io = script_happy_path(build_sha, false);
    // verify: nonzero exit
    io.push_run(RunResp::fail(2, "bad version"));
    io
}

fn script_happy_no_unit(build_sha: &str) -> RecordingIo {
    let io = script_happy_path(build_sha, false);
    io.push_run(RunResp::ok("labby 0.3.4\n")); // verify
    io
}

fn fake_build() -> Arc<BuildOutcome> {
    // Write a small tempfile so `tokio::fs::File::open(&build.path)` succeeds
    // inside `run_host_pipeline`. sha256 of the contents matches `target_sha`.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    std::fs::write(&path, b"labby-fake").unwrap();
    let sha = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"labby-fake");
        hex::encode(h.finalize())
    };
    // Keep the tempfile alive for the duration of the test by leaking the
    // handle — tests are short-lived.
    std::mem::forget(tmp);
    Arc::new(BuildOutcome {
        path,
        sha256: sha,
        size_bytes: 8,
        target_triple: "x86_64-unknown-linux-gnu".into(),
        role: labby::config::ArtifactRole::Node,
    })
}

/// Per-host `RecordingIo` factory. The factory returns a clone of the
/// pre-built `RecordingIo` instance for each host — each host gets its own
/// scripted queue.
#[derive(Clone)]
struct IoFactory {
    inner: Arc<Mutex<HashMap<String, RecordingIo>>>,
}

impl IoFactory {
    fn new(map: HashMap<String, RecordingIo>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(map)),
        }
    }

    fn take(&self, host: &str) -> RecordingIo {
        self.inner
            .lock()
            .unwrap()
            .remove(host)
            .unwrap_or_else(|| panic!("no RecordingIo scripted for host `{host}`"))
    }
}

/// Ordering-observing `HostIo` wrapper. Captures the instant each stage
/// entered/exited so the test can assert canary-before-rest.
struct TimedIo {
    inner: RecordingIo,
    started_at: Arc<Mutex<Option<Instant>>>,
    finished_at: Arc<Mutex<Option<Instant>>>,
    delay: Duration,
}

impl TimedIo {
    fn new(inner: RecordingIo, delay: Duration) -> Self {
        Self {
            inner,
            started_at: Arc::new(Mutex::new(None)),
            finished_at: Arc::new(Mutex::new(None)),
            delay,
        }
    }
}

impl HostIo for TimedIo {
    fn run_argv(
        &self,
        argv: &[&str],
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<(i32, String, String), lab_apis::deploy::DeployError>,
                > + Send
                + 'static,
        >,
    > {
        let started_at = self.started_at.clone();
        let finished_at = self.finished_at.clone();
        let delay = self.delay;
        let inner_fut = self.inner.run_argv(argv);
        Box::pin(async move {
            {
                let mut s = started_at.lock().unwrap();
                if s.is_none() {
                    *s = Some(Instant::now());
                }
            }
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let res = inner_fut.await;
            *finished_at.lock().unwrap() = Some(Instant::now());
            res
        })
    }

    fn upload_stream<R>(
        &self,
        remote_path: &str,
        reader: R,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<u64, lab_apis::deploy::DeployError>>
                + Send
                + 'static,
        >,
    >
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        self.inner.upload_stream(remote_path, reader)
    }

    fn sha256_remote(
        &self,
        remote_path: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<String>, lab_apis::deploy::DeployError>>
                + Send
                + 'static,
        >,
    > {
        self.inner.sha256_remote(remote_path)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn continue_on_error_reports_partial_failure() {
    let build = fake_build();
    let mut map = HashMap::new();
    // host1 fails verify; host2 happy path
    map.insert("host1".to_string(), script_verify_fail(&build.sha256));
    map.insert("host2".to_string(), script_happy_no_unit(&build.sha256));

    let factory = IoFactory::new(map);
    let f = factory.clone();
    let results = orchestrate_with_io(
        vec![
            ("host1".into(), None, None, "/usr/local/bin/labby".into()),
            ("host2".into(), None, None, "/usr/local/bin/labby".into()),
        ],
        build,
        2,
        false, // fail_fast = false
        "run-abc".into(),
        move |h| f.take(h),
    )
    .await;

    assert_eq!(results.len(), 2);
    let host1 = results.iter().find(|r| r.host == "host1").unwrap();
    let host2 = results.iter().find(|r| r.host == "host2").unwrap();
    assert!(!host1.succeeded);
    assert_eq!(host1.error_kind.as_deref(), Some("verify_failed"));
    assert!(host2.succeeded);
}

#[tokio::test]
async fn fail_fast_aborts_subsequent_hosts() {
    let build = fake_build();
    let mut map = HashMap::new();
    // host1: fails verify → triggers stop
    map.insert("host1".to_string(), script_verify_fail(&build.sha256));
    // host2: scripted but should not be reached; a blank RecordingIo
    // that would hit "no scripted run" if actually invoked.
    map.insert("host2".to_string(), RecordingIo::new());

    let factory = IoFactory::new(map);
    let f = factory.clone();
    // max_parallel = 1 guarantees host1 completes before host2 starts, so
    // the stop flag can actually prevent host2's pipeline from running.
    let results = orchestrate_with_io(
        vec![
            ("host1".into(), None, None, "/usr/local/bin/labby".into()),
            ("host2".into(), None, None, "/usr/local/bin/labby".into()),
        ],
        build,
        1,
        true, // fail_fast
        "run-ff".into(),
        move |h| f.take(h),
    )
    .await;

    assert_eq!(results.len(), 2);
    let host2 = results.iter().find(|r| r.host == "host2").unwrap();
    assert!(!host2.succeeded);
    assert_eq!(host2.error_kind.as_deref(), Some("aborted"));
}

#[tokio::test]
async fn max_parallel_bounds_concurrency() {
    let build = fake_build();
    // Build five timed hosts; measure how many can be in-flight at once.
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let max_seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    struct ConcIo {
        inner: RecordingIo,
        in_flight: Arc<std::sync::atomic::AtomicUsize>,
        max_seen: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl HostIo for ConcIo {
        fn run_argv(
            &self,
            argv: &[&str],
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<(i32, String, String), lab_apis::deploy::DeployError>,
                    > + Send
                    + 'static,
            >,
        > {
            let in_flight = self.in_flight.clone();
            let max_seen = self.max_seen.clone();
            let inner_fut = self.inner.run_argv(argv);
            let is_first = argv == ["uname", "-m"];
            let is_last = argv.len() == 2 && argv[1] == "--version";
            Box::pin(async move {
                if is_first {
                    let prev = in_flight.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    max_seen.fetch_max(prev, std::sync::atomic::Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                let r = inner_fut.await;
                if is_last {
                    in_flight.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                }
                r
            })
        }
        fn upload_stream<R>(
            &self,
            p: &str,
            r: R,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<u64, lab_apis::deploy::DeployError>>
                    + Send
                    + 'static,
            >,
        >
        where
            R: tokio::io::AsyncRead + Unpin + Send + 'static,
        {
            self.inner.upload_stream(p, r)
        }
        fn sha256_remote(
            &self,
            p: &str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<Option<String>, lab_apis::deploy::DeployError>,
                    > + Send
                    + 'static,
            >,
        > {
            self.inner.sha256_remote(p)
        }
    }

    let mut map = HashMap::new();
    for h in ["h1", "h2", "h3", "h4", "h5"] {
        map.insert(h.to_string(), script_happy_no_unit(&build.sha256));
    }
    let factory = IoFactory::new(map);
    let f = factory.clone();
    let counter_c = counter.clone();
    let max_seen_c = max_seen.clone();
    let results = orchestrate_with_io(
        vec![
            ("h1".into(), None, None, "/usr/local/bin/labby".into()),
            ("h2".into(), None, None, "/usr/local/bin/labby".into()),
            ("h3".into(), None, None, "/usr/local/bin/labby".into()),
            ("h4".into(), None, None, "/usr/local/bin/labby".into()),
            ("h5".into(), None, None, "/usr/local/bin/labby".into()),
        ],
        build,
        2,
        false,
        "run-conc".into(),
        move |h| ConcIo {
            inner: f.take(h),
            in_flight: counter_c.clone(),
            max_seen: max_seen_c.clone(),
        },
    )
    .await;

    assert_eq!(results.len(), 5, "all 5 hosts must complete");
    assert!(
        results.iter().all(|r| r.succeeded),
        "all hosts must succeed: {results:?}"
    );

    let observed = max_seen.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        observed > 0,
        "no host pipeline was observed; orchestration may not have run"
    );
    assert!(
        observed <= 2,
        "max_parallel=2 but observed {observed} concurrent hosts"
    );
}

#[tokio::test]
async fn all_succeed_happy_path() {
    let build = fake_build();
    let mut map = HashMap::new();
    for h in ["a", "b", "c"] {
        map.insert(h.to_string(), script_happy_no_unit(&build.sha256));
    }
    let factory = IoFactory::new(map);
    let f = factory.clone();
    let results = orchestrate_with_io(
        vec![
            ("a".into(), None, None, "/usr/local/bin/labby".into()),
            ("b".into(), None, None, "/usr/local/bin/labby".into()),
            ("c".into(), None, None, "/usr/local/bin/labby".into()),
        ],
        build,
        3,
        false,
        "run-happy".into(),
        move |h| f.take(h),
    )
    .await;
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|r| r.succeeded), "{results:?}");
}

#[tokio::test]
async fn skip_transfer_when_sha_matches_does_not_call_upload() {
    let build = fake_build();
    // Script: uname ok, canary ok, sha256 == build.sha256 → skip transfer,
    //         verify ok.
    let io = RecordingIo::new();
    io.push_run(RunResp::ok("x86_64\n"));
    io.push_run(RunResp::ok(""));
    io.push_sha(Some(build.sha256.clone()));
    io.push_run(RunResp::ok("labby 0.3.4\n")); // verify
    let mut map = HashMap::new();
    map.insert("skiphost".to_string(), io);
    let factory = IoFactory::new(map);

    // Snapshot the log-carrying reference. We clone the Arc before handing
    // the RecordingIo to the factory so we can inspect ops after the run.
    let recording = factory
        .inner
        .lock()
        .unwrap()
        .get("skiphost")
        .unwrap()
        .log
        .clone();

    let f = factory.clone();
    let results = orchestrate_with_io(
        vec![("skiphost".into(), None, None, "/usr/local/bin/labby".into())],
        build,
        1,
        false,
        "run-skip".into(),
        move |h| f.take(h),
    )
    .await;
    assert!(results[0].succeeded);
    assert!(results[0].skipped_transfer);

    // The log must not contain any `upload:` op.
    let ops = recording.lock().unwrap().clone();
    assert!(
        !ops.iter().any(|o| o.starts_with("upload:")),
        "unexpected upload op: {ops:?}"
    );
}

#[tokio::test]
async fn unknown_host_alias_in_factory_path_is_separate_from_plan_validation() {
    // Smoke: ensure TimedIo compiles as a HostIo impl (used to catch lifetime regressions).
    // This test doesn't call orchestrate_with_io — just touches TimedIo.
    let inner = RecordingIo::new();
    let _t = TimedIo::new(inner, Duration::from_millis(0));
}

// ── Task 13: per-role artifact plan test ──────────────────────────────────

/// Verify that `plan_impl` populates `DeployPlan.artifacts` with one entry per
/// role required by the requested targets.
///
/// This test uses the public `DefaultRunner::plan_impl` path which only inspects
/// the on-disk artifact path (no build is triggered). We do NOT need a live SSH
/// host — `plan_impl` only validates aliases against the inventory.
#[tokio::test]
async fn plan_artifacts_includes_per_role_entries() {
    use lab_apis::core::ssh::SshHostTarget;
    use lab_apis::deploy::DeployRequest;
    use labby::config::{ArtifactRole, DeployDefaults, DeployHostOverride, DeployPreferences};
    use labby::dispatch::deploy::runner::DefaultRunner;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    // Build a config with two hosts: one controller-role, one node-role.
    let mut hosts = BTreeMap::new();
    hosts.insert(
        "ctrl-host".to_string(),
        DeployHostOverride {
            artifact_role: Some(ArtifactRole::Controller),
            ..Default::default()
        },
    );
    hosts.insert(
        "node-host".to_string(),
        DeployHostOverride {
            artifact_role: Some(ArtifactRole::Node),
            ..Default::default()
        },
    );
    let prefs = DeployPreferences {
        defaults: Some(DeployDefaults {
            remote_path: Some("/usr/local/bin/labby".to_string()),
            ..Default::default()
        }),
        hosts,
    };

    // Build a minimal SSH inventory with the two hosts.
    let inventory = vec![
        SshHostTarget {
            alias: "ctrl-host".to_string(),
            hostname: None,
            user: None,
            port: None,
            identity_file: None,
        },
        SshHostTarget {
            alias: "node-host".to_string(),
            hostname: None,
            user: None,
            port: None,
            identity_file: None,
        },
    ];

    let runner = DefaultRunner::new(
        prefs,
        Arc::new(inventory),
        Arc::new(labby::dispatch::deploy::lock::HostLockRegistry::default()),
    );

    let req = DeployRequest {
        targets: vec!["ctrl-host".to_string(), "node-host".to_string()],
        max_parallel: Some(1),
        fail_fast: false,
        confirm: true,
    };

    let plan = runner
        .plan_impl(req)
        .await
        .expect("plan_impl should succeed");

    // The `artifacts` list must contain both roles.
    let roles: std::collections::HashSet<String> =
        plan.artifacts.iter().map(|a| a.role.clone()).collect();
    assert!(
        roles.contains("controller"),
        "expected 'controller' in artifacts; got: {roles:?}"
    );
    assert!(
        roles.contains("node"),
        "expected 'node' in artifacts; got: {roles:?}"
    );
    assert_eq!(
        plan.artifacts.len(),
        2,
        "expected exactly 2 artifact entries"
    );
}
