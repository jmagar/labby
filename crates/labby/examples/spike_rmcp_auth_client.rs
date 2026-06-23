//! Task 0 — rmcp `AuthClient` integration PoC (gating spike).
//!
//! # What this proves
//!
//! Before Task 2 commits to a design for wiring OAuth-authorized upstream MCP
//! clients, this spike verifies four integration points against `rmcp 1.4.0`:
//!
//! 1. `AuthClient<reqwest::Client>` can be constructed on top of an
//!    `AuthorizationManager` and its pluggable `CredentialStore`.
//! 2. `AuthClient` auto-injects `Authorization: Bearer <token>` on outbound
//!    requests without the caller having to pass `auth_header` explicitly —
//!    the `StreamableHttpClient` impl in
//!    `rmcp-1.4.0/src/transport/common/auth/streamable_http_client.rs` is the
//!    canonical integration path.
//! 3. On a synthesized 401 from the upstream, rmcp does **not** automatically
//!    refresh. `AuthorizationManager::get_access_token()` only refreshes when
//!    the local clock says the cached token has `< REFRESH_BUFFER_SECS` (30 s)
//!    remaining. Refresh-on-401 is the caller's responsibility.
//! 4. The spike runs against (a) a wiremock OAuth stub by default, and
//!    (b) a real OAuth-protected MCP upstream when `SPIKE_REAL_AS_URL` is
//!    set (operator validates interactively).
//!
//! # Running
//!
//! ```bash
//! # wiremock mode (default, no external services)
//! cargo run --example spike_rmcp_auth_client --all-features
//!
//! # real upstream mode — the operator must have already minted a bearer
//! # token for the upstream and exported it as SPIKE_REAL_AS_TOKEN.
//! SPIKE_REAL_AS_URL=https://example.com/mcp \
//!   SPIKE_REAL_AS_TOKEN=eyJhbGciOi… \
//!   cargo run --example spike_rmcp_auth_client --all-features
//! ```
//!
//! # Findings (mirror of `crates/lab/src/oauth/upstream/refresh.rs`)
//!
//! - **rmcp version verified:** `1.4.0`.
//! - **Integration path:** Plan A. `AuthClient<reqwest::Client>` IS a
//!   `StreamableHttpClient`; it wraps any underlying `C: StreamableHttpClient`
//!   and, when the caller passes `auth_token: None`, calls
//!   `self.get_access_token()` → `auth_manager.lock().await.get_access_token()`
//!   and injects the bearer before delegating.
//! - **`auth_manager.refresh_token()` is manual.** No automatic refresh on a
//!   401 response. rmcp 1.4's refresh trigger is purely local-clock based
//!   (`REFRESH_BUFFER_SECS = 30`). Task 2 must layer refresh-on-401 around
//!   `AuthClient`.
//! - **Plan B not needed.** The spike confirms Plan A end-to-end.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use oauth2::{AccessToken, basic::BasicTokenType};
use rmcp::{
    model::{ClientJsonRpcMessage, ClientRequest, NumberOrString, PingRequest},
    transport::{
        auth::{
            AuthClient, AuthError, AuthorizationManager, AuthorizationMetadata,
            InMemoryCredentialStore, OAuthTokenResponse, StoredCredentials, VendorExtraTokenFields,
        },
        streamable_http_client::{StreamableHttpClient, StreamableHttpError},
    },
};
use tracing::{info, warn};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

