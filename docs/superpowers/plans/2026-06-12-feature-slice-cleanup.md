# Feature Slice Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `labby` compile under intentional narrow Cargo feature slices without weakening the all-features production path or existing gateway/API/MCP security behavior.

**Architecture:** Keep `labby` as one binary crate. Move compile-time feature boundaries to the actual ownership edges: module declarations, CLI variants, API route mounting, MCP adapters, and registry registration. Shared security logic that gateway and marketplace both need moves to an always-compiled dispatch security module with a timeout-wrapped async entry point, while `marketplace` remains the owner of MCP Registry behavior for this plan.

**Tech Stack:** Rust 2024, Cargo features, `#[cfg(feature = "...")]`, Axum, rmcp, shell verification, `cargo check --all-targets`.

---

## Engineering Review Changes Applied

- Narrowed standalone feature acceptance to product slices: no-default core, `gateway`, `marketplace`, `fs`, `deploy`, `acp_registry`, `gateway,marketplace`, `all`, and `--all-features`.
- Removed standalone `mcpregistry` from the acceptance contract. For this plan, `mcpregistry` is dependency-owned by `marketplace`; splitting it later is separate work.
- Removed standalone helper/internal acceptance for `systemd`, `node-runtime`, and `test-utils`.
- Added `--all-targets` checks for supported narrow slices so test modules compile too.
- Replaced brittle route snippets with security-preserving invariants for large active files.
- Preserved scaffold markers as an explicit requirement.
- Moved URL validation to `dispatch/security/ssrf.rs`, not `net::ssrf`, because it returns `ToolError`.
- Preserved the current SSRF blocked ranges, including RFC1918, loopback, link-local, unspecified, CGNAT/Tailscale `100.64.0.0/10`, IPv6 ULA `fc00::/7`, and IPv4-mapped equivalents.
- Added a timeout-wrapped async helper for blocking DNS and documented the DNS-rebinding preflight limitation.
- Added route-security tests for `/v1/gateway` auth and marketplace host validation.
- Added protected MCP route fail-closed requirements for no-gateway builds.
- Added MCP Apps UI capability gating and targeted MCP passthrough regression tests.
- Added registry/router parity test updates and generated docs verification.
- Deferred CI workflow expansion to follow-up unless explicitly requested.

## Supported Feature Contract

These commands are the acceptance bar:

```bash
cargo check -p lab-apis --no-default-features
cargo check -p lab-apis --no-default-features --features all
cargo check -p labby --no-default-features --all-targets
cargo check -p labby --no-default-features --features gateway --all-targets
cargo check -p labby --no-default-features --features marketplace --all-targets
cargo check -p labby --no-default-features --features fs --all-targets
cargo check -p labby --no-default-features --features deploy --all-targets
cargo check -p labby --no-default-features --features acp_registry --all-targets
cargo check -p labby --no-default-features --features gateway,marketplace --all-targets
cargo check -p labby --no-default-features --features all --all-targets
cargo check -p labby --all-features --all-targets
```

`mcpregistry` is not a standalone slice in this plan. It remains enabled through `marketplace = ["mcpregistry"]`. A future split may create `marketplace-core` or move registry client/store helpers out of `dispatch::marketplace`; that is explicitly out of scope here.

`systemd`, `node-runtime`, and `test-utils` are helper/internal features. They may compile alone after the cleanup, but this plan does not make that a product contract or CI gate.

## File Structure

- Create `scripts/check-feature-slices.sh`: local verification for the supported feature contract.
- Modify `crates/lab/Cargo.toml`: document feature-slice policy.
- Create `crates/lab/src/dispatch/security.rs`: module entry point for always-compiled dispatch security helpers.
- Create `crates/lab/src/dispatch/security/ssrf.rs`: shared SSRF preflight helper with blocking and async timeout APIs.
- Modify `crates/lab/src/dispatch.rs`: expose `security`.
- Modify `crates/lab/src/dispatch/marketplace/mcp_params.rs`: remove marketplace-owned URL validation.
- Modify `crates/lab/src/dispatch/marketplace.rs`: re-export the shared validator for compatibility.
- Modify `crates/lab/src/dispatch/gateway/oauth_lifecycle/probe.rs`: call the shared async validator.
- Modify `crates/lab/src/cli.rs`: gate feature-backed CLI modules, variants, and dispatch arms while preserving scaffold markers.
- Modify `crates/lab/src/cli/serve.rs`: isolate gateway runtime setup and protected-route behavior behind gateway helpers.
- Modify `crates/lab/src/cli/helpers.rs`: gate or fallback any helper that reads the gateway manager.
- Modify `crates/lab/src/api.rs`: gate gateway-only modules.
- Modify `crates/lab/src/api/state.rs`: gate gateway manager state and registry-store fields consistently.
- Modify `crates/lab/src/api/services.rs`: gate feature-backed route modules.
- Modify `crates/lab/src/api/services/helpers.rs`: gate helper code that reads gateway state.
- Modify `crates/lab/src/api/health.rs`: gate health reporting that depends on gateway state.
- Modify `crates/lab/src/api/router.rs`: use gateway-gated helper functions for route mounting, protected-route metadata, upstream OAuth routes, and marketplace dev route mounting.
- Modify `crates/lab/src/registry.rs`: register only compiled services and update registry/router parity tests.
- Modify `crates/lab/src/mcp.rs`: gate gateway/upstream-specific modules.
- Modify `crates/lab/src/mcp/server.rs`: gate gateway manager fields and MCP Apps UI capability advertisement.
- Modify gateway-heavy MCP modules reported by `cargo check`, especially `call_tool.rs`, `catalog.rs`, `context.rs`, `handlers_prompts.rs`, `handlers_resources.rs`, `handlers_tools.rs`, and tests.
- Modify generated docs only through the repo’s docs generation command.

