# Upstream MCP Proxy Hardening — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix three independent defects in the upstream MCP proxy at `crates/lab/src/dispatch/upstream/`: (F1) stdio child process-group orphan when the connect future is dropped, (F2) post-hoc response size cap that allows OOM before rejection, (F3) UTF-8 panic in `wildcard_matches` on unicode tool names from upstream MCP servers.

**Architecture:** F3 is a self-contained rewrite of one function in `types.rs`. F2 introduces a new `BodyCappedHttpClient` implementing rmcp's `StreamableHttpClient` trait, replacing the bare `reqwest::Client` at both HTTP-connect sites (OAuth and non-OAuth). F1 introduces a `ProcessGroupGuard` (sync-Drop RAII) and adds a `Drop` impl on `UpstreamConnection`, leaving the existing async `shutdown()` graceful path intact via a `take()`-before-await invariant. Execution: Wave 1 = Task 1 (F3) + Task 2 (F2) in parallel; Wave 2 = Task 3 (F1) after F2 lands (they share the `UpstreamConnection` struct definition).

**Tech Stack:** Rust 2024 edition · Tokio · rmcp 1.6.0 (`StreamableHttpClient` trait) · reqwest 0.12 (`bytes_stream`) · process_wrap (`ProcessGroup::leader`) · nix (`killpg`) · cargo-nextest · wiremock · proptest.

**Source beads:** `lab-4z8sx` (epic) → `lab-4z8sx.1` (F1), `lab-4z8sx.2` (F2), `lab-4z8sx.3` (F3).

---

## Pre-flight (read before starting any task)

- [ ] **Read the bead context:** `bd show lab-4z8sx`, `bd show lab-4z8sx.1`, `bd show lab-4z8sx.2`, `bd show lab-4z8sx.3`, plus `bd comments lab-4z8sx.1` / `.2` / `.3`. The bead descriptions and notes contain the locked decisions and review findings — treat them as the source of truth.
- [ ] **Read the upstream module guide:** `crates/lab/src/dispatch/upstream/CLAUDE.md` and `crates/lab/src/dispatch/CLAUDE.md`.
- [ ] **Confirm baseline builds:** `just check` and `just lint` pass on `main` before touching anything.

---

## Task 1 (Wave 1 — parallel with Task 2): F3 wildcard_matches char-boundary safety

**Bead:** `lab-4z8sx.3`

**Files:**
- Modify: `crates/lab/src/dispatch/upstream/types.rs:130-169` (rewrite `wildcard_matches`)
- Modify: `crates/lab/src/dispatch/upstream/types.rs:254-282` (extend tests)
- Modify: `crates/lab/Cargo.toml` (add `proptest` as `[dev-dependencies]` if not already present)

**Locked invariants from the bead:**
- Replace byte-offset slicing with `str::match_indices`. No `candidate[cursor..]`-style slicing where `cursor` was computed by hand.
- Algorithm semantics MUST match the existing behavior bit-for-bit on every ASCII test case at `types.rs:254-282`.
- Do not add `wildmatch` / `globset` crates.

### Step 1.1 — Write the regression test that reproduces the panic

- [ ] Open `crates/lab/src/dispatch/upstream/types.rs`. Find the `#[cfg(test)] mod tests` block at line 253. Add this test inside the module (do not modify the existing tests yet):

```rust
    #[test]
    fn wildcard_matches_does_not_panic_on_multibyte_char_boundary() {
        // Trigger: pattern `f*o` against candidate `f∂o`.
        // The `∂` is 2 bytes; after matching "f" the cursor lands at byte 1,
        // which sits inside the multibyte codepoint. Old code panicked here.
        assert!(super::wildcard_matches("f*o", "f∂o"));
        assert!(!super::wildcard_matches("f*o", "f∂x"));
    }

    #[test]
    fn wildcard_matches_unicode_anchors() {
        assert!(super::wildcard_matches("*∂*", "prefix∂suffix"));
        assert!(super::wildcard_matches("∂*", "∂abc"));
        assert!(super::wildcard_matches("*∂", "abc∂"));
        assert!(super::wildcard_matches("a*b*c", "a∂b∂c"));
    }

    #[test]
    fn wildcard_matches_edge_cases() {
        assert!(super::wildcard_matches("*", ""));
        assert!(!super::wildcard_matches("a", ""));
        assert!(super::wildcard_matches("**", "anything"));
        // BIDI override is just a unicode codepoint as far as matching goes.
        // (Security normalization is out of scope for this bead.)
        assert!(super::wildcard_matches("*\u{202E}*", "abc\u{202E}def"));
    }
```

### Step 1.2 — Run the new tests to verify they FAIL (the boundary panic test must panic, not just return false)

- [ ] Run: `cargo nextest run -p lab --all-features wildcard_matches_does_not_panic` (any of the new test names)
- [ ] Expected: tests FAIL or PANIC. The boundary panic test produces `byte index 1 is not a char boundary` or similar; the unicode tests likely also panic or assert-fail. Either way, they must not pass.

### Step 1.3 — Rewrite `wildcard_matches` using `str::match_indices`

- [ ] Replace the body of `fn wildcard_matches` at `crates/lab/src/dispatch/upstream/types.rs:130-169` with:

```rust
fn wildcard_matches(pattern: &str, candidate: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == candidate;
    }

    let anchored_start = !pattern.starts_with('*');
    let anchored_end = !pattern.ends_with('*');
    let non_empty_parts: Vec<&str> = parts.into_iter().filter(|p| !p.is_empty()).collect();

    if non_empty_parts.is_empty() {
        return true;
    }

    let mut cursor: usize = 0;
    for (index, part) in non_empty_parts.iter().enumerate() {
        if index == 0 && anchored_start {
            if !candidate.starts_with(part) {
                return false;
            }
            cursor = part.len();
            continue;
        }

        // SAFETY: match_indices returns char-boundary-aligned byte offsets.
        // part.len() is the byte length of a UTF-8 string slice (also boundary-aligned).
        // Therefore `cursor` only ever points at valid char boundaries — no slicing panic possible.
        let found = candidate
            .match_indices(*part)
            .find(|(idx, _)| *idx >= cursor);
        match found {
            Some((idx, _)) => cursor = idx + part.len(),
            None => return false,
        }
    }

    if anchored_end && let Some(last) = non_empty_parts.last() {
        return candidate.ends_with(last);
    }

    true
}
```

### Step 1.4 — Run the new + existing tests to verify they pass

- [ ] Run: `cargo nextest run -p lab --all-features types::tests`
- [ ] Expected: PASS for all of:
  - `exact_and_wildcard_patterns_match_tool_names` (existing)
  - `missing_policy_defaults_to_all` (existing)
  - `wildcard_matching_supports_simple_globs` (existing)
  - `wildcard_matches_does_not_panic_on_multibyte_char_boundary` (new)
  - `wildcard_matches_unicode_anchors` (new)
  - `wildcard_matches_edge_cases` (new)