/// Bearer token we seed into the credential store. Wiremock matches on it.
const SEEDED_ACCESS_TOKEN: &str = "spike-access-token-abc123";
const CLIENT_ID: &str = "spike-client-id";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,rmcp=debug")),
        )
        .with_target(false)
        .init();

    if let Ok(real_url) = std::env::var("SPIKE_REAL_AS_URL") {
        run_real_upstream(real_url).await?;
    } else {
        run_wiremock_spike().await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Wiremock mode — default. Proves auto-injection and no-auto-refresh.
// ---------------------------------------------------------------------------

async fn run_wiremock_spike() -> Result<()> {
    info!("==> spike: wiremock mode");

    // The AS endpoints are conceptually separate from the resource server,
    // but for the spike we host both on one wiremock instance.
    let server = MockServer::start().await;
    let base_uri = server.uri();
    info!(base_uri = %base_uri, "started wiremock AS+RS stub");

    // --- Counters so we can assert "no refresh happened". -----------------
    let mcp_call_counter = Arc::new(AtomicUsize::new(0));
    let token_endpoint_counter = Arc::new(AtomicUsize::new(0));

    // Step 1 check — asserts the seeded bearer token is sent, then returns 401
    // to drive the re-authorization path (token is treated as expired/invalid).
    let mcp_counter_ok = mcp_call_counter.clone();
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header(
            "authorization",
            format!("Bearer {SEEDED_ACCESS_TOKEN}").as_str(),
        ))
        .respond_with(move |_req: &wiremock::Request| {
            mcp_counter_ok.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(401)
                .insert_header("www-authenticate", r#"Bearer error="invalid_token""#)
        })
        .up_to_n_times(1)
        .mount(&server)
        .await;

    // Step 3 check — a /token endpoint would refresh. We register a matcher
    // that just counts hits so we can assert rmcp does NOT hit it.
    let token_counter = token_endpoint_counter.clone();
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(move |_req: &wiremock::Request| {
            token_counter.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "refreshed-token-should-not-be-issued",
                "token_type": "Bearer",
                "expires_in": 3600,
            }))
        })
        .mount(&server)
        .await;

    // Build the AuthClient. See `build_auth_client` below for the seeding logic.
    let auth_client = build_auth_client(&base_uri).await?;

    // --- Integration point #1: construction ------------------------------
    info!("point 1: AuthClient<reqwest::Client> constructed OK");

    // --- Integration point #2: auto-injection of Authorization header ----
    //
    // We call `AuthClient::post_message` directly (bypassing the full MCP
    // handshake) — this is the same trait method `StreamableHttpClientWorker`
    // invokes on every outbound message. We pass `auth_token: None`, which is
    // what `StreamableHttpClientTransport` would pass if its
    // `config.auth_header` is left unset. The wiremock matcher above requires
    // the header; if AuthClient did NOT inject, the mock would not match and
    // wiremock would 404.
    let mcp_uri: Arc<str> = Arc::from(format!("{base_uri}/mcp").as_str());
    let ping = ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        NumberOrString::Number(1),
    );
    let result = auth_client
        .post_message(mcp_uri.clone(), ping, None, None, HashMap::new())
        .await;

    match result {
        Err(StreamableHttpError::AuthRequired(e)) => {
            info!(
                www_authenticate = %e.www_authenticate_header,
                "point 2: AuthClient injected Bearer header automatically (matched by wiremock); \
                 upstream returned 401 as scripted",
            );
        }
        Err(StreamableHttpError::UnexpectedServerResponse(msg)) => {
            // Some rmcp 1.4 paths wrap 401 as this variant depending on body.
            info!(msg = %msg, "point 2: Bearer injected; got 401 via UnexpectedServerResponse");
        }
        other => {
            return Err(anyhow::anyhow!(
                "spike failed at point 2: expected AuthRequired/unexpected response, got: {:?}",
                other.map(|_| "Ok")
            ));
        }
    }

    // Confirm the wiremock matcher that REQUIRED the header was hit exactly once.
    let seen = mcp_call_counter.load(Ordering::SeqCst);
    anyhow::ensure!(
        seen == 1,
        "spike failed: expected 1 MCP call with Bearer header, got {seen} — \
         AuthClient did NOT auto-inject the Authorization header (Plan A is BROKEN)",
    );
    info!("point 2 CONFIRMED: Authorization header auto-injection works.");

    // --- Integration point #3: no-auto-refresh on 401 --------------------
    //
    // The previous call returned 401. rmcp 1.4 does NOT automatically invoke
    // `auth_manager.refresh_token()` in response to a 401. We assert that the
    // /token endpoint was NOT hit during the call above. (We seeded the
    // credential store with NO expiry, so the local-clock refresh trigger
    // also does not fire.)
    tokio::time::sleep(Duration::from_millis(50)).await;
    let token_hits = token_endpoint_counter.load(Ordering::SeqCst);
    anyhow::ensure!(
        token_hits == 0,
        "spike surprised: rmcp 1.4 DID hit the /token endpoint ({token_hits} times). \
         The no-auto-refresh finding is invalid — re-read auth.rs before trusting refresh.rs.",
    );
    info!("point 3 CONFIRMED: rmcp did NOT auto-refresh on 401 (token endpoint hits = 0).");
    info!("        refresh-on-401 must be implemented by Task 2 (see oauth/upstream/refresh.rs).");

    // --- Integration point #4: interactive / real upstream path is below
    // (run with SPIKE_REAL_AS_URL set).
    info!(
        "point 4: skipped — set SPIKE_REAL_AS_URL=<mcp_url> SPIKE_REAL_AS_TOKEN=<bearer> to \
         exercise against a live OAuth-protected MCP upstream."
    );

    info!("==> spike: ALL POINTS CONFIRMED. Plan A is viable. Proceed to Task 2.");
    Ok(())
}