## Non-Negotiable Invariants

- Preserve all existing `// [lab-scaffold: ...]` markers.
- Preserve existing `/v1/gateway` auth behavior: gateway admin routes mount only when API auth is configured.
- Preserve existing marketplace host-validation middleware.
- Preserve protected MCP route middleware order in gateway builds.
- In no-gateway builds, configured protected MCP routes must fail closed: startup must reject the configuration or log a startup error and not advertise protected-route support.
- Non-gateway MCP builds must not advertise MCP Apps UI capability or Code Mode resources.
- All-features remains the authoritative build.
- Compile success alone is not enough for gateway/security-sensitive changes; targeted route and MCP tests must pass.

### Task 1: Add Local Feature Slice Verification

**Files:**
- Create: `scripts/check-feature-slices.sh`
- Modify: `crates/lab/Cargo.toml`

- [ ] **Step 1: Create the verification script**

Create `scripts/check-feature-slices.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

run cargo check -p lab-apis --no-default-features
run cargo check -p lab-apis --no-default-features --features all

labby_features=(
  ""
  "gateway"
  "marketplace"
  "fs"
  "deploy"
  "acp_registry"
  "gateway,marketplace"
  "all"
)

for features in "${labby_features[@]}"; do
  if [[ -z "$features" ]]; then
    run cargo check -p labby --no-default-features --all-targets
  else
    run cargo check -p labby --no-default-features --features "$features" --all-targets
  fi
done

run cargo check -p labby --all-features --all-targets
```

- [ ] **Step 2: Make it executable**

Run:

```bash
chmod +x scripts/check-feature-slices.sh
```

Expected: exits successfully.

- [ ] **Step 3: Run once and capture the first failure**

Run:

```bash
scripts/check-feature-slices.sh
```

Expected: FAIL before implementation. The failure should be an unresolved gated symbol such as `crate::dispatch::gateway`, `crate::dispatch::upstream`, or `crate::dispatch::marketplace`.

- [ ] **Step 4: Document feature policy**

Add this comment immediately above `[features]` in `crates/lab/Cargo.toml`:

```toml
# Feature-slice contract:
# - `default` keeps the production all-features binary.
# - `all` is the authoritative development and release build.
# - Product slices must compile with `--no-default-features --features <slice> --all-targets`.
# - Supported standalone product slices are: gateway, marketplace, fs, deploy, acp_registry.
# - `mcpregistry` is owned by marketplace in this crate; do not treat it as standalone here.
# - Helper/internal features are not standalone product slices unless this comment is updated.
```

- [ ] **Step 5: Do not commit a red harness by itself**

Keep the failing run as local evidence. Commit the script with the first implementation task that makes at least the no-default and `gateway` slices pass.

### Task 2: Move SSRF Preflight To Dispatch Security

**Files:**
- Create: `crates/lab/src/dispatch/security.rs`
- Create: `crates/lab/src/dispatch/security/ssrf.rs`
- Modify: `crates/lab/src/dispatch.rs`
- Modify: `crates/lab/src/dispatch/marketplace/mcp_params.rs`
- Modify: `crates/lab/src/dispatch/marketplace.rs`
- Modify: `crates/lab/src/dispatch/gateway/oauth_lifecycle/probe.rs`
- Modify: `crates/lab/src/dispatch/marketplace/mcp_dispatch.rs`

- [ ] **Step 1: Add the security module entry point**

Create `crates/lab/src/dispatch/security.rs`:

```rust
pub mod ssrf;
```

In `crates/lab/src/dispatch.rs`, add:

```rust
pub mod security;
```

Place it with the always-compiled dispatch modules.

- [ ] **Step 2: Add the shared SSRF helper**

Create `crates/lab/src/dispatch/security/ssrf.rs`:

```rust
//! Shared SSRF preflight guards for externally supplied HTTPS URLs.
//!
//! This is a preflight guard, not a complete DNS-rebinding defense. Any code
//! that later performs an outbound request must still avoid unsafe redirects and
//! must not claim that validation-time DNS pins the final connection target.

use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

use crate::dispatch::error::ToolError;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Validate an externally supplied HTTPS URL using blocking DNS.
///
/// # Blocking
/// Call this only from blocking contexts. Async dispatch paths should call
/// [`validate_external_https_url`] instead.
pub fn validate_external_https_url_blocking(url: &str) -> Result<(), ToolError> {
    let redacted = redact_url_for_error(url);
    let ssrf_err = |msg: String| ToolError::Sdk {
        sdk_kind: "ssrf_blocked".to_string(),
        message: msg,
    };

    let parsed = url::Url::parse(url)
        .map_err(|e| ssrf_err(format!("invalid URL `{redacted}`: {e}")))?;

    if parsed.scheme() != "https" {
        return Err(ssrf_err(format!(
            "URL `{redacted}` must use https to prevent SSRF"
        )));
    }

    if parsed.username() != "" || parsed.password().is_some() {
        return Err(ssrf_err(format!(
            "URL `{redacted}` must not include userinfo"
        )));
    }

    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(ssrf_err(format!(
            "URL `{redacted}` must not include query or fragment components"
        )));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| ssrf_err(format!("URL `{redacted}` must include a host")))?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| ssrf_err(format!("URL `{redacted}` must include a resolvable port")))?;

    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| ssrf_err(format!("failed to resolve host `{host}`: {e}")))?;

    for sock_addr in addrs {
        check_ip_not_private(sock_addr.ip(), &redacted)?;
    }

    Ok(())
}

/// Async wrapper for request/dispatch paths. Owns `spawn_blocking` and timeout
/// so async callers do not accidentally block runtime workers forever.
pub async fn validate_external_https_url(url: &str) -> Result<(), ToolError> {
    let url = url.to_string();
    tokio::time::timeout(DEFAULT_TIMEOUT, tokio::task::spawn_blocking(move || {
        validate_external_https_url_blocking(&url)
    }))
    .await
    .map_err(|_| ToolError::Sdk {
        sdk_kind: "ssrf_blocked".to_string(),
        message: "URL validation timed out".to_string(),
    })?
    .map_err(|e| ToolError::internal_message(format!("SSRF validation task panicked: {e}")))?
}

fn check_ip_not_private(ip: IpAddr, redacted_url: &str) -> Result<(), ToolError> {
    let blocked = match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || is_cgnat(v4)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || is_ipv6_link_local(v6)
                || is_ipv6_ula(v6)
                || v6.to_ipv4_mapped().is_some_and(|v4| {
                    v4.is_private()
                        || v4.is_loopback()
                        || v4.is_link_local()
                        || v4.is_unspecified()
                        || is_cgnat(v4)
                })
        }
    };

    if blocked {
        return Err(ToolError::Sdk {
            sdk_kind: "ssrf_blocked".to_string(),
            message: format!(
                "URL `{redacted_url}` resolves to private, loopback, link-local, CGNAT, or ULA address {ip}; blocked to prevent SSRF"
            ),
        });
    }

    Ok(())
}

fn is_cgnat(ip: std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

fn is_ipv6_link_local(ip: std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

fn is_ipv6_ula(ip: std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn redact_url_for_error(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        Err(_) => "<invalid-url>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_https_url() {
        let err = validate_external_https_url_blocking("http://example.com").unwrap_err();
        assert_eq!(err.kind(), "ssrf_blocked");
    }

    #[test]
    fn rejects_userinfo_query_and_fragment() {
        for url in [
            "https://user@example.com",
            "https://example.com/path?token=secret",
            "https://example.com/path#secret",
        ] {
            let err = validate_external_https_url_blocking(url).unwrap_err();
            assert_eq!(err.kind(), "ssrf_blocked");
            assert!(!err.user_message().contains("secret"));
        }
    }

    #[test]
    fn blocks_private_ranges_exactly() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "100.127.255.255",
            "::1",
            "fe80::1",
            "fc00::1",
            "fd00::1",
            "::ffff:127.0.0.1",
            "::ffff:10.1.2.3",
            "::ffff:100.64.0.1",
        ] {
            let parsed: IpAddr = ip.parse().expect(ip);
            let err = check_ip_not_private(parsed, "https://example.com").unwrap_err();
            assert_eq!(err.kind(), "ssrf_blocked", "{ip}");
        }
    }
}
```

- [ ] **Step 3: Remove marketplace-owned validation**

In `crates/lab/src/dispatch/marketplace/mcp_params.rs`, delete the existing URL validation functions and related `std::net::IpAddr` import:

```rust
pub fn validate_registry_url(url: &str) -> Result<(), ToolError>
fn check_ip_not_private(ip: IpAddr, url: &str) -> Result<(), ToolError>
```

Keep runtime-hint, stdio argv/env, search, and metadata helpers intact.

- [ ] **Step 4: Re-export the shared validator for compatibility**

In `crates/lab/src/dispatch/marketplace.rs`, add:

```rust
pub use crate::dispatch::security::ssrf::validate_external_https_url as validate_registry_url;
```

Keep:

```rust
pub use mcp_params::resolve_search_for_rest;
```

- [ ] **Step 5: Update gateway probe**

In `crates/lab/src/dispatch/gateway/oauth_lifecycle/probe.rs`, replace the marketplace validator call with:

```rust
crate::dispatch::security::ssrf::validate_external_https_url(&canonical_url).await?;
```

