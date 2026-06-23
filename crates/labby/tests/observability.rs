//! Observability regression test for the HTTP API dispatch surface.
//!
//! Closes CICD-H1: the shared API dispatch wrapper
//! (`api::services::helpers::handle_action`) must emit structured dispatch
//! events that carry the fields mandated by `docs/dev/OBSERVABILITY.md`:
//! `surface = "api"`, `service`, `action`, `elapsed_ms` on success, plus a
//! stable `kind` on failure.
//!
//! This test exercises the *real* code path — `handle_action` is the single
//! enforcement point every `/v1/<service>` handler calls — so a regression that
//! drops `surface` or the standard field set from the dispatch event fails here.
//!
//! Log capture uses a minimal in-process `tracing-subscriber` `fmt` layer
//! writing JSON into an `Arc<Mutex<Vec<u8>>>` via a custom `MakeWriter`. No new
//! dependency is introduced — `tracing-subscriber` is already a dependency of
//! the crate under test.

use std::io;
use std::sync::{Arc, Mutex};

use labby_apis::core::action::{ActionSpec, ParamSpec};
use labby::api::ActionRequest;
use labby::api::services::helpers::handle_action;
use labby::dispatch::error::ToolError;
use serde_json::{Value, json};
use tracing_subscriber::fmt;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::{EnvFilter, prelude::*};

/// In-memory writer collecting tracing output for assertion.
#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    fn contents(&self) -> String {
        String::from_utf8(self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()).unwrap()
    }
}

impl<'a> MakeWriter<'a> for SharedBuf {
    type Writer = SharedWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedWriter(self.0.clone())
    }
}

struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl io::Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

const ACTIONS: &[ActionSpec] = &[ActionSpec {
    name: "thing.read",
    description: "A non-destructive read action",
    destructive: false,
    requires_admin: false,
    returns: "Value",
    params: &[ParamSpec {
        name: "id",
        ty: "string",
        required: true,
        description: "Resource ID",
    }],
}];

fn make_req(action: &str, params: Value) -> ActionRequest {
    ActionRequest {
        action: action.to_string(),
        params,
    }
}

async fn ok_dispatch(_action: String, _params: Value) -> Result<Value, ToolError> {
    Ok(json!({ "result": "success" }))
}

async fn err_dispatch(_action: String, _params: Value) -> Result<Value, ToolError> {
    Err(ToolError::MissingParam {
        message: "missing required parameter `id`".into(),
        param: "id".into(),
    })
}

/// Run `body` with a JSON-emitting subscriber capturing into `buf`.
///
/// `set_default` installs a thread-local subscriber for the duration of the
/// closure, so the captured output is scoped to this test even under nextest's
/// parallel execution.
fn with_captured_logs<T>(buf: &SharedBuf, body: impl FnOnce() -> T) -> T {
    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new("labby=info"))
        .with(
            fmt::layer()
                .json()
                .with_writer(buf.clone())
                .with_ansi(false)
                .without_time(),
        );
    let _guard = tracing::subscriber::set_default(subscriber);
    body()
}

/// (a) A successful API dispatch emits an event carrying `surface="api"`,
/// `service`, `action`, and `elapsed_ms`.
#[test]
fn api_dispatch_success_emits_standard_fields() {
    let buf = SharedBuf::default();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    with_captured_logs(&buf, || {
        rt.block_on(async {
            let req = make_req("thing.read", json!({ "id": "abc" }));
            let response =
                handle_action("widgets", "api", Some("req-obs-1"), req, ACTIONS, |a, p| {
                    ok_dispatch(a, p)
                })
                .await
                .expect("successful dispatch");
            drop(response);
        });
    });

    let logs = buf.contents();
    assert!(
        logs.contains("\"surface\":\"api\""),
        "success dispatch event must carry surface=\"api\"; got: {logs}"
    );
    assert!(
        logs.contains("\"service\":\"widgets\""),
        "success dispatch event must carry service; got: {logs}"
    );
    assert!(
        logs.contains("\"action\":\"thing.read\""),
        "success dispatch event must carry action; got: {logs}"
    );
    assert!(
        logs.contains("\"elapsed_ms\""),
        "success dispatch event must carry elapsed_ms; got: {logs}"
    );
    assert!(
        logs.contains("\"request_id\":\"req-obs-1\""),
        "success dispatch event must carry request_id when present; got: {logs}"
    );
}

/// (b) A failing API dispatch carries a stable `kind` (alongside the standard
/// `surface="api"` field set).
#[test]
fn api_dispatch_failure_emits_stable_kind() {
    let buf = SharedBuf::default();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let err = with_captured_logs(&buf, || {
        rt.block_on(async {
            let req = make_req("thing.read", json!({}));
            handle_action("widgets", "api", Some("req-obs-2"), req, ACTIONS, |a, p| {
                err_dispatch(a, p)
            })
            .await
            .expect_err("dispatch closure returns an error")
        })
    });

    // The returned error exposes the stable kind to callers...
    assert_eq!(err.0.kind(), "missing_param");

    // ...and the dispatch failure event records it for log-based correlation.
    let logs = buf.contents();
    assert!(
        logs.contains("\"surface\":\"api\""),
        "failure dispatch event must carry surface=\"api\"; got: {logs}"
    );
    assert!(
        logs.contains("\"kind\":\"missing_param\""),
        "failure dispatch event must carry a stable kind; got: {logs}"
    );
    assert!(
        logs.contains("\"elapsed_ms\""),
        "failure dispatch event must carry elapsed_ms; got: {logs}"
    );
}