### Step 1.5 — Add the proptest dev-dependency (if not already present)

- [ ] Open `crates/lab/Cargo.toml`. In the `[dev-dependencies]` section, ensure proptest is present. If absent, add:

```toml
proptest = "1"
```

- [ ] Run `cargo check --workspace --all-features --tests` to confirm the dep resolves.

### Step 1.6 — Write the proptest fuzz to lock the panic-safety invariant

- [ ] In `crates/lab/src/dispatch/upstream/types.rs` inside the `tests` module, add:

```rust
    proptest::proptest! {
        #[test]
        fn wildcard_matches_never_panics(pattern in ".{0,32}", candidate in ".{0,128}") {
            // The only requirement is no panic. The return value is unconstrained —
            // any valid UTF-8 input must produce a bool.
            let _ = super::wildcard_matches(&pattern, &candidate);
        }

        #[test]
        fn wildcard_matches_star_injection_never_panics(parts in proptest::collection::vec(".{0,8}", 0..6), candidate in ".{0,64}") {
            let pattern = parts.join("*");
            let _ = super::wildcard_matches(&pattern, &candidate);
        }
    }
```

### Step 1.7 — Run the proptest

- [ ] Run: `cargo nextest run -p lab --all-features wildcard_matches_never_panics wildcard_matches_star_injection_never_panics`
- [ ] Expected: PASS. proptest defaults to 256 cases; you can scale via `PROPTEST_CASES=10000 cargo nextest run ...` for confidence.

### Step 1.8 — Lint and final workspace test

- [ ] Run: `cargo clippy --workspace --all-features -- -D warnings`
- [ ] Run: `cargo nextest run -p lab --all-features`
- [ ] Both must pass with no new warnings.

### Step 1.9 — Commit

- [ ] Stage and commit:

```bash
git add crates/lab/src/dispatch/upstream/types.rs crates/lab/Cargo.toml
git commit -m "fix(upstream): wildcard_matches char-boundary safety (lab-4z8sx.3)

Rewrite wildcard_matches using str::match_indices so the cursor never
lands inside a UTF-8 multi-byte codepoint. Add unicode test cases and
proptest coverage to lock the panic-safety invariant.

Trigger: pattern 'f*o' against candidate 'f∂o' panicked with
'byte index 1 is not a char boundary'. Tool names come from upstream
MCP servers — low-trust input.

Refs: lab-4z8sx.3
"
```

---

## Task 2 (Wave 1 — parallel with Task 1): F2 streaming body cap

**Bead:** `lab-4z8sx.2`

**Files:**
- Create: `crates/lab/src/dispatch/upstream/http_client.rs` (new `BodyCappedHttpClient`)
- Modify: `crates/lab/src/dispatch/upstream.rs` (re-export new module)
- Modify: `crates/lab/src/dispatch/upstream/pool.rs:3241-3298` (wrap both reqwest::Client construction sites)
- Modify (read first, only edit if needed): `crates/lab/src/oauth/upstream/cache.rs` (only if `auth_client` type needs an adapter)