Remove the local `spawn_blocking` wrapper around that call.

- [ ] **Step 6: Audit marketplace call sites**

Run:

```bash
rg -n "validate_registry_url|validate_external_https_url|ToSocketAddrs" crates/lab/src/dispatch/marketplace crates/lab/src/dispatch/gateway
```

Every async dispatch/request path must call `validate_external_https_url(...).await`. Direct calls to `validate_external_https_url_blocking` are allowed only in synchronous tests or synchronous helper code.

- [ ] **Step 7: Run focused checks**

Run:

```bash
cargo test -p labby --lib --no-default-features --features gateway dispatch::security::ssrf
cargo check -p labby --no-default-features --features gateway --all-targets
```

Expected: both pass.

- [ ] **Step 8: Commit**

Run:

```bash
git add crates/lab/src/dispatch.rs crates/lab/src/dispatch/security.rs crates/lab/src/dispatch/security/ssrf.rs crates/lab/src/dispatch/marketplace.rs crates/lab/src/dispatch/marketplace/mcp_params.rs crates/lab/src/dispatch/marketplace/mcp_dispatch.rs crates/lab/src/dispatch/gateway/oauth_lifecycle/probe.rs scripts/check-feature-slices.sh crates/lab/Cargo.toml
git commit -m "refactor: share external url ssrf preflight"
```

Expected: commit succeeds after the focused checks pass.

### Task 3: Gate CLI Boundaries Without Breaking Scaffold Markers

**Files:**
- Modify: `crates/lab/src/cli.rs`
- Modify: `crates/lab/src/cli/serve.rs`
- Modify: `crates/lab/src/cli/helpers.rs`
- Modify: `crates/lab/src/cli/health.rs`

- [ ] **Step 1: Gate feature-backed CLI modules, variants, and dispatch arms**

In `crates/lab/src/cli.rs`, gate only modules and commands whose backing dispatch modules are feature-gated:

```rust
#[cfg(feature = "gateway")]
pub mod gateway;
#[cfg(feature = "marketplace")]
pub mod marketplace;
#[cfg(feature = "mcpregistry")]
pub mod mcpregistry;
#[cfg(feature = "deploy")]
pub mod deploy;
```

Apply matching `#[cfg(...)]` attributes to the `Command` enum variants and `dispatch` match arms.

Preserve these scaffold markers exactly where they are:

```rust
// [lab-scaffold: cli-modules]
// [lab-scaffold: cli-variants]
// [lab-scaffold: cli-dispatch]
```

- [ ] **Step 2: Keep serve core usable without gateway**

In `crates/lab/src/cli/serve.rs`, group gateway setup into gateway-gated helpers instead of scattering inline `cfg`s through unrelated code. The helpers must follow this shape:

```rust
#[cfg(feature = "gateway")]
async fn build_gateway_runtime(
    config: &crate::config::LabConfig,
    auth_config: &Option<lab_auth::config::AuthConfig>,
) -> anyhow::Result<std::sync::Arc<crate::dispatch::gateway::manager::GatewayManager>> {
    // Move the existing GatewayManager, UpstreamPool, OAuth runtime, notifier,
    // seed_config, discover, and auto-import setup here without changing behavior.
}

#[cfg(not(feature = "gateway"))]
fn reject_protected_routes_without_gateway(config: &crate::config::LabConfig) -> anyhow::Result<()> {
    if !config.protected_mcp_routes.is_empty() {
        anyhow::bail!(
            "protected MCP routes are configured but this labby build does not include the gateway feature"
        );
    }
    Ok(())
}
```

Use the existing config field names from `LabConfig`. If the protected route field name differs, use the field currently read by `ProtectedMcpRouteEffectiveTarget`.

- [ ] **Step 3: Gate CLI helper imports**

Run:

```bash
rg -n "dispatch::gateway|dispatch::upstream|GatewayManager|UpstreamPool" crates/lab/src/cli
```

For each hit outside a `#[cfg(feature = "gateway")]` item, either gate the item or return a clear `gateway_unavailable` error in no-gateway builds. Do not leave hidden panics.

- [ ] **Step 4: Keep mcpregistry dependency-owned**

Do not make `mcpregistry` a standalone CLI product slice. If `cli/health.rs` imports marketplace internals under `#[cfg(feature = "mcpregistry")]`, change the guard to:

```rust
#[cfg(feature = "marketplace")]
```

or move the import into a marketplace-gated function.

- [ ] **Step 5: Run CLI checks**

Run:

```bash
cargo check -p labby --no-default-features --all-targets
cargo check -p labby --no-default-features --features gateway --all-targets
cargo check -p labby --no-default-features --features marketplace --all-targets
```