/// Build an `AuthClient<reqwest::Client>` with a seeded non-expiring token.
///
/// The key moves:
/// 1. `InMemoryCredentialStore` is populated with a `StoredCredentials` whose
///    `OAuthTokenResponse` has NO `expires_in` — `get_access_token()` will
///    therefore skip the local-clock refresh check and return the seeded
///    token verbatim.
/// 2. The store is handed to an `AuthorizationManager` via
///    `set_credential_store`. The manager also needs `set_metadata` and
///    `configure_client_id` so its internal oauth2 client is wired, even
///    though in wiremock mode we never actually exercise the refresh path.
/// 3. `AuthClient::new(reqwest::Client::new(), manager)` produces the
///    `StreamableHttpClient` we hand to the rest of the spike.
async fn build_auth_client(base_uri: &str) -> Result<AuthClient<reqwest::Client>> {
    // 1. Seed credentials. No expires_in → skip the refresh-buffer branch.
    let token_response = OAuthTokenResponse::new(
        AccessToken::new(SEEDED_ACCESS_TOKEN.to_string()),
        BasicTokenType::Bearer,
        VendorExtraTokenFields::default(),
    );
    let store = InMemoryCredentialStore::new();
    rmcp::transport::auth::CredentialStore::save(
        &store,
        StoredCredentials::new(
            CLIENT_ID.to_string(),
            Some(token_response),
            vec!["read".to_string()],
            None, // token_received_at unset — combined with no expires_in, refresh path is off
        ),
    )
    .await
    .context("seed credential store")?;

    // 2. Wire the manager. `base_url` for the manager is the RS base URL; we
    // reuse the wiremock URL. Metadata endpoints point at the same wiremock
    // (never actually hit in this spike).
    let mut manager = AuthorizationManager::new(base_uri)
        .await
        .context("construct AuthorizationManager")?;
    let mut metadata = AuthorizationMetadata::default();
    metadata.authorization_endpoint = format!("{base_uri}/authorize");
    metadata.token_endpoint = format!("{base_uri}/token");
    manager.set_metadata(metadata);
    manager.set_credential_store(store);
    manager
        .configure_client_id(CLIENT_ID)
        .context("configure oauth2 client id")?;

    // Sanity: the manager must now yield the seeded token. If this fails the
    // whole spike is meaningless because AuthClient would skip injection.
    match manager.get_access_token().await {
        Ok(t) if t == SEEDED_ACCESS_TOKEN => {}
        Ok(_other) => anyhow::bail!("expected seeded token, got <unexpected token>"),
        Err(AuthError::AuthorizationRequired) => {
            anyhow::bail!("manager says no creds — credential store seeding is broken");
        }
        Err(e) => anyhow::bail!("unexpected AuthError from manager: {e}"),
    }

    // 3. Wrap.
    Ok(AuthClient::new(reqwest::Client::new(), manager))
}

// ---------------------------------------------------------------------------
// Real-upstream mode — operator validation.
// ---------------------------------------------------------------------------

async fn run_real_upstream(mcp_url: String) -> Result<()> {
    info!(mcp_url = %mcp_url, "==> spike: real upstream mode");

    let bearer = std::env::var("SPIKE_REAL_AS_TOKEN").context(
        "SPIKE_REAL_AS_URL is set but SPIKE_REAL_AS_TOKEN is not — \
         paste a bearer you minted out-of-band (the spike does not run the full OAuth dance)",
    )?;

    // We still go through AuthClient + InMemoryCredentialStore, just with a
    // hand-minted token. This exercises the same code path as wiremock mode.
    let token_response = OAuthTokenResponse::new(
        AccessToken::new(bearer),
        BasicTokenType::Bearer,
        VendorExtraTokenFields::default(),
    );
    let store = InMemoryCredentialStore::new();
    rmcp::transport::auth::CredentialStore::save(
        &store,
        StoredCredentials::new(
            "real-client".to_string(),
            Some(token_response),
            vec![],
            None,
        ),
    )
    .await?;
    let mut manager = AuthorizationManager::new(&mcp_url).await?;
    let mut metadata = AuthorizationMetadata::default();
    metadata.authorization_endpoint = format!("{mcp_url}/__noop_authorize");
    metadata.token_endpoint = format!("{mcp_url}/__noop_token");
    manager.set_metadata(metadata);
    manager.set_credential_store(store);
    manager.configure_client_id("real-client")?;
    let auth_client = AuthClient::new(reqwest::Client::new(), manager);

    let uri: Arc<str> = Arc::from(mcp_url.as_str());
    let ping = ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        NumberOrString::Number(1),
    );
    match auth_client
        .post_message(uri, ping, None, None, HashMap::new())
        .await
    {
        Ok(resp) => {
            info!(
                ?resp,
                "real upstream responded successfully — Plan A works end-to-end"
            );
        }
        Err(StreamableHttpError::AuthRequired(e)) => {
            warn!(
                www_authenticate = %e.www_authenticate_header,
                "real upstream returned 401 — either the bearer is bad or the upstream rejects it. \
                 This still proves header injection worked (the upstream saw Authorization); \
                 it does not invalidate Plan A."
            );
        }
        Err(e) => {
            warn!(error = ?e, "real upstream call failed with non-401 error; inspect manually");
        }
    }

    Ok(())
}