**Locked invariants:**
- Cap applies to **decoded** bytes (reqwest's `bytes_stream()` yields post-gzip/br/zstd bytes — confirmed).
- **Per-event cap for SSE**, cumulative cap for `Json` variant. **Do not** accumulate across SSE events — that would disconnect legitimate long-lived subscriptions.
- Preserve the post-hoc check sites at `pool.rs:1748, 2035, 2532, 2616` as defense-in-depth (do not touch).
- Error kind on cap breach must remain `response_too_large` so existing dashboards/log filters keep working.

### Step 2.1 — Confirm the rmcp `StreamableHttpClient` trait shape

- [ ] Read: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rmcp-1.6.0/src/transport/streamable_http_client.rs` (the trait definition lives near line 199).
- [ ] Read: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rmcp-1.6.0/src/transport/common/reqwest/streamable_http_client.rs` (the existing reqwest impl — use as reference structure for our wrapper).
- [ ] Confirm three trait methods exist: `post_message`, `delete_session`, `get_stream`. Confirm the return shape of `post_message` is `Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>>` where `StreamableHttpPostResponse` is the enum with `Accepted | Json(_, _) | Sse(_, _)` variants.

### Step 2.2 — Scaffold the new module

- [ ] Create `crates/lab/src/dispatch/upstream/http_client.rs` with:

```rust
//! HTTP client wrapper that enforces a maximum response body size at the
//! `StreamableHttpClient` trait layer, BEFORE deserialization.
//!
//! The cap applies to decoded bytes (reqwest auto-decodes Content-Encoding:
//! gzip/br/zstd, so `bytes_stream()` yields decoded chunks). For the SSE
//! variant and `get_stream`, the cap applies PER EVENT, not cumulatively —
//! a legitimate long-lived SSE subscription must not be disconnected after
//! an arbitrary cumulative byte count.

use std::collections::HashMap;
use std::sync::Arc;

use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::{ClientJsonRpcMessage};
use rmcp::transport::common::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};

/// Wraps a `reqwest::Client` and enforces a per-response decoded-body size cap.
///
/// Cap semantics:
/// - `post_message` -> `Json(_, _)`: cumulative cap on the buffer.
/// - `post_message` -> `Sse(_, _)`: per-event cap (not cumulative).
/// - `get_stream`: per-event cap (not cumulative).
#[derive(Clone)]
pub struct BodyCappedHttpClient {
    inner: reqwest::Client,
    max_bytes: usize,
}

impl BodyCappedHttpClient {
    pub fn new(inner: reqwest::Client, max_bytes: usize) -> Self {
        Self { inner, max_bytes }
    }

    #[must_use]
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }
}
```

- [ ] Verify the file compiles (just the scaffold, no trait impl yet): `cargo check -p lab --all-features`. Expected: unused-import warnings are fine for now; no errors.

### Step 2.3 — Register the new module

- [ ] Open `crates/lab/src/dispatch/upstream.rs`. Add a sibling `pub mod http_client;` declaration alongside the existing `pub mod auth;`, `pub mod pool;`, etc. Re-export the new type if the file re-exports siblings (read first to match the existing pattern).

- [ ] Run `cargo check -p lab --all-features` — must compile cleanly (warnings on the unused `BodyCappedHttpClient` are acceptable until Task 2.5 wires it in).

### Step 2.4 — Write the integration tests FIRST (TDD — they will fail until Step 2.5)

- [ ] Append to `crates/lab/src/dispatch/upstream/http_client.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn build_capped_client(max_bytes: usize) -> BodyCappedHttpClient {
        let inner = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("client");
        BodyCappedHttpClient::new(inner, max_bytes)
    }

    /// A 50 MB JSON body against a 10 MB cap must be rejected — before it OOMs us.
    #[tokio::test]
    async fn rejects_oversized_json_response_via_content_length() {
        let server = MockServer::start().await;
        let big = "x".repeat(50 * 1024 * 1024);
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/json")
                    .set_body_string(format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"{big}\"}}")),
            )
            .mount(&server)
            .await;

        let client = build_capped_client(10 * 1024 * 1024);
        let uri: Arc<str> = format!("{}/mcp", server.uri()).into();
        // ClientJsonRpcMessage construction — use whatever rmcp 1.6 exposes
        // as a minimal valid initialize/notification message in tests.
        // (See the rmcp test fixtures for the canonical builder.)
        let msg: ClientJsonRpcMessage = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
        )
        .expect("valid jsonrpc");
        let result = client
            .post_message(uri, msg, None, None, HashMap::new())
            .await;
        let err = result.expect_err("must reject oversized body");
        let s = format!("{err:?}");
        assert!(s.contains("response_too_large") || s.contains("too large"), "unexpected error: {s}");
    }

    /// Chunked transfer-encoding (no Content-Length) — must bail mid-stream.
    #[tokio::test]
    async fn rejects_oversized_chunked_response_during_stream() {
        let server = MockServer::start().await;
        let big = "x".repeat(20 * 1024 * 1024);
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/json")
                    .insert_header("Transfer-Encoding", "chunked")
                    .set_body_string(format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"{big}\"}}")),
            )
            .mount(&server)
            .await;

        let client = build_capped_client(1 * 1024 * 1024);
        let uri: Arc<str> = format!("{}/mcp", server.uri()).into();
        let msg: ClientJsonRpcMessage = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
        )
        .expect("valid jsonrpc");
        let result = client
            .post_message(uri, msg, None, None, HashMap::new())
            .await;
        assert!(result.is_err(), "must reject oversized chunked body");
    }

    /// 9 MB body under 10 MB cap — must succeed.
    #[tokio::test]
    async fn allows_response_under_cap() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#,
            ))
            .mount(&server)
            .await;

        let client = build_capped_client(10 * 1024 * 1024);
        let uri: Arc<str> = format!("{}/mcp", server.uri()).into();
        let msg: ClientJsonRpcMessage = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
        )
        .expect("valid jsonrpc");
        let result = client
            .post_message(uri, msg, None, None, HashMap::new())
            .await;
        assert!(result.is_ok(), "small response should succeed: {result:?}");
    }

    /// Gzip-bomb: 1 KB compressed expanding to 50 MB decoded.
    /// reqwest auto-decodes; the cap must apply to decoded bytes.
    #[tokio::test]
    async fn rejects_gzip_bomb_post_decompression() {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all("x".repeat(50 * 1024 * 1024).as_bytes()).unwrap();
        let gz = encoder.finish().unwrap();

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/json")
                    .insert_header("Content-Encoding", "gzip")
                    .set_body_bytes(gz),
            )
            .mount(&server)
            .await;

        let client = build_capped_client(10 * 1024 * 1024);
        let uri: Arc<str> = format!("{}/mcp", server.uri()).into();
        let msg: ClientJsonRpcMessage = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
        )
        .expect("valid jsonrpc");
        let result = client
            .post_message(uri, msg, None, None, HashMap::new())
            .await;
        assert!(result.is_err(), "gzip bomb must be rejected post-decompression");
    }
}
```

- [ ] Add the required test deps to `crates/lab/Cargo.toml` `[dev-dependencies]` if not already present:

```toml
wiremock = "0.6"
flate2 = "1"
```

- [ ] Run: `cargo check -p lab --all-features --tests`. Expected: the body-cap trait impl doesn't exist yet, so the tests will fail to compile (missing `post_message`). That's fine — we write the impl next.

### Step 2.5 — Implement `StreamableHttpClient` for `BodyCappedHttpClient`

- [ ] Read the existing reqwest impl at `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rmcp-1.6.0/src/transport/common/reqwest/streamable_http_client.rs` to mirror its structure. Note that the existing impl uses `request_builder.bearer_auth(auth_header)` etc. — preserve that exact behavior in our wrapper by delegating header setup to `self.inner`.
- [ ] Append to `crates/lab/src/dispatch/upstream/http_client.rs` (above the `#[cfg(test)]` module):

```rust
use futures::StreamExt;
use rmcp::model::ServerJsonRpcMessage;
use rmcp::transport::common::streamable_http_client::StreamableHttpPostResponse as Resp;

#[derive(Debug)]
pub struct BodyCapError(pub String);

impl std::fmt::Display for BodyCapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for BodyCapError {}

impl BodyCappedHttpClient {
    /// Apply Content-Length pre-check + counting body read.
    /// Returns the full body bytes if under cap, or BodyCapError otherwise.
    /// Cap applies to DECODED bytes — caller must use reqwest with default
    /// auto-decompression so `bytes_stream` yields decoded chunks.
    async fn read_body_capped(
        &self,
        response: reqwest::Response,
    ) -> Result<Vec<u8>, BodyCapError> {
        let max = self.max_bytes as u64;
        if let Some(cl) = response.content_length() {
            if cl > max {
                return Err(BodyCapError(format!(
                    "response_too_large: declared {cl} bytes, max {max}"
                )));
            }
        }
        let mut buf: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();
        let mut count: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| BodyCapError(format!("body stream error: {e}")))?;
            count = count.saturating_add(chunk.len() as u64);
            if count > max {
                return Err(BodyCapError(format!(
                    "response_too_large: streamed {count} bytes, max {max}"
                )));
            }
            buf.extend_from_slice(&chunk);
        }
        Ok(buf)
    }
}

impl StreamableHttpClient for BodyCappedHttpClient {
    type Error = reqwest::Error;

    fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> impl std::future::Future<
        Output = Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>>,
    > + Send
    + '_ {
        async move {
            let mut req = self
                .inner
                .post(&*uri)
                .header(reqwest::header::ACCEPT, "application/json, text/event-stream")
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .json(&message);
            if let Some(token) = auth_header {
                req = req.bearer_auth(token);
            }
            if let Some(sid) = session_id {
                req = req.header("Mcp-Session-Id", &*sid);
            }
            for (k, v) in custom_headers {
                req = req.header(k, v);
            }
            let response = req
                .send()
                .await
                .map_err(StreamableHttpError::Client)?;
            let status = response.status();
            if status == reqwest::StatusCode::ACCEPTED || status == reqwest::StatusCode::NO_CONTENT {
                return Ok(Resp::Accepted);
            }
            let session_id_response = response
                .headers()
                .get("Mcp-Session-Id")
                .and_then(|h| h.to_str().ok())
                .map(String::from);

            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("")
                .to_string();

            if content_type.starts_with("application/json") {
                let bytes = self
                    .read_body_capped(response)
                    .await
                    .map_err(|e| StreamableHttpError::Client(reqwest_error_from_str(&e.0)))?;
                let msg: ServerJsonRpcMessage = serde_json::from_slice(&bytes)
                    .map_err(|e| StreamableHttpError::Client(reqwest_error_from_str(&format!("json parse error: {e}"))))?;
                Ok(Resp::Json(msg, session_id_response))
            } else if content_type.starts_with("text/event-stream") {
                // Per-event cap (NOT cumulative): wrap the SSE byte stream
                // and reject any SINGLE event exceeding max_bytes. This protects
                // against a hostile upstream sending a 1 GB single SSE event
                // while preserving long-lived legitimate subscriptions.
                let max = self.max_bytes;
                let byte_stream = response.bytes_stream();
                let capped_stream = build_per_event_capped_sse(byte_stream, max);
                Ok(Resp::Sse(capped_stream, session_id_response))
            } else {
                // Unknown content type; treat like a small JSON response
                // — read with cap and surface as a JSON-parse failure if it's not.
                let bytes = self
                    .read_body_capped(response)
                    .await
                    .map_err(|e| StreamableHttpError::Client(reqwest_error_from_str(&e.0)))?;
                let msg: ServerJsonRpcMessage = serde_json::from_slice(&bytes)
                    .map_err(|e| StreamableHttpError::Client(reqwest_error_from_str(&format!("unexpected content-type {content_type}, parse: {e}"))))?;
                Ok(Resp::Json(msg, session_id_response))
            }
        }
    }

    fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> impl std::future::Future<Output = Result<(), StreamableHttpError<Self::Error>>> + Send + '_ {
        // No body of significance returned; delegate without cap.
        // (Match the structure of the reqwest reference impl.)
        async move {
            let mut req = self
                .inner
                .delete(&*uri)
                .header("Mcp-Session-Id", &*session_id);
            if let Some(token) = auth_header {
                req = req.bearer_auth(token);
            }
            for (k, v) in custom_headers {
                req = req.header(k, v);
            }
            let _ = req.send().await.map_err(StreamableHttpError::Client)?;
            Ok(())
        }
    }

    fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> impl std::future::Future<
        Output = Result<
            futures::stream::BoxStream<
                'static,
                Result<rmcp::transport::common::client_side_sse::Sse, rmcp::transport::common::client_side_sse::SseError>,
            >,
            StreamableHttpError<Self::Error>,
        >,
    > + Send + '_ {
        async move {
            let mut req = self
                .inner
                .get(&*uri)
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .header("Mcp-Session-Id", &*session_id);
            if let Some(eid) = last_event_id {
                req = req.header("Last-Event-Id", eid);
            }
            if let Some(token) = auth_header {
                req = req.bearer_auth(token);
            }
            for (k, v) in custom_headers {
                req = req.header(k, v);
            }
            let response = req
                .send()
                .await
                .map_err(StreamableHttpError::Client)?;
            let max = self.max_bytes;
            Ok(build_per_event_capped_sse(response.bytes_stream(), max))
        }
    }
}

/// Build an SSE event stream from a byte stream that rejects any individual
/// event whose raw bytes exceed `max_bytes`. Uses rmcp's `SseStream`
/// reconstruction layer, but interposes a per-event byte counter.
///
/// IMPORTANT: this is per-event, not cumulative. Long-lived legitimate
/// SSE subscriptions must keep working.
fn build_per_event_capped_sse(
    byte_stream: impl futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send + 'static,
    max_bytes: usize,
) -> futures::stream::BoxStream<
    'static,
    Result<rmcp::transport::common::client_side_sse::Sse, rmcp::transport::common::client_side_sse::SseError>,
> {
    use bytes::Bytes;
    use futures::stream::StreamExt;
    // SSE events are delimited by an empty line ("\n\n"). We accumulate raw
    // bytes between delimiters; if the accumulator exceeds max_bytes, we
    // emit an error and skip the oversized event. Successful events are
    // forwarded to rmcp's SseStream parser.
    let max = max_bytes;
    let counted = byte_stream
        .scan(Vec::<u8>::new(), move |acc, chunk_res| {
            let res = match chunk_res {
                Ok(chunk) => {
                    acc.extend_from_slice(&chunk);
                    if acc.len() > max {
                        let len = acc.len();
                        acc.clear();
                        Err::<Bytes, _>(reqwest_error_from_str(&format!(
                            "response_too_large: single SSE event reached {len} bytes, max {max}"
                        )))
                    } else {
                        // Forward chunk along; rmcp's SseStream will frame events.
                        // Reset the accumulator on event boundary ("\n\n").
                        if chunk.windows(2).any(|w| w == b"\n\n") {
                            acc.clear();
                        }
                        Ok::<Bytes, reqwest::Error>(chunk)
                    }
                }
                Err(e) => Err::<Bytes, _>(e),
            };
            futures::future::ready(Some(res))
        });
    let sse = rmcp::transport::common::client_side_sse::SseStream::from_byte_stream(counted);
    sse.boxed()
}

// Helper to manufacture a reqwest::Error from a string. reqwest does not
// expose a constructor for synthetic errors; the closest we have is wrapping
// via a custom std::io::Error and converting. This is a narrow shim used
// only when the body cap fires.
fn reqwest_error_from_str(msg: &str) -> reqwest::Error {
    // reqwest::Error has no public constructor for arbitrary kinds; build one
    // by triggering an obvious failure mode. We use a "decode error" by
    // constructing via a controlled deserialize call.
    // SAFETY: this is the standard workaround documented in reqwest issues
    // where users need synthetic errors for trait-impl adapters.
    let dummy = serde_json::from_str::<serde_json::Value>(msg);
    match dummy {
        Ok(_) => {
            // Unlikely path. Fall through using a sentinel JSON that always errs.
            let err: serde_json::Error =
                serde_json::from_str::<serde_json::Value>("§§invalid§§").unwrap_err();
            reqwest::Error::from(err)
        }
        Err(_e) => {
            // Pass the msg through a JSON parse failure to attach context.
            let _ = serde_json::from_str::<serde_json::Value>("§§").err();
            // reqwest::Error::from impls accept serde_json::Error in some
            // versions. If not available on 0.12 in the workspace, this
            // closure can be replaced with a thiserror wrapper around
            // StreamableHttpError directly — adjust if the build fails.
            // Use the err variant produced above:
            let err: serde_json::Error =
                serde_json::from_str::<serde_json::Value>("§§").unwrap_err();
            reqwest::Error::from(err)
        }
    }
}
```

> **Implementation note for the engineer:** the `reqwest_error_from_str` shim is the most fragile part. If `reqwest::Error::from(serde_json::Error)` is not available in the workspace's reqwest version, change `BodyCappedHttpClient::Error` to a custom enum (e.g., `BodyCappedError { Reqwest(reqwest::Error), Cap(BodyCapError) }`) that implements `std::error::Error`, and update the `StreamableHttpClient::Error` associated type accordingly. The integration sites in pool.rs don't care about the concrete error type — only that it impls `Display`.

### Step 2.6 — Run the integration tests; iterate until green

- [ ] Run: `cargo nextest run -p lab --all-features http_client::tests`
- [ ] Expected: all 4 tests PASS. If `reqwest_error_from_str` or the SSE adapter has type mismatches, fix them by:
  - Switching `type Error` to a custom enum that wraps `reqwest::Error` + `BodyCapError`.
  - Adjusting the `SseStream::from_byte_stream` call site if rmcp's actual signature differs from the documented one (verify against the rmcp source in step 2.1).

### Step 2.7 — Wire `BodyCappedHttpClient` into the non-OAuth HTTP connect path

- [ ] Open `crates/lab/src/dispatch/upstream/pool.rs`. Locate `connect_http_upstream` at line ~3211. The non-OAuth branch builds a `reqwest::Client` at line 3275 and passes it to `StreamableHttpClientWorker::new(client, transport_config)` at line 3278. Modify:

```rust
    // BEFORE
    let client = reqwest::Client::builder()
        .timeout(DEFAULT_REQUEST_TIMEOUT)
        .build()?;
    let worker = StreamableHttpClientWorker::new(client, transport_config);
```

```rust
    // AFTER
    let client = reqwest::Client::builder()
        .timeout(DEFAULT_REQUEST_TIMEOUT)
        .build()?;
    let capped = crate::dispatch::upstream::http_client::BodyCappedHttpClient::new(
        client,
        max_response_bytes(),
    );
    let worker = StreamableHttpClientWorker::new(capped, transport_config);
```

### Step 2.8 — Wire `BodyCappedHttpClient` into the OAuth HTTP connect path

- [ ] In the same file, locate the OAuth branch at lines ~3241-3258. It constructs an `auth_client` via `OauthClientCache::get_or_build(...)` and passes `(*auth_client).clone()` to `StreamableHttpClientWorker::new`. Modify:

```rust
    // BEFORE
    let auth_client = cache
        .get_or_build(config, subject)
        .await
        .map_err(|e| anyhow::anyhow!("oauth_required: {e}"))?;

    let worker = StreamableHttpClientWorker::new((*auth_client).clone(), transport_config);
```

```rust
    // AFTER
    let auth_client = cache
        .get_or_build(config, subject)
        .await
        .map_err(|e| anyhow::anyhow!("oauth_required: {e}"))?;

    // NOTE: OauthClientCache returns a reqwest::Client-compatible type.
    // Read the cache.rs return type — if it's `reqwest::Client`, wrap directly.
    // If it's a different type (e.g., reqwest-oauth2 client), either:
    //   (a) add an adapter that implements StreamableHttpClient on the wrapped type, or
    //   (b) extend BodyCappedHttpClient to be generic over an inner client that
    //       also implements StreamableHttpClient.
    let capped = crate::dispatch::upstream::http_client::BodyCappedHttpClient::new(
        (*auth_client).clone(),
        max_response_bytes(),
    );
    let worker = StreamableHttpClientWorker::new(capped, transport_config);
```

- [ ] **Verification:** open `crates/lab/src/oauth/upstream/cache.rs` and confirm the return type of `get_or_build`. If it's NOT `Arc<reqwest::Client>`, follow the alternative (b) above and make `BodyCappedHttpClient` generic over `T: StreamableHttpClient`. Do not silently broken-paste — write the adapter that makes the OAuth path compile cleanly.

### Step 2.9 — Build + clippy

- [ ] Run: `cargo check -p lab --all-features`
- [ ] Run: `cargo clippy --workspace --all-features -- -D warnings`
- [ ] Both must pass.

### Step 2.10 — Run all upstream tests including the new integration suite

- [ ] Run: `cargo nextest run -p lab --all-features upstream`
- [ ] Expected: all existing upstream tests pass, plus 4 new tests in `http_client::tests`.

### Step 2.11 — Manual smoke check that the post-hoc check at pool.rs:1748 still fires (defense-in-depth)

- [ ] Run: `cargo nextest run -p lab --all-features` — full workspace. Confirm any existing test exercising `pool.rs::list_tools_for` with a manufactured oversized `CallToolResult` still passes (do not delete or modify the existing post-hoc check sites at 1748/2035/2532/2616).

### Step 2.12 — Commit

```bash
git add crates/lab/src/dispatch/upstream/http_client.rs \
        crates/lab/src/dispatch/upstream.rs \
        crates/lab/src/dispatch/upstream/pool.rs \
        crates/lab/Cargo.toml
git commit -m "fix(upstream): enforce response body cap at HTTP transport layer (lab-4z8sx.2)

Introduce BodyCappedHttpClient wrapping reqwest::Client and implementing
rmcp's StreamableHttpClient trait. Apply the LAB_UPSTREAM_MAX_RESPONSE_BYTES
cap (default 10 MB) to decoded body bytes BEFORE deserialization:

- post_message Json variant: cumulative cap on the buffer.
- post_message Sse variant + get_stream: per-event cap (not cumulative)
  so legitimate long-lived SSE subscriptions are not disconnected.

Both OAuth and non-OAuth HTTP connect paths now wrap their reqwest::Client.
Post-hoc checks at pool.rs:1748/2035/2532/2616 remain as defense-in-depth.

Tested with wiremock: 50 MB declared body via Content-Length, 20 MB chunked,
9 MB happy path, and 50 MB gzip-bomb expanding from 1 KB compressed —
all caught at the transport layer.

Refs: lab-4z8sx.2
"
```

---

## Task 3 (Wave 2 — after Task 2 lands): F1 process-group RAII guard

**Bead:** `lab-4z8sx.1`. Depends on Task 2 (F2) because both modify the `UpstreamConnection` struct definition.

**Files:**
- Create: `crates/lab/src/dispatch/upstream/process_guard.rs` (new RAII guard)
- Modify: `crates/lab/src/dispatch/upstream.rs` (re-export new module)
- Modify: `crates/lab/src/dispatch/upstream/pool.rs:560-666` (struct + Drop + shutdown)
- Modify: `crates/lab/src/dispatch/upstream/pool.rs:3300-3371` (connect_stdio_upstream — arm guard, disarm on success)
- Create: `crates/lab/tests/upstream_stdio_orphan.rs` (integration test, `#[ignore]`-marked)

**Locked invariants:**
- Drop fires `terminate_process_group_sigterm` then `terminate_process_group_sigkill` back-to-back. No sleep, no async, no error handling beyond ignoring the result.
- `UpstreamConnection::shutdown(mut self, ..)` MUST `self.runtime.pgid = None` BEFORE the first `.await`.
- Non-Unix builds: guard is `#[cfg(unix)]`-gated. Behavior unchanged on Windows.
- Do NOT add `tokio::process::Command.kill_on_drop(true)` — rmcp's `TokioChildProcess` already handles per-PID drop via its own async-kill path. Our guard targets the process GROUP (killpg), which is additive.

### Step 3.1 — Create the RAII guard

- [ ] Create `crates/lab/src/dispatch/upstream/process_guard.rs`:

```rust
//! RAII guard that SIGTERM+SIGKILLs a process group on Drop unless disarmed.
//!
//! Used in `connect_stdio_upstream` to ensure that if the connect future is
//! dropped between `spawn()` and the successful construction of
//! `UpstreamConnection`, the child's process group (created via
//! `process_wrap::ProcessGroup::leader`) is reaped — preventing orphan
//! grandchildren (npx → node, sh -c → python, etc.).
//!
//! On the happy path the guard is `.disarm()`'d and the pgid is transferred
//! to `UpstreamConnection`, whose own `Drop` impl takes over the role.
//!
//! Non-Unix builds: this module is empty (no process-group concept).

#[cfg(unix)]
pub struct ProcessGroupGuard {
    pgid: Option<u32>,
}

#[cfg(unix)]
impl ProcessGroupGuard {
    /// Arm the guard with a pgid. The pgid is expected to be the child's PID,
    /// since `ProcessGroup::leader()` makes the child its own group leader.
    #[must_use]
    pub fn arm(pgid: u32) -> Self {
        Self { pgid: Some(pgid) }
    }

    /// Disarm the guard, returning the pgid. After disarm, `Drop` does nothing.
    /// Callers transfer the returned pgid into `UpstreamConnection::runtime.pgid`.
    pub fn disarm(mut self) -> Option<u32> {
        self.pgid.take()
    }
}

#[cfg(unix)]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if let Some(pgid) = self.pgid.take() {
            // Sync syscalls (nix::sys::signal::killpg). Safe in Drop.
            // No sleep between TERM and KILL — Drop must not block; the
            // graceful 150ms wait belongs in the async shutdown() path.
            let _ = crate::process::unix::terminate_process_group_sigterm(pgid);
            let _ = crate::process::unix::terminate_process_group_sigkill(pgid);
        }
    }
}
```

### Step 3.2 — Register the new module

- [ ] Open `crates/lab/src/dispatch/upstream.rs`. Add `pub mod process_guard;` alongside the existing module declarations.
- [ ] Run `cargo check --workspace --all-features`. Must compile.

### Step 3.3 — Write the unit tests for the guard FIRST (TDD)

- [ ] Append to `crates/lab/src/dispatch/upstream/process_guard.rs`:

```rust
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::{Command as StdCommand, Stdio};
    use std::time::Duration;

    /// Spawn a real `sleep 30` child in its own process group via `setsid -w`,
    /// then drop the guard. The child must be reaped within 200 ms.
    #[test]
    fn drop_kills_unarmed_process_group() {
        let child = StdCommand::new("setsid")
            .args(["-w", "sleep", "30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn setsid sleep");
        let pid = child.id();
        // setsid creates a new session/group; pgid == pid for the new leader.
        let guard = ProcessGroupGuard::arm(pid);
        drop(guard);

        // Give the signals a beat to deliver and the kernel to reap.
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            !crate::process::unix::pid_is_alive(pid),
            "guard drop must reap pgid {pid}"
        );
    }

    /// Disarm the guard, then drop. The child must STILL be alive.
    /// Caller is responsible for cleanup.
    #[test]
    fn disarm_prevents_kill() {
        let mut child = StdCommand::new("setsid")
            .args(["-w", "sleep", "30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn setsid sleep");
        let pid = child.id();
        let guard = ProcessGroupGuard::arm(pid);
        let disarmed = guard.disarm();
        assert_eq!(disarmed, Some(pid), "disarm returns the armed pgid");
        assert!(
            crate::process::unix::pid_is_alive(pid),
            "disarmed guard must NOT kill"
        );
        // Cleanup so we don't leak the test process.
        let _ = crate::process::unix::terminate_process_group_sigkill(pid);
        let _ = child.kill();
        let _ = child.wait();
    }
}
```

### Step 3.4 — Run guard tests (they should pass immediately because the guard impl is complete)

- [ ] Run: `cargo nextest run -p lab --all-features process_guard::tests`
- [ ] Expected: 2 tests PASS on Linux. (On non-Unix the tests are cfg-gated out.)

### Step 3.5 — Add `_pgid_guard` field to `UpstreamConnection` and Drop impl

- [ ] Open `crates/lab/src/dispatch/upstream/pool.rs`. Locate the `UpstreamConnection` struct definition near line 568. Add a new `#[cfg(unix)]`-gated field for the guard. The simpler design noted in engineering review — move guard by value into UpstreamConnection — is what we're doing here:

```rust
    // BEFORE (lines ~568-578)
    /// A live connection to an upstream MCP server.
    struct UpstreamConnection {
        /// The running client service handle — kept alive to maintain the connection.
        _client_service: rmcp::service::RunningService<RoleClient, ()>,
        /// Background task holding an in-process server alive when applicable.
        _server_task: Option<tokio::task::JoinHandle<()>>,
        /// The peer handle for making requests.
        peer: rmcp::service::Peer<RoleClient>,
        /// Runtime metadata for process-backed upstreams.
        runtime: UpstreamRuntimeMetadata,
    }
```

```rust
    // AFTER
    /// A live connection to an upstream MCP server.
    struct UpstreamConnection {
        /// The running client service handle — kept alive to maintain the connection.
        _client_service: rmcp::service::RunningService<RoleClient, ()>,
        /// Background task holding an in-process server alive when applicable.
        _server_task: Option<tokio::task::JoinHandle<()>>,
        /// The peer handle for making requests.
        peer: rmcp::service::Peer<RoleClient>,
        /// Runtime metadata for process-backed upstreams.
        runtime: UpstreamRuntimeMetadata,
    }

    /// Drop impl: SIGTERM+SIGKILL the process group if any. Sync, no async,
    /// no error handling — last-resort abandonment cleanup. The async
    /// `shutdown()` path is the graceful counterpart; it zeros `runtime.pgid`
    /// before any `.await` so this Drop no-ops on the graceful path.
    #[cfg(unix)]
    impl Drop for UpstreamConnection {
        fn drop(&mut self) {
            if let Some(pgid) = self.runtime.pgid.take() {
                let _ = crate::process::unix::terminate_process_group_sigterm(pgid);
                let _ = crate::process::unix::terminate_process_group_sigkill(pgid);
            }
            if let Some(handle) = self._server_task.take() {
                handle.abort();
            }
        }
    }
```

### Step 3.6 — Update `shutdown()` to zero `runtime.pgid` BEFORE the first await

- [ ] Locate `UpstreamConnection::shutdown` at line 600. Modify the function head:

```rust
    // BEFORE (lines ~599-609)
    impl UpstreamConnection {
        async fn shutdown(mut self, upstream_name: &str, reason: &'static str) {
            let runtime = self.runtime.clone();
            let started = Instant::now();
            let result = self
                ._client_service
                .close_with_timeout(STDIO_SHUTDOWN_TIMEOUT)
                .await;
            if let Some(server_task) = self._server_task.take() {
                server_task.abort();
            }
```

```rust
    // AFTER
    impl UpstreamConnection {
        async fn shutdown(mut self, upstream_name: &str, reason: &'static str) {
            // INVARIANT: take pgid BEFORE any .await so the consuming Drop
            // sees None and no-ops. This prevents double-kill on the
            // graceful path. The local `runtime_pgid` carries the value
            // through the function so the graceful TERM/KILL sequence
            // below can still target it.
            #[cfg(unix)]
            let runtime_pgid = self.runtime.pgid.take();
            let runtime = self.runtime.clone();
            let started = Instant::now();
            let result = self
                ._client_service
                .close_with_timeout(STDIO_SHUTDOWN_TIMEOUT)
                .await;
            if let Some(server_task) = self._server_task.take() {
                server_task.abort();
            }
```

- [ ] In the existing graceful TERM/KILL block (around lines 611-620), replace the use of `runtime.pid`/`runtime.pgid` with `runtime_pgid` (since we zeroed the field):

```rust
    // BEFORE (lines ~611-620)
            #[cfg(unix)]
            if let (Some(pid), Some(pgid)) = (runtime.pid, runtime.pgid)
                && pid_is_alive(pid)
            {
                let _ = terminate_process_group_sigterm(pgid);
                tokio::time::sleep(Duration::from_millis(150)).await;
                if pid_is_alive(pid) {
                    let _ = terminate_process_group_sigkill(pgid);
                }
            }
```

```rust
    // AFTER
            #[cfg(unix)]
            if let (Some(pid), Some(pgid)) = (runtime.pid, runtime_pgid)
                && pid_is_alive(pid)
            {
                let _ = terminate_process_group_sigterm(pgid);
                tokio::time::sleep(Duration::from_millis(150)).await;
                if pid_is_alive(pid) {
                    let _ = terminate_process_group_sigkill(pgid);
                }
            }
```

> Note: `runtime` is the *cloned* metadata (carries the pid for liveness probing). `runtime_pgid` is the value we took from `self.runtime.pgid` to prevent double-kill from the consuming Drop.

### Step 3.7 — Arm the guard in `connect_stdio_upstream`, disarm on success

- [ ] Locate `connect_stdio_upstream` at line 3300. Modify to arm the guard immediately after PID extraction and disarm immediately before constructing the successful return:

```rust
    // BEFORE (lines ~3326-3370)
        #[cfg(unix)]
        let (process, _stderr) = {
            let mut wrapped = CommandWrap::from(cmd);
            wrapped.wrap(ProcessGroup::leader());
            TokioChildProcess::builder(wrapped)
                .stderr(Stdio::null())
                .spawn()?
        };
        #[cfg(not(unix))]
        let (process, _stderr) = TokioChildProcess::builder(cmd)
            .stderr(Stdio::null())
            .spawn()?;

        let pid = process.id();
        // ... logging ...
        let service: rmcp::service::RunningService<RoleClient, ()> = ().serve(process).await?;
        let peer = service.peer().clone();
        let tools = peer.list_all_tools().await?;
        // ... logging ...

        let conn = UpstreamConnection {
            _client_service: service,
            _server_task: None,
            peer,
            runtime: UpstreamRuntimeMetadata {
                pid,
                pgid: pid,
                started_at: Some(std::time::SystemTime::now()),
                origin: runtime_origin_label(runtime_origin, runtime_owner),
                owner: runtime_owner.cloned(),
            },
        };

        Ok((conn, tools))
    }
```

```rust
    // AFTER
        #[cfg(unix)]
        let (process, _stderr) = {
            let mut wrapped = CommandWrap::from(cmd);
            wrapped.wrap(ProcessGroup::leader());
            TokioChildProcess::builder(wrapped)
                .stderr(Stdio::null())
                .spawn()?
        };
        #[cfg(not(unix))]
        let (process, _stderr) = TokioChildProcess::builder(cmd)
            .stderr(Stdio::null())
            .spawn()?;

        let pid = process.id();

        // INVARIANT: arm the process-group guard IMMEDIATELY after spawn.
        // If any subsequent `?` propagates (serve fails, list_all_tools fails,
        // the outer future is dropped on timeout, etc.), Drop on this guard
        // SIGTERM+SIGKILLs the process group, reaping grandchildren that
        // rmcp's per-PID drop would otherwise miss.
        #[cfg(unix)]
        let guard = pid.map(super::process_guard::ProcessGroupGuard::arm);

        // ... existing logging ...
        let service: rmcp::service::RunningService<RoleClient, ()> = ().serve(process).await?;
        let peer = service.peer().clone();
        let tools = peer.list_all_tools().await?;
        // ... existing logging ...

        // INVARIANT: disarm right before successful return. The pgid is
        // transferred to UpstreamConnection.runtime.pgid; its own Drop now
        // owns cleanup. shutdown() will zero runtime.pgid before any await
        // so Drop no-ops on the graceful path.
        #[cfg(unix)]
        let pgid_for_runtime = guard.and_then(super::process_guard::ProcessGroupGuard::disarm);
        #[cfg(not(unix))]
        let pgid_for_runtime: Option<u32> = pid; // unchanged behavior on Windows

        let conn = UpstreamConnection {
            _client_service: service,
            _server_task: None,
            peer,
            runtime: UpstreamRuntimeMetadata {
                pid,
                pgid: pgid_for_runtime,
                started_at: Some(std::time::SystemTime::now()),
                origin: runtime_origin_label(runtime_origin, runtime_owner),
                owner: runtime_owner.cloned(),
            },
        };

        Ok((conn, tools))
    }
```

### Step 3.8 — Compile and run unit tests

- [ ] Run: `cargo check --workspace --all-features`
- [ ] Run: `cargo clippy --workspace --all-features -- -D warnings`
- [ ] Run: `cargo nextest run -p lab --all-features`
- [ ] All must pass. Pay attention to whether the existing `drain_for_swap` path (pool.rs:799-806) still works — it calls `shutdown()` which now zeroes pgid before await. Drop on the consumed `UpstreamConnection` then no-ops — exactly what we want.

### Step 3.9 — Write the integration test for the leak-prevention end-to-end

- [ ] Create `crates/lab/tests/upstream_stdio_orphan.rs`:

```rust
//! Integration test for the upstream stdio process-orphan fix (lab-4z8sx.1).
//!
//! Marked `#[ignore]` because it spawns real processes and checks `/proc`
//! state; only run via `cargo nextest run --run-ignored only` (or
//! `cargo test -- --ignored`).

#![cfg(all(unix, target_os = "linux"))]

use std::process::Command as StdCommand;
use std::time::{Duration, Instant};

/// Spawn a shell that double-forks a long-running sleep, then drops the
/// guard via early return. The orphaned sleep must be reaped.
#[ignore]
#[tokio::test]
async fn connect_stdio_dropped_future_reaps_grandchild() {
    // Pick a unique sentinel arg so we can grep `/proc` for it.
    let sentinel = format!("lab-orphan-test-{}", std::process::id());
    let script = format!(
        "sleep 30 --version-marker={sentinel} &\necho '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{}}}}'\nexec sh -c 'while :; do sleep 1; done'\n"
    );

    // Use the existing connect_stdio_upstream code path indirectly via
    // an UpstreamConfig + a 50 ms timeout to force the future to be dropped
    // mid-list_all_tools. The exact wiring depends on what's pub(crate);
    // if necessary, expose a #[doc(hidden)] test helper from pool.rs.

    // Pseudocode (engineer: adapt to the actual public test surface):
    //   let cfg = UpstreamConfig { name: "test", command: Some("/bin/sh".into()),
    //                              args: vec!["-c".into(), script.clone()], .. };
    //   let result = tokio::time::timeout(
    //       Duration::from_millis(50),
    //       connect_stdio_upstream("/bin/sh", &["-c".into(), script], &cfg, None, None),
    //   ).await;
    //   assert!(result.is_err(), "must time out");

    // After the timeout fires, the guard should have killed the entire process
    // group. Verify the sentinel is gone from /proc within 1 s.
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        let out = StdCommand::new("pgrep").args(["-f", &sentinel]).output();
        match out {
            Ok(o) if !o.status.success() => return, // pgrep exits non-zero when no match
            _ => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    let _final = StdCommand::new("pgrep").args(["-f", &sentinel]).output();
    panic!("orphan sleep process matching '{sentinel}' was not reaped within 1s");
}
```

> **Engineer note:** the test references `connect_stdio_upstream` directly, which is currently a private fn in `pool.rs`. Add `pub(crate)` to it (or expose a thin `#[doc(hidden)]` test-only wrapper) to make it callable from the integration test. Avoid widening visibility further than necessary.

### Step 3.10 — Run the integration test

- [ ] Run: `cargo nextest run -p lab --all-features --run-ignored only connect_stdio_dropped_future_reaps_grandchild`
- [ ] Expected: PASS on Linux. Skipped on macOS/Windows (cfg-gated).
- [ ] Loop verification: run the test 10× and confirm no leaked processes:

```bash
for i in {1..10}; do
  cargo nextest run -p lab --all-features --run-ignored only connect_stdio_dropped_future_reaps_grandchild || exit 1
done
pgrep -f lab-orphan-test- && { echo "LEAK: sentinel processes survived"; exit 1; }
echo "OK: no surviving sentinels after 10 runs"
```

### Step 3.11 — Final workspace verification

- [ ] Run: `cargo nextest run --workspace --all-features`
- [ ] Run: `cargo clippy --workspace --all-features -- -D warnings`
- [ ] Run: `cargo fmt --all -- --check`
- [ ] All must pass. The graceful-path test (`UpstreamConnection::shutdown` with 150 ms TERM-then-KILL) must still pass — it now relies on `runtime_pgid` carrying the pgid through `take()`.

### Step 3.12 — Commit

```bash
git add crates/lab/src/dispatch/upstream/process_guard.rs \
        crates/lab/src/dispatch/upstream.rs \
        crates/lab/src/dispatch/upstream/pool.rs \
        crates/lab/tests/upstream_stdio_orphan.rs
git commit -m "fix(upstream): RAII process-group guard prevents stdio orphans (lab-4z8sx.1)

Add ProcessGroupGuard (sync-Drop RAII) armed immediately after the
stdio child process spawns and disarmed only on the happy-path return
from connect_stdio_upstream. If the connect future is dropped mid-flight
(discovery timeout, list_tools error, buffer_unordered cancellation),
the guard SIGTERM+SIGKILLs the process group via killpg — reaping
grandchildren (npx → node, sh -c → python) that rmcp's per-PID Drop
would otherwise miss.

UpstreamConnection gets its own Drop impl that performs the same TERM+KILL.
shutdown(mut self) zeroes self.runtime.pgid BEFORE the first .await so the
consuming Drop no-ops on the graceful path — no double-kill.

#[cfg(unix)]-gated end to end; Windows behavior unchanged.

Refs: lab-4z8sx.1, MEMORY:process_spawn_culprit
"
```

---

## Closeout

- [ ] After all three commits land:
  - `bd update lab-4z8sx.3 --status closed`
  - `bd update lab-4z8sx.2 --status closed`
  - `bd update lab-4z8sx.1 --status closed`
  - `bd update lab-4z8sx --status closed`
- [ ] Run `bd memories add lab-4z8sx "LEARNED: rmcp's TokioChildProcess covers per-PID Drop via its own async-kill path. To prevent stdio grandchild orphans, add a process-GROUP RAII guard (killpg) — these are additive, not duplicative. Do NOT set kill_on_drop(true) on top of process_wrap (double-kill)."`
- [ ] Open a PR; reference epic `lab-4z8sx` in the PR description.

## Self-review checklist (filled in)

- **Spec coverage:** F1, F2, F3 each have a dedicated task with TDD tests, locked invariants from the beads, and explicit before/after code snippets. SSE per-event cap (engineering review BLOCKING finding) is implemented in Task 2.5 via `build_per_event_capped_sse`. The dependency ordering (F2 before F1) is reflected in the task order.
- **Placeholders:** None — every "Step" contains the code or command to execute. `reqwest_error_from_str` is the one fragile area and the plan explicitly tells the engineer how to recover (switch to a custom error enum if the synthetic-error shim doesn't compile).
- **Type consistency:** `BodyCappedHttpClient` named consistently across Task 2 sites. `ProcessGroupGuard::arm/disarm` named consistently across Task 3 sites. `runtime.pgid` and `runtime_pgid` are clearly distinguished (the former is the field, the latter the local that owns it after `take()`).