Expected: no unresolved symbols from `crate::cli::gateway`, `crate::cli::marketplace`, `crate::dispatch::gateway`, `crate::dispatch::upstream`, or `crate::dispatch::marketplace`.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/lab/src/cli.rs crates/lab/src/cli/serve.rs crates/lab/src/cli/helpers.rs crates/lab/src/cli/health.rs
git commit -m "refactor: gate cli feature slices"
```

Expected: commit succeeds.

### Task 4: Gate API Boundaries And Preserve Route Security

**Files:**
- Modify: `crates/lab/src/api.rs`
- Modify: `crates/lab/src/api/state.rs`
- Modify: `crates/lab/src/api/services.rs`
- Modify: `crates/lab/src/api/services/helpers.rs`
- Modify: `crates/lab/src/api/health.rs`
- Modify: `crates/lab/src/api/router.rs`

- [ ] **Step 1: Gate gateway-only API modules**

In `crates/lab/src/api.rs`, gate:

```rust
#[cfg(feature = "gateway")]
pub mod upstream_oauth;
```

Keep always-on health, oauth, browser session, nodes, web, host validation, and services modules unchanged unless compile errors prove they directly require gateway.

- [ ] **Step 2: Gate API state fields**

In `crates/lab/src/api/state.rs`, gate the gateway manager field and builder:

```rust
#[cfg(feature = "gateway")]
pub gateway_manager: Option<Arc<crate::dispatch::gateway::manager::GatewayManager>>,
```

```rust
#[cfg(feature = "gateway")]
#[must_use]
#[allow(dead_code)]
pub fn with_gateway_manager(
    mut self,
    manager: Arc<crate::dispatch::gateway::manager::GatewayManager>,
) -> Self {
    self.gateway_manager = Some(manager);
    self
}
```

Gate initializer fields with the same `#[cfg(feature = "gateway")]`.

Keep `registry_store` under `#[cfg(feature = "mcpregistry")]` only if `dispatch::marketplace::store` remains compiled whenever `mcpregistry` is enabled. Otherwise change the guard to `#[cfg(feature = "marketplace")]` and remove standalone `mcpregistry` expectations.

- [ ] **Step 3: Gate route modules**

In `crates/lab/src/api/services.rs`, gate:

```rust
#[cfg(feature = "gateway")]
pub mod gateway;
#[cfg(feature = "marketplace")]
pub mod marketplace;
#[cfg(feature = "mcpregistry")]
pub mod registry_v01;
#[cfg(feature = "fs")]
pub mod fs;
```

If `registry_v01` imports marketplace store/client types, use:

```rust
#[cfg(feature = "marketplace")]
pub mod registry_v01;
```

and keep `/v0.1` registry routes marketplace-owned.

- [ ] **Step 4: Group gateway router helpers**

In `crates/lab/src/api/router.rs`, avoid many small inline `cfg` blocks. Add gateway-gated helpers for the gateway-owned route families:

```rust
#[cfg(feature = "gateway")]
fn mount_gateway_routes(mut v1: Router<AppState>, state: &AppState, api_auth_configured: bool) -> Router<AppState> {
    if api_auth_configured
        && state
            .registry
            .services()
            .iter()
            .any(|service| service.name == "gateway")
    {
        v1 = v1.nest("/gateway", services::gateway::routes(state.clone()));
    } else if !api_auth_configured {
        tracing::warn!(
            subsystem = "startup",
            phase = "gateway.mount.skipped",
            reason = "no_auth_configured",
            "gateway service routes not mounted: gateway admin actions require API auth"
        );
    }
    v1
}
```

Add similar gateway-gated helpers for upstream OAuth browser routes, protected-resource metadata, and protected MCP route interception. Preserve the current middleware order: protected-route interception must remain outside state injection and before static fallback, matching the existing order.

- [ ] **Step 5: Preserve marketplace host validation**

When mounting marketplace routes, keep the existing host-validation layer:

```rust
#[cfg(feature = "marketplace")]
fn mount_marketplace_routes(mut v1: Router<AppState>, state: &AppState) -> Router<AppState> {
    if state
        .registry
        .services()
        .iter()
        .any(|service| service.name == "marketplace")
    {
        v1 = v1.nest(
            "/marketplace",
            services::marketplace::routes(state.clone()).layer(axum::middleware::from_fn(
                crate::api::host_validation::host_validation_layer,
            )),
        );
    }
    v1
}
```

- [ ] **Step 6: Gate API helper modules**

Run:

```bash
rg -n "gateway_manager|dispatch::gateway|dispatch::upstream|SHARED_GATEWAY_OAUTH_SUBJECT|configured_bearer_token" crates/lab/src/api
```

Every hit must be inside a `#[cfg(feature = "gateway")]` item or have a no-gateway fallback that does not advertise gateway/protected-route support.

- [ ] **Step 7: Add route-security regression tests**

In the existing router test module in `crates/lab/src/api/router.rs`, add or preserve tests with these names:

```rust
#[cfg(feature = "gateway")]
#[tokio::test]
async fn gateway_routes_do_not_mount_without_api_auth() {
    // Build a state with gateway registered and a GatewayManager.
    // Build router with no bearer token and no OAuth state.
    // Request POST /v1/gateway.
    // Assert status is NOT 200 and the gateway dispatch handler was not reached.
}

#[cfg(feature = "marketplace")]
#[tokio::test]
async fn marketplace_routes_reject_untrusted_host_header() {
    // Build a marketplace-enabled router.
    // Request POST /v1/marketplace with Host: attacker.example.
    // Assert status is 421 Misdirected Request.
}
```

Use existing router test helpers for request construction. Do not introduce new auth bypasses to make the tests pass.

- [ ] **Step 8: Run API checks**

Run:

```bash
cargo check -p labby --no-default-features --all-targets
cargo test -p labby --lib --no-default-features --features gateway api::router::gateway_routes_do_not_mount_without_api_auth
cargo test -p labby --lib --no-default-features --features marketplace api::router::marketplace_routes_reject_untrusted_host_header
```

Expected: all pass.

- [ ] **Step 9: Commit**

Run:

```bash
git add crates/lab/src/api.rs crates/lab/src/api/state.rs crates/lab/src/api/services.rs crates/lab/src/api/services/helpers.rs crates/lab/src/api/health.rs crates/lab/src/api/router.rs
git commit -m "refactor: gate api feature slices safely"
```

Expected: commit succeeds.

### Task 5: Gate Registry And Parity Tests

**Files:**
- Modify: `crates/lab/src/registry.rs`

- [ ] **Step 1: Gate registry entries by compiled dispatch ownership**

In `build_registry`, wrap gateway registration in:

```rust
#[cfg(feature = "gateway")]
reg.register(RegisteredService {
    name: "gateway",
    description: "Manage proxied upstream MCP gateways",
    category: "bootstrap",
    kind: RegisteredServiceKind::BootstrapOperator,
    status: "available",
    actions: crate::dispatch::gateway::ACTIONS,
    dispatch: dispatch_fn!(crate::dispatch::gateway::dispatch),
});
```

Wrap marketplace registration in:

```rust
#[cfg(feature = "marketplace")]
{
    let meta = lab_apis::marketplace::META;
    reg.register(RegisteredService {
        name: meta.name,
        description: meta.description,
        category: category_slug(meta.category),
        kind: registered_service_kind(meta.name, meta.category),
        status: "available",
        actions: crate::dispatch::marketplace::actions(),
        dispatch: dispatch_fn!(crate::dispatch::marketplace::dispatch),
    });
}
```

Do not register `mcpregistry` as a separate service in this plan.

- [ ] **Step 2: Update registry/router parity test expectations**

In `registry_and_router_service_sets_are_identical`, gate the expected HTTP service set with the same features used by `api/router.rs`:

```rust
#[cfg(feature = "gateway")]
s.insert("gateway");
#[cfg(feature = "marketplace")]
s.insert("marketplace");
#[cfg(feature = "fs")]
s.insert("fs");
```

Keep existing exemptions only for services that genuinely have no HTTP route.

- [ ] **Step 3: Add feature-gated registry tests**

Add tests:

```rust
#[cfg(not(feature = "gateway"))]
#[test]
fn default_registry_omits_gateway_without_feature() {
    let registry = build_default_registry();
    assert!(registry.services().iter().all(|service| service.name != "gateway"));
}

#[cfg(feature = "gateway")]
#[test]
fn default_registry_includes_gateway_with_feature() {
    let registry = build_default_registry();
    assert!(registry.services().iter().any(|service| service.name == "gateway"));
}

#[cfg(not(feature = "marketplace"))]
#[test]
fn default_registry_omits_marketplace_without_feature() {
    let registry = build_default_registry();
    assert!(registry.services().iter().all(|service| service.name != "marketplace"));
}

#[cfg(feature = "marketplace")]
#[test]
fn default_registry_includes_marketplace_with_feature() {
    let registry = build_default_registry();
    assert!(registry.services().iter().any(|service| service.name == "marketplace"));
}
```

- [ ] **Step 4: Run registry checks**

Run:

```bash
cargo test -p labby --lib --no-default-features registry
cargo test -p labby --lib --no-default-features --features gateway registry
cargo test -p labby --lib --no-default-features --features marketplace registry
```

Expected: all pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/lab/src/registry.rs
git commit -m "refactor: gate registry by compiled services"
```

Expected: commit succeeds.

### Task 6: Gate MCP Gateway Adapters And Capabilities

**Files:**
- Modify: `crates/lab/src/mcp.rs`
- Modify: `crates/lab/src/mcp/server.rs`
- Modify: `crates/lab/src/mcp/call_tool.rs`
- Modify: `crates/lab/src/mcp/catalog.rs`
- Modify: `crates/lab/src/mcp/context.rs`
- Modify: `crates/lab/src/mcp/handlers_prompts.rs`
- Modify: `crates/lab/src/mcp/handlers_resources.rs`
- Modify: `crates/lab/src/mcp/handlers_tools.rs`
- Modify additional `crates/lab/src/mcp/*.rs` files only when the feature script reports missing gateway/upstream symbols.

- [ ] **Step 1: Gate gateway-specific module declarations**

In `crates/lab/src/mcp.rs`, gate modules that are unusable without gateway:

```rust
#[cfg(feature = "gateway")]
pub mod call_tool_codemode;
#[cfg(feature = "gateway")]
pub mod call_tool_upstream;
#[cfg(feature = "gateway")]
pub mod in_process_peer;
#[cfg(feature = "gateway")]
pub mod peers;
#[cfg(feature = "gateway")]
pub mod resource_proxy;
#[cfg(feature = "gateway")]
pub mod upstream;
```

Keep `route_scope` compiled if non-gateway MCP code uses it for local-service filtering; otherwise gate it with gateway too.

- [ ] **Step 2: Gate MCP server gateway fields**

In `crates/lab/src/mcp/server.rs`, gate `GatewayManager` imports and fields:

```rust
#[cfg(feature = "gateway")]
pub gateway_manager: Option<Arc<GatewayManager>>,
```

Constructors and tests must have matching gateway/no-gateway forms. In no-gateway builds, server construction must not require a manager or route-scoped upstream state.

- [ ] **Step 3: Gate MCP Apps UI capability advertisement**

In `LabMcpServer::get_info()` or the capability builder it calls, advertise the MCP Apps UI extension only when gateway is enabled:

```rust
#[cfg(feature = "gateway")]
{
    // Existing io.modelcontextprotocol/ui extension advertisement.
}
```

In no-gateway builds, do not advertise Code Mode or UI resources that cannot be served.

- [ ] **Step 4: Preserve route-scope and callback security**

For gateway builds, keep the existing checks for:

```rust
self.route_scope.exposes_code_mode()
self.route_scope.allowed_upstreams()
self.route_scope.allows_service(...)
self.route_scope.allows_upstream(...)
```

Do not replace these with unconditional `true` fallbacks. No-gateway builds should omit upstream/Code Mode behavior rather than allow it.

- [ ] **Step 5: Add targeted MCP regression tests**

Add or preserve gateway-feature tests proving:

```rust
#[cfg(feature = "gateway")]
#[tokio::test]
async fn code_mode_raw_tools_remain_hidden_when_route_scope_denies_code_mode() {
    // Request search or execute through a protected route scope that denies Code Mode.
    // Assert route_scope_denied.
}

#[cfg(feature = "gateway")]
#[tokio::test]
async fn destructive_mcp_app_callbacks_remain_blocked() {
    // Invoke the existing destructive widget callback path.
    // Assert it returns the existing denial kind rather than dispatching.
}

#[cfg(feature = "gateway")]
#[tokio::test]
async fn protected_route_resource_reads_require_authorized_scope() {
    // Attempt a UI resource read without an allowed upstream/scope.
    // Assert authorization failure or route_scope_denied.
}
```

Use existing MCP test helpers where available.

- [ ] **Step 6: Run MCP checks**

Run:

```bash
cargo check -p labby --no-default-features --all-targets
cargo test -p labby --lib --no-default-features --features gateway mcp
cargo check -p labby --no-default-features --features marketplace --all-targets
```

Expected: no missing gateway/upstream symbols in non-gateway builds; gateway MCP tests pass.

- [ ] **Step 7: Commit**

Run:

```bash
git add crates/lab/src/mcp.rs crates/lab/src/mcp/server.rs crates/lab/src/mcp/call_tool.rs crates/lab/src/mcp/catalog.rs crates/lab/src/mcp/context.rs crates/lab/src/mcp/handlers_prompts.rs crates/lab/src/mcp/handlers_resources.rs crates/lab/src/mcp/handlers_tools.rs
git commit -m "refactor: gate mcp gateway adapters"
```

Expected: commit succeeds.

### Task 7: Close Remaining Product Slice Compile Failures

**Files:**
- Modify files reported by `scripts/check-feature-slices.sh`.

- [ ] **Step 1: Run the full local feature script**

Run:

```bash
scripts/check-feature-slices.sh
```

Expected: either PASS or fail on the first remaining supported product slice.

- [ ] **Step 2: Fix one failing slice at a time**

For unresolved imports, apply one of these patterns:

```rust
#[cfg(feature = "gateway")]
use crate::dispatch::gateway;
```

```rust
#[cfg(feature = "marketplace")]
use crate::dispatch::marketplace;
```

```rust
#[cfg(feature = "fs")]
use crate::dispatch::fs;
```

or gate the whole item:

```rust
#[cfg(feature = "deploy")]
pub mod deploy;
```

If a failure reveals a helper/internal feature such as `systemd`, `node-runtime`, or `test-utils`, do not widen the acceptance contract. Fix it only if the change is local and harmless; otherwise record it as out of scope in the final notes.

- [ ] **Step 3: Re-run the failing slice**

Run the exact failing command from the script. Example:

```bash
cargo check -p labby --no-default-features --features fs --all-targets
```

Expected: the targeted slice passes before moving on.

- [ ] **Step 4: Repeat until the script passes**

Run:

```bash
scripts/check-feature-slices.sh
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/lab/src scripts/check-feature-slices.sh crates/lab/Cargo.toml
git commit -m "refactor: support product feature slices"
```

Expected: commit succeeds.

### Task 8: Refresh Generated Docs And Final Verification

**Files:**
- Modify generated docs only if `docs generate` changes them.

- [ ] **Step 1: Regenerate docs with all features**

Run:

```bash
cargo run -p labby --all-features -- docs generate
```

Expected: generated docs reflect gateway and marketplace under default/all builds.

- [ ] **Step 2: Check docs**

Run:

```bash
cargo run -p labby --all-features -- docs check
```

Expected: PASS.

- [ ] **Step 3: Run final verification**

Run:

```bash
scripts/check-feature-slices.sh
cargo fmt --all -- --check
cargo clippy -p labby --all-features --all-targets -- -D warnings
```

Expected: all pass.

- [ ] **Step 4: Commit**

Run:

```bash
git add docs/generated crates/lab/src crates/lab/Cargo.toml scripts/check-feature-slices.sh
git commit -m "docs: refresh feature slice surfaces"
```

Expected: commit succeeds if files changed. If no files changed, do not create an empty commit.

### Task 9: Optional Follow-Up CI Enforcement

**Files:**
- Modify: `.github/workflows/ci.yml` or the workflow file that owns Rust checks.

This task is optional and should be done only if the user explicitly wants CI enforcement in this branch.

- [ ] **Step 1: Locate the Rust CI workflow**

Run:

```bash
rg -n "rust-toolchain|cargo check|cargo clippy|nextest|rustfmt|1\\.94\\.1" .github/workflows
```

Expected: output identifies the existing Rust workflow and pinned toolchain.

- [ ] **Step 2: Add a PR smoke job using the repo-pinned toolchain**

Add a PR job or step that runs the local script only after the main Rust cache/toolchain setup. Use the same Rust toolchain version as the existing workflow; do not replace it with `stable`.

```yaml
      - name: Check supported feature slices
        run: scripts/check-feature-slices.sh
```

Do not add a separate exhaustive matrix in PR CI for this plan.

- [ ] **Step 3: Add scheduled exhaustive audit only if desired**

If a scheduled workflow already exists, add the script there. If not, create no new schedule unless explicitly requested.

- [ ] **Step 4: Run YAML sanity check**

Run:

```bash
yamllint .github/workflows || true
```

Expected: no syntax errors if `yamllint` is installed.

- [ ] **Step 5: Commit**

Run:

```bash
git add .github/workflows scripts/check-feature-slices.sh
git commit -m "ci: check supported cargo feature slices"
```

Expected: commit succeeds if workflow files changed. If CI enforcement is skipped, do not commit anything for this task.

## Failure Modes Table

| Codepath | Failure Mode | Rescued? | Test? | User Sees? | Logged? |
|---|---|---:|---:|---|---:|
| Feature script | CI/local checks become too slow from repeated heavy builds | Y | Y | Visible failure/timeout | Y |
| Shared SSRF helper | Slow DNS blocks runtime or DNS rebinding bypasses validation-time preflight | Y | Y | Visible validation error or blocked probe | Y |
| CLI gates | No-default binary omits commands operators expected | Y | Y | Visible missing command | N/A |
| Serve without gateway | Protected routes configured but gateway feature absent | Y | Y | Startup error | Y |
| API gateway route | `/v1/gateway` mounts without auth | Y | Y | Route unavailable without auth | Y |
| API marketplace route | Host-validation layer is lost | Y | Y | 421 for bad Host | Y |
| Registry gates | Catalog and router drift | Y | Y | Missing/extra command surface | Y |
| MCP gates | Non-gateway server advertises Code Mode/MCP Apps UI it cannot serve | Y | Y | Capability absent | N/A |
| MCP passthrough | Route-scope or destructive callback checks are bypassed | Y | Y | Denial envelope | Y |

No critical gap remains after the review updates: every security-sensitive new codepath has a rescue behavior and a named test requirement.

## Deferrable Work

- Full connect-time DNS pinning or a centralized safe outbound HTTP client for all user-supplied URLs.
- Splitting marketplace into `marketplace-core` and registry/install subfeatures.
- Standalone `mcpregistry` support.
- Standalone helper/internal feature support for `systemd`, `node-runtime`, and `test-utils`.
- Runtime smoke tests for every feature slice.
- CI enforcement in this branch, unless explicitly requested.

## Self-Review

Spec coverage:
- The plan supports real product feature slices and keeps all-features authoritative.
- All engineering review critical/high findings are now represented as tasks or non-negotiable invariants.
- `mcpregistry` ownership is explicitly resolved by removing it from standalone support for this plan.
- Security-sensitive route, host-validation, protected-route, SSRF, and MCP passthrough behaviors have named tests.

Placeholder scan:
- No placeholder markers, deferred implementation notes, or unnamed file targets remain.
- Large-file steps use exact invariants plus exact commands, avoiding brittle copy/paste snippets where the current code is too active for safe wholesale replacement.

Type consistency:
- Shared URL validation is named `validate_external_https_url` / `validate_external_https_url_blocking`.
- Marketplace keeps a compatibility re-export named `validate_registry_url`.
- Gateway manager references are only valid under `feature = "gateway"`.
- Marketplace-owned registry behavior is only valid under `feature = "marketplace"` for this plan.
