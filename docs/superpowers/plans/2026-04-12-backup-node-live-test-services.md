# Backup Node Live Test Services Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a reproducible live end-to-end test environment on SSH host `backup-node` that provisions disposable real service stacks from ZFS golden snapshots, runs catalog-driven CLI/MCP/API coverage, and records structured artifacts.

**Architecture:** The implementation splits into four layers: declarative fixture definitions in-repo, a host-side orchestration contract executed over SSH on `backup-node`, a repo-side live runner that requests environments and executes test matrices, and catalog-driven case generation plus reporting. All 15 services are covered across 7 profiles (`servarr-core`, `media`, `download`, `notes`, `notifications`, `ai`, `all`). The first vertical slice targets `servarr-core` so the environment model, manifest contract, teardown, and one full-surface execution path are proven before broader onboarding — but all fixture definitions and host provisioning logic are built for the full 15-service set from the start.

**Tech Stack:** Rust 2024 (`lab` crate), `serde`/`serde_json`, `tokio`, `tracing`, existing CLI/MCP/API surfaces, `just`, SSH, Docker or Docker Compose on `backup-node`, ZFS snapshots/clones, shell helpers in `bin/` where appropriate.

---

## Backup Node Host Reconnaissance (2026-04-14)

### What we found

| Item | Reality |
|------|---------|
| **OS** | Unraid OS 7.2 (Slackware-based) |
| **User** | root (uid=0), sudo available |
| **ZFS pool** | `backup` (7.27T, 5.38T free, mirror-0 healthy) — **NOT `tank`** |
| **ZFS version** | zfs-2.3.4-1 |
| **Docker** | 27.5.1, Compose v2.40.3 |
| **jq** | 1.6 |
| **Network** | LAN 10.1.0.3/24, Tailscale 100.64.0.50, Docker 172.17.0.1/16 |
| **PATH** | `/usr/local/sbin:/usr/sbin:/sbin:/usr/local/bin:/usr/bin:/bin` — `~/.local/bin` exists and IS in PATH |
| **Existing containers** | arcane-agent, dockersocket, portainer_agent only |
| **Existing backups** | syncoid backups from `node-b` and `controller` (production data — do NOT use for testing) |

### Golden snapshots — CREATED

Golden instances were stood up from fresh linuxserver images, verified via API, then stopped and snapshotted:

| Service | Image | Version | Dataset | Snapshot | Auth / API Key | Container Port |
|---------|-------|---------|---------|----------|----------------|---------------|
| **Servarr** | | | | | | |
| Radarr | `lscr.io/linuxserver/radarr:latest` | 6.1.1.10360 | `backup/lab/live/golden/radarr` | `@configured-v1` | API Key: `e1aa7e5c555642abba060cd9466cc24d` | 7878 |
| Sonarr | `lscr.io/linuxserver/sonarr:latest` | 4.0.17.2952 | `backup/lab/live/golden/sonarr` | `@configured-v1` | API Key: `0be6971b8ce348a29beef1efa75564db` | 8989 |
| Prowlarr | `lscr.io/linuxserver/prowlarr:latest` | 2.3.5.5327 | `backup/lab/live/golden/prowlarr` | `@configured-v1` | API Key: `c73bebf7b160405f9d386e5527369271` | 9696 |
| **Media** | | | | | | |
| Plex | `lscr.io/linuxserver/plex:latest` | 1.43.1.10611 | `backup/lab/live/golden/plex` | `@configured-v1` | Unclaimed (no auth). `/identity` works, library endpoints 401. | 32400 |
| Tautulli | `ghcr.io/hotio/tautulli:testing` | 2.17.0 | `backup/lab/live/golden/tautulli` | `@configured-v1` | API Key: `9aa536fa86024d42af45d774c10763db` | 8181 |
| Overseerr | `ghcr.io/hotio/overseerr:release` | 1.34.0 | `backup/lab/live/golden/overseerr` | `@configured-v1` | Setup-pending (needs Plex account). `/api/v1/status` works. | 5055 |
| **Download** | | | | | | |
| SABnzbd | `ghcr.io/hotio/sabnzbd:latest` | 4.5.5 | `backup/lab/live/golden/sabnzbd` | `@configured-v1` | API Key: `73813a0364534c7bad5a3266f23fcb49`. `inet_exposure=4`, wizard bypassed. | 8080 |
| qBittorrent | `linuxserver/qbittorrent:latest` | 5.1.4 | `backup/lab/live/golden/qbittorrent` | `@configured-v1` | user: `admin`, pass: `lab-test-golden`. Login via cookie auth. | 8080 |
| **Notes/Docs** | | | | | | |
| Memos | `ghcr.io/usememos/memos` | 0.24.0 | `backup/lab/live/golden/memos` | `@configured-v1` | Bearer token (JWT access token, no expiry). User: `admin`. | 5230 |
| Linkding | `sissbruecker/linkding` | latest | `backup/lab/live/golden/linkding` | `@configured-v1` | Token via `ApiToken` model. User: `admin`, pass: `lab-test-golden`. Env: `LD_SUPERUSER_NAME`. | 9090 |
| Bytestash | `ghcr.io/jordan-dalby/bytestash:latest` | latest | `backup/lab/live/golden/bytestash` | `@configured-v1` | JWT auth. User: `admin`, pass: `lab-test-golden`. `JWT_SECRET=test-golden-secret-key-for-lab`. | 5000 |
| Paperless | `ghcr.io/paperless-ngx/paperless-ngx` | latest | `backup/lab/live/golden/paperless` | `@configured-v1` | Token: `25a9e0b34ca3485d620783a9bcc7f7a7febff339`. User: `admin`, pass: `lab-test-golden`. **Needs Redis sidecar.** | 8000 |
| **Notifications** | | | | | | |
| Gotify | `gotify/server:latest` | latest | `backup/lab/live/golden/gotify` | `@configured-v1` | Default user: `admin`/`admin`. App token: `AjoHfnf2U3AOJ5d`. | 80 |
| Apprise | `caronc/apprise:latest` | 1.3.3 | `backup/lab/live/golden/apprise` | `@configured-v1` | No auth (stateless). `/status` returns `OK`. | 8000 |
| **AI** | | | | | | |
| Qdrant | `qdrant/qdrant:latest` | latest | `backup/lab/live/golden/qdrant` | `@configured-v1` | No auth. `/healthz` and `/collections` work. | 6333 (HTTP), 6334 (gRPC) |

**Common config:** `PUID=99`, `PGID=100`, `TZ=America/New_York` where applicable.

**Services NOT included** (not containerizable or external APIs): Extract (synthetic, no container), OpenAI (external API), TEI (stateless inference), Tailscale (network-level), UniFi (appliance), Unraid (host OS).

**Sidecar note:** Paperless requires a Redis container. The `bin/live-host` script must start `redis:7-alpine` alongside paperless and pass the Redis IP via `PAPERLESS_REDIS` env var.

**Services with limited API in golden state:**
- **Plex** — unclaimed, only `/identity` responds. Full API requires a Plex account claim.
- **Overseerr** — setup-pending, redirects to `/setup`. Only `/api/v1/status` responds without Plex auth.
- **Linkding** — token must be regenerated via `ApiToken` model after each clone start (not DRF `Token`).
- **Memos** — access tokens are JWT-based, created via `/api/v1/users/1/access_tokens` with session cookie.

**Readiness endpoints** — used by `wait_for_services()` (no auth needed for readiness checks). All verified against golden snapshots on 2026-04-14:

| Service | Readiness URL | Expected | Volume Mount |
|---------|--------------|----------|--------------|
| Radarr | `/ping` | 200 | `/config` |
| Sonarr | `/ping` | 200 | `/config` |
| Prowlarr | `/ping` | 200 | `/config` |
| Plex | `/identity` | 200 | `/config` |
| Tautulli | `/status` | 200 | `/config` |
| Overseerr | `/api/v1/status` | 200 | `/config` |
| SABnzbd | `/sabnzbd/api?mode=version` | 200 | `/config` |
| qBittorrent | `/api/v2/app/version` | 200 | `/config` |
| Memos | `/healthz` | 200 | `/var/opt/memos` |
| Linkding | `/health` | 200 | `/etc/linkding/data` |
| Bytestash | `/` | 200 | `/data/snippets` |
| Paperless | `/api/` | 200 or 302 | `/usr/src/paperless/data` |
| Gotify | `/health` | 200 | `/app/data` |
| Apprise | `/status` | 200 | `/config` |
| Qdrant | `/healthz` | 200 | `/qdrant/storage` |

**Note:** Paperless returns 302 (redirect to login) for `/api/` — accept both 200 and 302 as ready. The readiness poller in `wait_for_services()` must use `resp.status().is_success() || resp.status() == 302` for paperless.

### Clone lifecycle — VERIFIED

Full round-trip proven:
1. `zfs clone backup/lab/live/golden/radarr@configured-v1 backup/lab/live/runs/test-001/radarr` — instant
2. `docker run ... -v /mnt/backup/lab/live/runs/test-001/radarr:/config` — starts in <3s
3. API responds on first poll with same API key from golden snapshot
4. `docker stop + rm` → `sync` → `zfs destroy` — clean, no leaks

### Plan corrections required

1. **Pool name:** Original plan referenced `tank/lab/live/...` everywhere — corrected to `backup/lab/live/...`. The `tank` pool does not exist on backup-node.
2. **Images:** Plan references `lscr.io/linuxserver/radarr:latest` generically. Actual images are `lscr.io/linuxserver/{radarr,sonarr,prowlarr}:latest` (hotio is what controller uses for radarr, but linuxserver is fine for golden instances).
3. **Prowlarr config path:** Prowlarr's config.xml lives at `/config/config.xml` (not nested in `/config/prowlarr/` like the controller backup layout). The linuxserver image uses `/config` directly.
4. **Auth:** Golden instances have `AuthenticationMethod=None` — API key auth works but no forms login. This is ideal for testing (no login flow to deal with).
5. **`--internal` network is INCOMPATIBLE with `-p` port mapping.** Docker internal networks block host port binding entirely — containers get no `Ports` entries. **Fix:** Use a regular bridge network (`docker network create lab-live-$RUN_ID` without `--internal`). Loopback binding (`127.0.0.1:0:<port>`) already prevents LAN exposure. Tested and confirmed.
6. **No `tank` pool** means `bin/live-host` ZFS paths must all use `backup/lab/live/...` prefix.
7. **`docker port` for port extraction.** `docker inspect --format` with Go templates fails when ports aren't mapped. Use `docker port <name> <port>/tcp | cut -d: -f2` instead — simpler and reliable.
8. **SSH automation:** backup-node is already in `~/.ssh/known_hosts` (ed25519 + rsa). SSH config maps `backup-node` → `User root, HostName backup-node`. BatchMode works. The plan's `~/.labby/known_hosts` approach is unnecessary — standard known_hosts is fine.
9. **Bash on backup-node:** 5.3.3, supports arrays, `timeout` (coreutils 9.8), `grep -E` regex — all script features work.
10. **Dynamic ports start at 32768** (kernel ephemeral range). Confirmed `127.0.0.1:0:<port>` allocates correctly.
11. **Startup latency:** All 3 services ready in ~6s from `docker run` (one failed poll, second succeeds). Readiness timeout of 120s is very conservative.

### Full lifecycle timing (verified)

| Phase | Duration |
|-------|----------|
| ZFS clone (3 services) | <1s |
| Docker network create | <1s |
| Docker run (3 containers) | ~2s |
| Readiness polling | ~6s (1 failed poll + 1 success at 3s interval) |
| API verification | <1s |
| Teardown (stop + rm + zfs destroy) | ~10s |
| **Total round-trip** | **~20s** |

---

## File Structure

### New Files

- `crates/lab/src/cli/live.rs`
  New CLI entrypoint for live environment lifecycle and test execution.
- `crates/lab/src/live.rs`
  Module declaration for live test infrastructure in the `lab` crate.
- `crates/lab/src/live/types.rs`
  Shared manifest, fixture definition, case definition, and result types.
- `crates/lab/src/live/config.rs`
  In-repo loading and validation for fixture definitions and profiles.
- `crates/lab/src/live/host.rs`
  Module declaration for the host client package.
- `crates/lab/src/live/host/connection.rs`
  SSH subprocess execution via `tokio::process::Command` — the only place SSH args are constructed.
- `crates/lab/src/live/host/validation.rs`
  Input validation for `run_id` and `profile` against `^[a-z0-9][a-z0-9_-]{0,63}$`. Single source of truth — the bash script's `validate_id()` must match this exact regex (add a `# SYNC: must match crates/lab/src/live/host/validation.rs:LIVE_ID_PATTERN` comment in `bin/live-host`).
- `crates/lab/src/live/host/readiness.rs`
  Parallel readiness polling: `wait_for_services()` uses `try_join_all` — never a sequential loop. See Task 4.
- `crates/lab/src/live/host/manifest.rs`
  Manifest parsing and `RunGuard` struct. `RunGuard` wraps a run_id and host; its `Drop` impl calls `live-host down <run_id>` to guarantee ZFS clone teardown under panic or early return. See Task 4.
- `crates/lab/src/live/runner.rs`
  Executes live cases against CLI/MCP/API surfaces and records structured results.
- `crates/lab/src/live/report.rs`
  Writes machine-readable artifacts and human-readable summaries with incremental checkpoint support.
- `crates/lab/src/live/CLAUDE.md`
  Hard rules for this module: `tokio::process::Command` only (never `std::process`), `kill_on_drop(true)` on every SSH child, SSH ConnectTimeout + ServerAliveInterval on every invocation. Violations block merge.
- `crates/lab/tests/live_config.rs`
  CI-safe tests for fixture/profile/manifest parsing and validation.
- `crates/lab/tests/live_runner.rs`
  CI-safe tests for manifest handling, case execution plumbing, and report shaping with mocked host responses.

> **Deferred to Task 10:** `live/catalog.rs`, `live/matrix.rs`, `tests/live_catalog.rs`, `tests/live_matrix.rs` — catalog-driven gap analysis and matrix classification belong after the runner is proven end-to-end. Adding them before Task 9 adds ~390 LOC with no milestone impact.
- `tests/live_host_contract_test.sh`
  Opt-in live host contract smoke test against `backup-node`.
- `tests/live_servarr_core_e2e_test.sh`
  Opt-in full vertical-slice test for `servarr-core`.
- `bin/live-host`
  Host-side orchestration script invoked over SSH on `backup-node`.
- `bin/live-cleanup`
  Host-side orphan cleanup script for stale runs on `backup-node`.
- `fixtures/live/profiles/servarr-core.json`
  First live profile definition — Radarr, Sonarr, Prowlarr.
- `fixtures/live/profiles/media.json`
  Media stack profile — Plex, Tautulli, Overseerr.
- `fixtures/live/profiles/download.json`
  Download stack profile — SABnzbd, qBittorrent.
- `fixtures/live/profiles/notes.json`
  Notes/docs stack profile — Memos, Linkding, Bytestash, Paperless.
- `fixtures/live/profiles/notifications.json`
  Notifications stack profile — Gotify, Apprise.
- `fixtures/live/profiles/ai.json`
  AI stack profile — Qdrant.
- `fixtures/live/profiles/all.json`
  Aggregate profile — all 15 services.
- `fixtures/live/services/radarr.json`
  Fixture definition and live cases for Radarr.
- `fixtures/live/services/sonarr.json`
  Fixture definition and live cases for Sonarr.
- `fixtures/live/services/prowlarr.json`
  Fixture definition and live cases for Prowlarr.
- `fixtures/live/services/plex.json`
  Fixture definition and live cases for Plex (limited — unclaimed, only `/identity`).
- `fixtures/live/services/tautulli.json`
  Fixture definition and live cases for Tautulli.
- `fixtures/live/services/overseerr.json`
  Fixture definition and live cases for Overseerr (limited — setup-pending, only `/api/v1/status`).
- `fixtures/live/services/sabnzbd.json`
  Fixture definition and live cases for SABnzbd.
- `fixtures/live/services/qbittorrent.json`
  Fixture definition and live cases for qBittorrent.
- `fixtures/live/services/memos.json`
  Fixture definition and live cases for Memos.
- `fixtures/live/services/linkding.json`
  Fixture definition and live cases for Linkding.
- `fixtures/live/services/bytestash.json`
  Fixture definition and live cases for Bytestash.
- `fixtures/live/services/paperless.json`
  Fixture definition and live cases for Paperless (requires Redis sidecar).
- `fixtures/live/services/gotify.json`
  Fixture definition and live cases for Gotify.
- `fixtures/live/services/apprise.json`
  Fixture definition and live cases for Apprise.
- `fixtures/live/services/qdrant.json`
  Fixture definition and live cases for Qdrant.
- `fixtures/live/README.md`
  Operator docs for fixture layout, snapshot naming, and refresh workflow.
- `docs/LIVE_TESTING.md`
  Canonical doc for live environment usage, artifact layout, and safety rules.
- `docs/coverage/live.md`
  Cross-service live environment coverage status and fixture ownership notes.

### Modified Files

- `crates/lab/src/cli.rs`
  Register the new `live` subcommand.
- `crates/lab/src/main.rs`
  Wire new module exports if needed by existing structure.
- `Cargo.toml`
  If needed, update workspace or test target configuration for new shell/live assets.
- `crates/lab/Cargo.toml`
  Add any minimal dependencies required by the live infrastructure.
- `Justfile`
  Add `live-env-up`, `live-env-down`, `live-test`, `live-test-integration`, orphan cleanup targets, and `install-live-host HOST=backup-node` (two-line scp + chmod; replaces Task 3.5 — no CLI subcommand needed).
- `docs/README.md`
  Add `LIVE_TESTING.md` to the topic map.
- `docs/TESTING.md`
  Link the live E2E system as the canonical automated live environment.
- `docs/OPERATIONS.md`
  Add operator workflow for `backup-node` fixture host management.
- `docs/OBSERVABILITY.md`
  Add expectations for live-run artifact fields if new ones are introduced.
- `.gitignore`
  Add `artifacts/` entry in **Task 1, Step 1** — before any code is written. Verify with `git check-ignore artifacts/live/canary/manifest.json`.

## Task 1: Introduce the Live Testing Module Skeleton

**Files:**
- Create: `crates/lab/src/live.rs` (module declaration — **no** `live/mod.rs`; this is the modern Rust module style required by the project)
- Create: `crates/lab/src/live/types.rs`
- Create: `crates/lab/src/live/config.rs`
- Create: `crates/lab/src/live/CLAUDE.md`
- Modify: `crates/lab/src/main.rs`
- Test: `crates/lab/tests/live_config.rs`

> **Module style note:** The project has a hard no-`mod.rs` rule. Use `crates/lab/src/live.rs` as the module declaration file and `crates/lab/src/live/` as the directory for sub-modules. Never create `live/mod.rs`.

- [ ] **Step 0: Add `artifacts/` to `.gitignore` first (do this before anything else)**

```
# live test run artifacts — contain run manifests with credentials; never commit
artifacts/
```

Verify immediately: `git check-ignore artifacts/live/canary/manifest.json` must return a match.
This MUST be committed before any live test infrastructure code is added. A `git add .` after Task 8 without this entry would commit manifest JSON containing live service credentials.

- [ ] **Step 1: Write the failing config parsing test**

Use `env!("CARGO_MANIFEST_DIR")` to anchor the fixture path so CI tests don't break when run from a different working directory:

```rust
use lab::live::config::load_profile;
use std::path::Path;

#[test]
fn loads_servarr_core_profile() {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap().parent().unwrap()
        .join("fixtures/live/profiles");
    let profile = load_profile(fixtures_dir.join("servarr-core.json")).unwrap();
    assert_eq!(profile.name, "servarr-core");
    assert!(profile.services.iter().any(|svc| svc == "radarr"));
}
```

- [ ] **Step 2: Run the narrow test to verify it fails**

Run: `cargo test -p lab --test live_config loads_servarr_core_profile -- --exact`
Expected: FAIL because `lab::live` or `load_profile` does not exist yet.

- [ ] **Step 3: Add minimal module/type/config scaffolding**

```rust
// crates/lab/src/live/types.rs
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]  // surface typos at parse time, not at runtime
pub struct LiveProfile {
    pub name: String,
    pub services: Vec<String>,
}

/// Typed errors for the live test infrastructure with stable `kind` values.
/// These parallel the ToolError vocabulary used throughout the project.
#[derive(Debug, thiserror::Error)]
pub enum LiveError {
    #[error("manifest schema version mismatch: expected {expected}, got {got}")]
    ManifestVersionMismatch { expected: u32, got: u32 },
    #[error("container did not start for service '{service}': {detail}")]
    ContainerNotReady { service: String, detail: String },
    #[error("fixture deserialize error for {path}: {message}")]
    FixtureDeserialize { path: String, message: String },
    #[error("SSH command timed out after {seconds}s")]
    SshTimeout { seconds: u64 },
    #[error("credential fetch failed")]
    CredentialFetchFailed,
}

// crates/lab/src/live/config.rs
pub fn load_profile(path: impl AsRef<std::path::Path>) -> anyhow::Result<crate::live::types::LiveProfile> {
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}
```

- [ ] **Step 4: Create `crates/lab/src/live/CLAUDE.md`**

```markdown
# live/ — Hard Rules

1. All SSH invocations MUST use `tokio::process::Command` (never `std::process::Command`).
   `std::process` blocks the Tokio runtime thread. Violations block merge.

2. Every `Child` handle from an SSH subprocess MUST call `.kill_on_drop(true)` before awaiting.
   Without it, a cancelled task leaks the SSH process — `live down` never runs, ZFS clones are orphaned.

3. Every SSH invocation MUST include connection timeout options:
   `-o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3`
   Plus `timeout <N>` on the remote command for the total wall-clock budget.

4. `run_id` and `profile` MUST be validated against `^[a-z0-9][a-z0-9_-]{0,63}$`
   before constructing any SSH command. Fail with a structured error, not a panic.

5. Never use string interpolation to build SSH remote commands.
   Use `.arg()` calls on `tokio::process::Command` — each value becomes a discrete argv entry.
```

- [ ] **Step 5: Run the narrow test to verify it passes**

Run: `cargo test -p lab --test live_config loads_servarr_core_profile -- --exact`
Expected: PASS.

- [ ] **Step 6: Expand the parsing tests to cover invalid profiles**

Add tests for:
- missing `name`
- empty `services`
- duplicate service names
- unknown field (must fail with `deny_unknown_fields`)

- [ ] **Step 7: Run the full config test target**

Run: `cargo test -p lab --test live_config`
Expected: PASS.

- [ ] **Step 8: Commit the skeleton**

```bash
git add .gitignore crates/lab/src/live.rs crates/lab/src/live/types.rs crates/lab/src/live/config.rs crates/lab/src/live/CLAUDE.md crates/lab/tests/live_config.rs crates/lab/src/main.rs
git commit -m "feat: add live testing module skeleton"
```

## Task 2: Add Fixture Definitions and Validation

**Files:**
- Create: `fixtures/live/profiles/servarr-core.json`
- Create: `fixtures/live/profiles/media.json`
- Create: `fixtures/live/profiles/download.json`
- Create: `fixtures/live/profiles/notes.json`
- Create: `fixtures/live/profiles/notifications.json`
- Create: `fixtures/live/profiles/ai.json`
- Create: `fixtures/live/profiles/all.json`
- Create: `fixtures/live/services/radarr.json`
- Create: `fixtures/live/services/sonarr.json`
- Create: `fixtures/live/services/prowlarr.json`
- Create: `fixtures/live/services/plex.json`
- Create: `fixtures/live/services/tautulli.json`
- Create: `fixtures/live/services/overseerr.json`
- Create: `fixtures/live/services/sabnzbd.json`
- Create: `fixtures/live/services/qbittorrent.json`
- Create: `fixtures/live/services/memos.json`
- Create: `fixtures/live/services/linkding.json`
- Create: `fixtures/live/services/bytestash.json`
- Create: `fixtures/live/services/paperless.json`
- Create: `fixtures/live/services/gotify.json`
- Create: `fixtures/live/services/apprise.json`
- Create: `fixtures/live/services/qdrant.json`
- Create: `fixtures/live/README.md`
- Modify: `crates/lab/src/live/types.rs`
- Modify: `crates/lab/src/live/config.rs`
- Test: `crates/lab/tests/live_config.rs`

- [ ] **Step 1: Write the failing fixture validation test**

Use `env!("CARGO_MANIFEST_DIR")` to anchor paths (see Task 1 note):

```rust
use lab::live::config::load_service_fixture;
use std::path::Path;

#[test]
fn validates_radarr_fixture_has_snapshot_and_cases() {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap().parent().unwrap()
        .join("fixtures/live/services");
    let fixture = load_service_fixture(fixtures_dir.join("radarr.json")).unwrap();
    assert_eq!(fixture.service, "radarr");
    assert!(fixture.snapshot.dataset.contains("radarr"));
    assert!(!fixture.cases.is_empty());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p lab --test live_config validates_radarr_fixture_has_snapshot_and_cases -- --exact`
Expected: FAIL because service fixture loading/types do not exist yet.

- [ ] **Step 3: Add minimal fixture definition types**

Apply `#[serde(deny_unknown_fields)]` to all fixture types to surface typos at parse time.

**Fixture schema** — each service fixture JSON has this shape:

```json
{
  "service": "radarr",
  "image": "lscr.io/linuxserver/radarr:latest",
  "snapshot": {
    "dataset": "backup/lab/live/golden/radarr",
    "snapshot": "configured-v1"
  },
  "container_port": 7878,
  "readiness_url": "/ping",
  "readiness_timeout_s": 120,
  "poll_interval_s": 2,
  "env": {
    "PUID": "99",
    "PGID": "100",
    "TZ": "America/New_York"
  },
  "sidecars": [],
  "auth_type": "api_key",
  "secret_env_key": "api_key",
  "cases": [
    {
      "surface": "mcp",
      "operation": "system.status",
      "expected": "pass",
      "destructive": false
    }
  ]
}
```

**Key fields:**
- `image` — Docker image to `docker run`. Required for `bin/live-host` to know what to start.
- `container_port` — the port the service listens on inside the container. Host port is allocated dynamically via `127.0.0.1:0:<container_port>`.
- `env` — environment variables to pass via `-e` flags. Service-specific (e.g., `JWT_SECRET` for bytestash, `LD_SUPERUSER_NAME` for linkding).
- `sidecars` — array of sidecar container definitions (see Paperless below). Empty for most services.
- `auth_type` — one of: `api_key`, `token`, `basic`, `cookie`, `jwt`, `none`. Drives how `bin/live-host` extracts credentials for the manifest `secrets` block.
- `secret_env_key` — the key name in the manifest `secrets` object for this service's primary credential.

**Sidecar definition** (for Paperless Redis):
```json
{
  "sidecars": [
    {
      "name": "redis",
      "image": "redis:7-alpine",
      "container_port": 6379,
      "env": {},
      "inject_env": {
        "PAPERLESS_REDIS": "redis://{sidecar_ip}:6379"
      }
    }
  ]
}
```
`inject_env` passes the sidecar's container IP into the main service's environment. `bin/live-host` resolves `{sidecar_ip}` after starting the sidecar.

**Service-specific fixture notes:**

| Service | `auth_type` | Special `env` | Sidecars | Notes |
|---------|-------------|---------------|----------|-------|
| Radarr | `api_key` | PUID, PGID, TZ | — | API key from config.xml |
| Sonarr | `api_key` | PUID, PGID, TZ | — | API key from config.xml |
| Prowlarr | `api_key` | PUID, PGID, TZ | — | API key from config.xml |
| Plex | `none` | PUID, PGID, TZ | — | Unclaimed; only `/identity` |
| Tautulli | `api_key` | PUID, PGID, TZ | — | API key from config.ini |
| Overseerr | `none` | PUID, PGID, TZ | — | Setup-pending; only `/api/v1/status` |
| SABnzbd | `api_key` | PUID, PGID, TZ | — | API key from sabnzbd.ini; `inet_exposure=4` |
| qBittorrent | `cookie` | PUID, PGID, TZ | — | Login via `/api/v2/auth/login`, cookie auth |
| Memos | `jwt` | — | — | Bearer token via access_tokens API |
| Linkding | `token` | `LD_SUPERUSER_NAME`, `LD_SUPERUSER_PASSWORD` | — | Token via `ApiToken` model (not DRF Token) |
| Bytestash | `jwt` | `JWT_SECRET` | — | JWT auth, user/pass login |
| Paperless | `token` | — | redis:7-alpine | DRF Token auth; needs Redis |
| Gotify | `token` | — | — | App token via `/application` |
| Apprise | `none` | — | — | Stateless, no auth |
| Qdrant | `none` | — | — | No auth; REST + gRPC |

Note: `ports` (host-side) are **not** in the fixture definition — they are allocated dynamically per run (see Task 8) to avoid port collisions between concurrent runs. Only `container_port` is declared.

- [ ] **Step 4: Implement validation rules**

Rules to enforce:
- snapshot dataset and snapshot name are required and non-empty
- case operations must be unique per surface
- profile service list must refer only to known fixture files
- `readiness_timeout_s` must be present and > 0

**Do not** enforce "service name must match filename stem" as a runtime validation rule — that is a convention, not a hard constraint. Document it in `fixtures/live/README.md` instead.

**Document in `fixtures/live/README.md`:** Golden snapshots must be created from a clean-seed environment with test-only credentials. Never snapshot a production service instance. Rotate credentials when refreshing a snapshot.

- [ ] **Step 5: Run the full config test target**

Run: `cargo test -p lab --test live_config`
Expected: PASS with profile + fixture validation coverage.

- [ ] **Step 6: Commit fixture definitions and validation**

```bash
git add fixtures/live/profiles/ fixtures/live/services/ fixtures/live/README.md \
    crates/lab/src/live/types.rs crates/lab/src/live/config.rs crates/lab/tests/live_config.rs
git commit -m "feat: add live fixture definitions and validation for all 15 services"
```

## Task 3: Define the SSH Host Orchestration Contract

**Files:**
- Create: `bin/live-host`
- Create: `bin/live-cleanup`
- Create: `tests/live_host_contract_test.sh`
- Modify: `fixtures/live/README.md`
- Create: `docs/LIVE_TESTING.md`
- Modify: `Justfile`
- Test: `tests/live_host_contract_test.sh`

- [ ] **Step 1: Write the host contract test script first**

```bash
#!/usr/bin/env bash
set -euo pipefail

# Use staged invocation (bin/live-host must be installed on backup-node first — see Task 3.5)
manifest=$(ssh -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 \
    -o ServerAliveCountMax=3 backup-node live-host up servarr-core test-run-123)
echo "$manifest" | jq -e '.run_id == "test-run-123"'
echo "$manifest" | jq -e '.services.radarr.url | startswith("http")'
ssh -o BatchMode=yes -o ConnectTimeout=10 backup-node live-host down test-run-123 >/dev/null
```

- [ ] **Step 2: Run the script to verify it fails**

Run: `bash tests/live_host_contract_test.sh`
Expected: FAIL because `bin/live-host` does not exist yet.

- [ ] **Step 3: Implement the minimal host contract script**

**Security requirements (enforce at the top of the script before any operations):**

```bash
# SYNC: this pattern must match crates/lab/src/live/host/validation.rs:LIVE_ID_PATTERN
# Validate run_id and profile against strict allowlist
# NOTE: Use printf '%s', never echo — echo interprets leading hyphens as flags on busybox/dash
validate_id() {
    if ! printf '%s' "$1" | grep -qE '^[a-z0-9][a-z0-9_-]{0,63}$'; then
        printf '{"error":"invalid_input","field":"%s","value":"%s"}\n' "$2" "$1" >&2
        exit 2
    fi
}

# Service registry — maps profile names to service lists.
# bin/live-host is the SINGLE SOURCE OF TRUTH for which services belong to which profile.
# The Rust-side profile JSON files declare the same lists for fixture loading, but
# bin/live-host does NOT read those files — it uses this static registry.
# SYNC: profile membership must match fixtures/live/profiles/*.json
declare -A PROFILE_SERVICES=(
    [servarr-core]="radarr sonarr prowlarr"
    [media]="plex tautulli overseerr"
    [download]="sabnzbd qbittorrent"
    [notes]="memos linkding bytestash paperless"
    [notifications]="gotify apprise"
    [ai]="qdrant"
    [all]="radarr sonarr prowlarr plex tautulli overseerr sabnzbd qbittorrent memos linkding bytestash paperless gotify apprise qdrant"
)

# Declare SERVICES as an array BEFORE any ZFS operation.
# The cleanup trap iterates this array — if SIGKILL arrives mid-clone and the array
# is not yet populated, no cleanup runs and ZFS datasets are orphaned.
PROFILE="${2:-}"
[[ -n "${PROFILE_SERVICES[$PROFILE]+x}" ]] || {
    printf '{"error":"unknown_profile","value":"%s","valid":[%s]}\n' \
        "$PROFILE" "$(printf '"%s",' "${!PROFILE_SERVICES[@]}" | sed 's/,$//')" >&2
    exit 2
}
read -ra SERVICES <<< "${PROFILE_SERVICES[$PROFILE]}"

# Service image registry — maps service names to Docker images.
declare -A SERVICE_IMAGES=(
    [radarr]="lscr.io/linuxserver/radarr:latest"
    [sonarr]="lscr.io/linuxserver/sonarr:latest"
    [prowlarr]="lscr.io/linuxserver/prowlarr:latest"
    [plex]="lscr.io/linuxserver/plex:latest"
    [tautulli]="ghcr.io/hotio/tautulli:testing"
    [overseerr]="ghcr.io/hotio/overseerr:release"
    [sabnzbd]="ghcr.io/hotio/sabnzbd:latest"
    [qbittorrent]="linuxserver/qbittorrent:latest"
    [memos]="ghcr.io/usememos/memos"
    [linkding]="sissbruecker/linkding"
    [bytestash]="ghcr.io/jordan-dalby/bytestash:latest"
    [paperless]="ghcr.io/paperless-ngx/paperless-ngx"
    [gotify]="gotify/server:latest"
    [apprise]="caronc/apprise:latest"
    [qdrant]="qdrant/qdrant:latest"
)

# Service container port registry — maps service names to internal ports.
declare -A SERVICE_PORTS=(
    [radarr]=7878 [sonarr]=8989 [prowlarr]=9696
    [plex]=32400 [tautulli]=8181 [overseerr]=5055
    [sabnzbd]=8080 [qbittorrent]=8080
    [memos]=5230 [linkding]=9090 [bytestash]=5000 [paperless]=8000
    [gotify]=80 [apprise]=8000
    [qdrant]=6333
)

# Service-specific env vars (beyond common PUID/PGID/TZ).
declare -A SERVICE_EXTRA_ENV=(
    [linkding]="-e LD_SUPERUSER_NAME=admin -e LD_SUPERUSER_PASSWORD=lab-test-golden"
    [bytestash]="-e JWT_SECRET=test-golden-secret-key-for-lab"
)

# Sidecar registry — services that need companion containers.
# Format: "sidecar_name:image:port:inject_env_key=inject_env_value_template"
declare -A SERVICE_SIDECARS=(
    [paperless]="redis:redis:7-alpine:6379:PAPERLESS_REDIS=redis://{ip}:6379"
)
```

**SSH one-time setup requirement:** The SSH calls in this script require `backup-node`'s host key to be in a known-hosts file. One-time setup (add to `docs/LIVE_TESTING.md`):
```bash
ssh-keyscan backup-node >> ~/.labby/known_hosts
```
All SSH calls must include `-o StrictHostKeyChecking=yes -o UserKnownHostsFile=~/.labby/known_hosts`.

Contract:
- `up <profile> <run-id>` validates inputs first, then emits JSON manifest to stdout (stdout must be **only** the JSON manifest — redirect all other output to stderr)
- `down <run-id>` performs idempotent teardown
- `cleanup` removes orphaned runs older than 48h
- non-zero exit for invalid args or failed orchestration

```bash
case "${1:-}" in
  up)   validate_id "${3:-}" "run_id"; validate_id "${2:-}" "profile" ;;
  down) validate_id "${2:-}" "run_id" ;;
  cleanup) ;;
  *)
    printf '{"error":"invalid_command","value":"%s"}\n' "${1:-}" >&2
    exit 2
    ;;
esac
```

Add a cleanup trap so ZFS clones and Docker stacks are removed on abort:

```bash
cleanup_on_exit() {
    if [[ -n "${RUN_ID:-}" ]]; then
        # Stop and remove all containers for this run (services + sidecars)
        for svc in "${SERVICES[@]}"; do
            docker rm -f "${svc}-${RUN_ID}" 2>/dev/null || true
            # Remove sidecar if one exists
            if [[ -n "${SERVICE_SIDECARS[$svc]:-}" ]]; then
                SC_NAME="${SERVICE_SIDECARS[$svc]%%:*}"
                docker rm -f "${SC_NAME}-${RUN_ID}" 2>/dev/null || true
            fi
        done
        docker network rm "lab-live-$RUN_ID" 2>/dev/null || true
        sync
        # destroy per-service clones explicitly (no -r recursive to avoid siblings)
        for svc in "${SERVICES[@]}"; do
            zfs destroy "backup/lab/live/runs/$RUN_ID/$svc" 2>/dev/null || true
        done
    fi
}
trap cleanup_on_exit EXIT INT TERM
```

- [ ] **Step 4: Implement per-run naming and placeholder manifest output**

Manifest fields:
- `schema_version` (u32, currently `1`) — **required**. The Rust parser must fail with `LiveError::ManifestVersionMismatch` if this does not match the expected value. Prevents silent drift between `bin/live-host` and the Rust manifest parser.
- `run_id`
- `profile`
- `services` (object — keys are service names, values have `url` and `port`)
- `secrets` (object — credentials per service, stripped by Rust before writing `artifacts/`) — see Task 8
- `network`
- `artifacts_dir`
- `snapshot_versions`

**Manifest delivery:** Emit the full manifest (including the `secrets` key) to stdout ONLY. Do NOT write a manifest file on backup-node — the Rust caller captures stdout and holds credentials in memory. The Rust caller strips `secrets` before writing `artifacts/live/<run_id>/manifest.json`. All other output from `bin/live-host` (Docker pull progress, ZFS output, debug messages) MUST be redirected to stderr — a single byte on stdout that is not the JSON manifest will corrupt the parse.

**stdout-only rule:** Add this at the top of the `up` subcommand, after initial validation:
```bash
exec 3>&1  # save original stdout
exec 1>&2  # redirect stdout to stderr (all script output goes to stderr)
# ... do all work ...
printf '%s\n' "$MANIFEST_JSON" >&3  # emit manifest on original stdout at end
exec 3>&-
```

- [ ] **Step 5: Add `Justfile` targets for raw host contract calls**

Add SSH timeout and host key options on every invocation:

```just
live-env-up PROFILE RUN_ID:
    ssh -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 \
        -o StrictHostKeyChecking=yes -o UserKnownHostsFile=~/.labby/known_hosts \
        -o ServerAliveCountMax=3 backup-node live-host up {{PROFILE}} {{RUN_ID}}

live-env-down RUN_ID:
    ssh -o BatchMode=yes -o ConnectTimeout=10 \
        -o StrictHostKeyChecking=yes -o UserKnownHostsFile=~/.labby/known_hosts \
        backup-node live-host down {{RUN_ID}}

# One-time setup: stage bin/live-host onto backup-node (no CLI subcommand needed)
install-live-host HOST="backup-node":
    scp bin/live-host {{HOST}}:~/.local/bin/live-host.tmp
    ssh {{HOST}} 'chmod +x ~/.local/bin/live-host.tmp && mv ~/.local/bin/live-host.tmp ~/.local/bin/live-host'
    @echo "Installed. Verify PATH: ssh {{HOST}} which live-host"
```

- [ ] **Step 6: Run the host contract test again**

Run: `bash tests/live_host_contract_test.sh`
Expected: PASS for placeholder orchestration contract and teardown.

- [ ] **Step 7: Document the contract**

Document:
- required tools on `backup-node` (zfs, docker/docker compose, jq)
- expected ZFS layout
- expected Docker privileges
- manifest shape
- golden snapshot safety rule: "Never snapshot a production service instance. Use test-only credentials."

- [ ] **Step 8: Commit the host contract**

```bash
git add bin/live-host bin/live-cleanup tests/live_host_contract_test.sh fixtures/live/README.md docs/LIVE_TESTING.md Justfile
git commit -m "feat: add backup-node live host contract"
```

## ~~Task 3.5: Add `lab live install-host` Subcommand~~ — ELIMINATED

> Replaced by `just install-live-host HOST=backup-node` (added to Justfile in Task 3 Step 5). The `install-host` concern is a one-time developer setup — a two-line Justfile target is the correct level of abstraction. No CLI subcommand, no `host.rs` method, no test needed.
>
> Document in `docs/LIVE_TESTING.md`: "Run `just install-live-host` once after cloning the repo or after updating `bin/live-host`."

## Task 4: Implement Repo-Side Host Client

**Files:**
- Create: `crates/lab/src/live/host.rs` (module declaration — `pub mod connection; pub mod validation; pub mod readiness; pub mod manifest;`)
- Create: `crates/lab/src/live/host/connection.rs` (SSH execution)
- Create: `crates/lab/src/live/host/validation.rs` (run_id/profile regex validation)
- Create: `crates/lab/src/live/host/readiness.rs` (parallel readiness polling)
- Create: `crates/lab/src/live/host/manifest.rs` (manifest parsing + `RunGuard`)
- Modify: `crates/lab/src/live/types.rs`
- Test: `crates/lab/tests/live_runner.rs`

- [ ] **Step 1: Write the failing host client test**

```rust
use lab::live::host::parse_manifest;

#[test]
fn parses_host_manifest_json() {
    let json = r#"{"run_id":"abc","profile":"servarr-core","services":{"radarr":{"url":"http://127.0.0.1:7878"}}}"#;
    let manifest = parse_manifest(json).unwrap();
    assert_eq!(manifest.run_id, "abc");
    assert_eq!(manifest.profile, "servarr-core");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p lab --test live_runner parses_host_manifest_json -- --exact`
Expected: FAIL because host client parsing does not exist yet.

- [ ] **Step 3: Implement minimal manifest parsing and SSH command execution**

**Hard rules (from `crates/lab/src/live/CLAUDE.md`):**
- Use `tokio::process::Command` — never `std::process::Command`
- Call `.kill_on_drop(true)` on every child handle
- Include SSH timeout options on every invocation
- Use `.arg()` calls only — no string interpolation into the remote command

```rust
/// Execute a live-host command on backup-node and return stdout as a string.
/// Enforces wall-clock timeout and kills the child on drop.
async fn run_host_command(args: &[&str]) -> anyhow::Result<String> {
    let mut child = tokio::process::Command::new("ssh")
        .args(["-o", "BatchMode=yes",
               "-o", "ConnectTimeout=10",
               "-o", "ServerAliveInterval=15",
               "-o", "ServerAliveCountMax=3",
               "backup-node"])
        .args(["live-host"])
        .args(args)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(330),
        child.wait_with_output(),
    ).await
    .map_err(|_| anyhow::anyhow!("SSH command timed out after 330s"))?
    .map_err(|e| anyhow::anyhow!("SSH command failed: {e}"))?;

    if !output.status.success() {
        // IMPORTANT: never include raw stderr in error events — it may contain credential fragments
        // Log at WARN with a redacted summary; return a typed LiveError instead of an anyhow chain
        tracing::warn!(surface = "live", kind = "ssh_error", exit_code = ?output.status, "live-host command failed");
        return Err(anyhow::anyhow!(LiveError::SshTimeout { seconds: 120 }));
    }
    Ok(String::from_utf8(output.stdout)?)
}
```

**SSH `up` lifecycle — critical design change:**
`bin/live-host up` exits as soon as `docker run` completes and the manifest JSON is emitted. It does NOT wait for service readiness — that is the Rust client's responsibility via `wait_for_services()` in `readiness.rs`. This keeps the SSH subprocess short-lived (~10–60s) and eliminates the truncated-JSON failure mode where Docker slow-start causes SSH timeout before the manifest is written.

The SSH timeout for `up` is **120s** (Docker start, not readiness). The readiness timeout is a separate Rust-side parameter.

Additional requirements:
- validate `run_id` and `profile` against `^[a-z0-9][a-z0-9_-]{0,63}$` in `validation.rs` before constructing any command (Rust side). Add a `SYNC` comment referencing the bash `validate_id` regex.
- classify transport errors (timeout, connection refused) as `LiveError::SshTimeout` separately from manifest validation errors
- parse `schema_version` from manifest JSON before any field access; return `LiveError::ManifestVersionMismatch` if unexpected

**Parallel readiness polling (`host/readiness.rs`):**
```rust
use futures::future::try_join_all;

/// Build one shared client for all polls in this run. Never call reqwest::get() directly
/// — it allocates a new connection pool on every call (the same class of bug as the
/// prior fix(perf,tui): single-pass GraphQL deserialization regression).
pub async fn wait_for_services(
    services: &[(&str, &str)],  // (service_name, readiness_url)
    client: &reqwest::Client,   // shared, built once per run
    timeout_s: u64,
    poll_interval_s: u64,
) -> anyhow::Result<()> {
    try_join_all(services.iter().map(|(name, url)| {
        let client = client.clone();
        let url = url.to_string();
        let name = name.to_string();
        async move {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_s);
            loop {
                // Build request without following redirects (paperless returns 302 when ready)
                if let Ok(resp) = client.get(&url).send().await {
                    if resp.status().is_success() || resp.status() == reqwest::StatusCode::FOUND {
                        return Ok::<_, anyhow::Error>(());
                    }
                }
                if tokio::time::Instant::now() >= deadline {
                    anyhow::bail!("service '{}' did not become ready within {}s", name, timeout_s);
                }
                tokio::time::sleep(std::time::Duration::from_secs(poll_interval_s)).await;
            }
        }
    })).await?;
    Ok(())
}
```

This reduces worst-case `up` latency for servarr-core from 360s (3 × 120s sequential) to 120s (parallel max).

**`RunGuard` (`host/manifest.rs`) — required:**
```rust
/// Wraps a run_id and guarantees teardown on drop.
/// This is the only mechanism that ensures ZFS clones are destroyed under panic or early return.
/// kill_on_drop(true) only kills the SSH client process, not the remote bash session —
/// without RunGuard, orphaned ZFS datasets accumulate on backup-node until the 48h cleanup window.
pub struct RunGuard {
    run_id: String,
    host: String,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        // Spawn a blocking teardown call. In async context, prefer explicit `.down()` first.
        let _ = std::process::Command::new("ssh")
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10",
                   "-o", "StrictHostKeyChecking=yes",
                   "-o", "UserKnownHostsFile=~/.labby/known_hosts",
                   &self.host, "live-host", "down", &self.run_id])
            .output();
    }
}
```

**Validator parity test (`tests/live_runner.rs`):**
Add a test that asserts the Rust `validation.rs` pattern and the bash `validate_id` regex agree on a fixed set of valid and invalid IDs:
```rust
#[test]
fn valid_run_ids_match_both_validators() {
    // These must also pass the bash grep -qE in validate_id()
    for id in ["abc", "run-123", "a", "servarr-core-20260412"] {
        assert!(is_valid_live_id(id), "expected valid: {id}");
    }
    for id in ["", "-start", "UPPER", "has/slash", &"a".repeat(65)] {
        assert!(!is_valid_live_id(id), "expected invalid: {id}");
    }
}
```

- [ ] **Step 4: Add tests for malformed JSON and missing fields**

Expected classifications:
- invalid JSON
- missing `run_id`
- missing requested service entry
- timeout error (use a mock that delays)

- [ ] **Step 5: Run the full live runner unit test target**

Run: `cargo test -p lab --test live_runner`
Expected: PASS.

- [ ] **Step 6: Commit the host client**

```bash
git add crates/lab/src/live/host.rs crates/lab/src/live/host/ crates/lab/src/live/types.rs crates/lab/tests/live_runner.rs
git commit -m "feat: add live host client with tokio ssh and parallel readiness polling"
```

## Task 5: Add the `lab live` CLI Surface

**Files:**
- Create: `crates/lab/src/cli/live.rs`
- Modify: `crates/lab/src/cli.rs`
- Modify: `crates/lab/src/live/host.rs`
- Test: `crates/lab/tests/live_runner.rs`

- [ ] **Step 1: Write the failing CLI parsing test**

```rust
use clap::Parser;
use lab::cli::Cli;

#[test]
fn parses_live_up_command() {
    let cli = Cli::try_parse_from(["lab", "live", "up", "servarr-core"]).unwrap();
    assert!(matches!(cli.command, lab::cli::Command::Live(_)));
}
```

- [ ] **Step 2: Run the narrow parsing test**

Run: `cargo test -p lab --test live_runner parses_live_up_command -- --exact`
Expected: FAIL because the `live` subcommand is not registered.

- [ ] **Step 3: Implement the minimal CLI shape**

Subcommands:
- `lab live up <profile>`
- `lab live down <run-id>`
- `lab live status <run-id>`
- `lab live cleanup`
- `lab live test <profile> [--allow-destructive]`

**`--allow-destructive` is required.** Without this flag, all destructive cases (`"destructive": true` in fixture) must be classified as `CaseOutcome::SkipDestructive` (not executed). The runner must NOT auto-inject `"confirm": true` based solely on the fixture flag — that would be a programmatic bypass of the confirmation gate, repeating the prior `X-Lab-Confirm` header bypass regression.

With `--allow-destructive`:
- API surface destructive cases: include `"confirm": true` in params body
- MCP surface destructive cases: classify as `CaseOutcome::SkipNoElicitation` (MCP elicitation not yet implemented — see Task 10)
- CLI surface destructive cases: pass `--yes` flag

- [ ] **Step 4: Add a JSON output mode for `up` and `status`**

Requirement:
- `--json` must emit the raw manifest or status object
- human output can remain thin because artifacts carry the detail

- [ ] **Step 5: Run the CLI parsing and behavior tests**

Run: `cargo test -p lab --test live_runner`
Expected: PASS.

- [ ] **Step 6: Commit the CLI surface**

```bash
git add crates/lab/src/cli/live.rs crates/lab/src/cli.rs crates/lab/src/live/host.rs crates/lab/tests/live_runner.rs
git commit -m "feat: add live environment cli"
```

## ~~Task 6: Catalog-Driven Surface Enumeration~~ — DEFERRED

> Absorbed into Task 10. Catalog enumeration only makes sense after the runner is proven end-to-end (Task 9). Adding it before the vertical slice adds ~200 LOC and one test file with no milestone impact. When implemented in Task 10, source MCP actions from `build_catalog()` (not source file parsing), enumerate API surface the same way (same Catalog type), and avoid dynamic CLI enumeration for Tier-2 stubs.

## ~~Task 7: Live Matrix Matching and Classification~~ — DEFERRED

> Absorbed into Task 10. `SurfaceItem`/`MatrixRow` are projections of `LiveCase` for a gap-classification feature that doesn't exist until catalog enumeration exists. For Tasks 8 and 9, the runner iterates `ServiceFixture.cases` directly — no matrix abstraction needed. When introduced in Task 10, add `CaseOutcome { Pass, Fail { kind, message }, SkipReadonly, SkipNoFixture }` as the canonical classification enum in `types.rs`.

## Task 8: Implement Real Host Provisioning (All Profiles)

**Files:**
- Modify: `bin/live-host`
- Modify: `fixtures/live/profiles/servarr-core.json`
- Modify: `fixtures/live/services/radarr.json`
- Modify: `fixtures/live/services/sonarr.json`
- Modify: `fixtures/live/services/prowlarr.json`
- Modify: `fixtures/live/services/plex.json`
- Modify: `fixtures/live/services/tautulli.json`
- Modify: `fixtures/live/services/overseerr.json`
- Modify: `fixtures/live/services/sabnzbd.json`
- Modify: `fixtures/live/services/qbittorrent.json`
- Modify: `fixtures/live/services/memos.json`
- Modify: `fixtures/live/services/linkding.json`
- Modify: `fixtures/live/services/bytestash.json`
- Modify: `fixtures/live/services/paperless.json`
- Modify: `fixtures/live/services/gotify.json`
- Modify: `fixtures/live/services/apprise.json`
- Modify: `fixtures/live/services/qdrant.json`
- Modify: `tests/live_host_contract_test.sh`
- Test: `tests/live_host_contract_test.sh`

- [ ] **Step 1: Extend the host contract test to assert real readiness**

Assertions:
- returned URLs are reachable from `backup-node`
- returned credentials are present for each service
- `down` removes the run resources idempotently

- [ ] **Step 2: Run the test to verify it fails against placeholder orchestration**

Run: `bash tests/live_host_contract_test.sh`
Expected: FAIL because placeholder host provisioning does not create real service stacks yet.

- [ ] **Step 3: Implement ZFS clone lifecycle in `bin/live-host`**

Required operations:
- derive dataset clone names from `run_id` (validated pattern from Task 3; the `SERVICES` static array must already be declared — see Task 3 security requirements)
- Clone each service dataset individually using explicit paths — **never use `zfs destroy -r`** (recursive destroy risks siblings):
  ```bash
  zfs clone backup/lab/live/golden/radarr@configured-v1 backup/lab/live/runs/$RUN_ID/radarr
  zfs clone backup/lab/live/golden/sonarr@configured-v1 backup/lab/live/runs/$RUN_ID/sonarr
  zfs clone backup/lab/live/golden/prowlarr@configured-v1 backup/lab/live/runs/$RUN_ID/prowlarr
  ```
- **Teardown order for `down <run_id>`:** Docker MUST stop before ZFS destroy. If Docker is still holding the filesystem mount, `zfs destroy` will fail:
  ```bash
  # Step 1: stop and remove containers + network (wait for full stop)
  docker compose -p "lab-live-$RUN_ID" down --volumes --remove-orphans 2>/dev/null || true
  docker network rm "lab-live-$RUN_ID" 2>/dev/null || true
  sync  # flush pending writes before unmount
  # Step 2: destroy ZFS clones (explicit per-service, never recursive)
  for svc in "${SERVICES[@]}"; do
      zfs destroy "backup/lab/live/runs/$RUN_ID/$svc" 2>/dev/null || true
  done
  ```
- **Never mount or modify the golden dataset directly** — always clone first. Guard: assert the target path starts with `backup/lab/live/runs/` before any ZFS operation (string prefix check, not a live ZFS query, to avoid TOCTOU).

- [ ] **Step 4: Implement Docker startup (registry-driven)**

Requirements:
- create isolated Docker network per run: `docker network create lab-live-$RUN_ID` (NOT `--internal` — internal networks block `-p` host port mapping; loopback binding provides LAN isolation instead)
- mount per-run datasets into service containers
- **Loop over the `SERVICES` array** — use the registries from Task 3 (`SERVICE_IMAGES`, `SERVICE_PORTS`, `SERVICE_EXTRA_ENV`, `SERVICE_SIDECARS`) to construct each `docker run` invocation:

  ```bash
  # Common env vars for linuxserver images
  COMMON_ENV="-e PUID=99 -e PGID=100 -e TZ=America/New_York"

  for svc in "${SERVICES[@]}"; do
      IMAGE="${SERVICE_IMAGES[$svc]}"
      CPORT="${SERVICE_PORTS[$svc]}"
      EXTRA_ENV="${SERVICE_EXTRA_ENV[$svc]:-}"

      # Start sidecars first if needed
      if [[ -n "${SERVICE_SIDECARS[$svc]:-}" ]]; then
          # Parse sidecar spec: name:image:port:env_key=env_value_template
          IFS=: read -r SC_NAME SC_IMAGE SC_PORT SC_INJECT <<< "${SERVICE_SIDECARS[$svc]}"
          docker run -d --name "${SC_NAME}-${RUN_ID}" --network "lab-live-${RUN_ID}" \
              "$SC_IMAGE" >/dev/null
          SC_IP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "${SC_NAME}-${RUN_ID}")
          # Resolve {ip} placeholder in inject_env
          SC_ENV_VAL="${SC_INJECT#*=}"
          SC_ENV_KEY="${SC_INJECT%%=*}"
          SC_ENV_VAL="${SC_ENV_VAL//\{ip\}/$SC_IP}"
          EXTRA_ENV="$EXTRA_ENV -e ${SC_ENV_KEY}=${SC_ENV_VAL}"
      fi

      # Determine volume mount path — most linuxserver/hotio images use /config
      # SYNC: must match the mount used when creating golden snapshots
      VMOUNT="/config"
      case "$svc" in
          memos)     VMOUNT="/var/opt/memos" ;;
          linkding)  VMOUNT="/etc/linkding/data" ;;
          bytestash) VMOUNT="/data/snippets" ;;
          paperless) VMOUNT="/usr/src/paperless/data" ;;
          gotify)    VMOUNT="/app/data" ;;
          apprise)   ;; # uses default /config
          qdrant)    VMOUNT="/qdrant/storage" ;;
      esac

      docker run -d --name "${svc}-${RUN_ID}" --network "lab-live-${RUN_ID}" \
          -p "127.0.0.1:0:${CPORT}" \
          -v "/mnt/backup/lab/live/runs/${RUN_ID}/${svc}:${VMOUNT}" \
          $COMMON_ENV $EXTRA_ENV \
          "$IMAGE" >/dev/null

      # Extract dynamically-assigned host port
      HOST_PORT=$(docker port "${svc}-${RUN_ID}" "${CPORT}/tcp" | cut -d: -f2)
      [[ "$HOST_PORT" =~ ^[0-9]+$ ]] || {
          printf '{"error":"container_start_failed","service":"%s","port":"%s"}\n' "$svc" "$HOST_PORT" >&2
          exit 1
      }
      # Store for manifest generation
      eval "PORT_${svc^^}=$HOST_PORT"  # e.g., PORT_RADARR=32789
  done
  ```

- **Qdrant exposes two ports** — 6333 (HTTP) and 6334 (gRPC). Bind both: `-p 127.0.0.1:0:6333 -p 127.0.0.1:0:6334`. Extract both for the manifest.
- Emit the manifest (stdout) **immediately after all `docker run` calls complete** — do NOT wait for service readiness in bash. Readiness polling is the Rust client's responsibility (Task 4 `wait_for_services()`). This keeps the SSH subprocess short-lived.
- ensure services cannot make external network calls: configure Radarr/Sonarr/Prowlarr to disable external updates/indexers in the golden snapshot config

- [ ] **Step 5: Emit a real manifest**

Manifest must include credentials under `secrets` (stripped by Rust before writing `artifacts/`). No separate `.secrets.json` file on backup-node — credentials live only in the stdout pipe and Rust process memory.

```json
{
  "schema_version": 1,
  "run_id": "...",
  "profile": "all",
  "network": "lab-live-<run_id>",
  "snapshot_versions": {"radarr": "configured-v1", "sonarr": "configured-v1", "...": "..."},
  "services": {
    "radarr": {"url": "http://127.0.0.1:<port>", "port": "<port>"},
    "sonarr": {"url": "http://127.0.0.1:<port>", "port": "<port>"},
    "plex": {"url": "http://127.0.0.1:<port>", "port": "<port>"},
    "qdrant": {"url": "http://127.0.0.1:<port>", "port": "<port>", "grpc_port": "<grpc_port>"}
  },
  "secrets": {
    "radarr": {"api_key": "<key>"},
    "sonarr": {"api_key": "<key>"},
    "prowlarr": {"api_key": "<key>"},
    "tautulli": {"api_key": "<key>"},
    "sabnzbd": {"api_key": "<key>"},
    "qbittorrent": {"username": "admin", "password": "lab-test-golden"},
    "memos": {"access_token": "<jwt>"},
    "linkding": {"token": "<token>"},
    "bytestash": {"username": "admin", "password": "lab-test-golden", "jwt_secret": "test-golden-secret-key-for-lab"},
    "paperless": {"token": "<token>"},
    "gotify": {"app_token": "AjoHfnf2U3AOJ5d"},
    "plex": {},
    "overseerr": {},
    "apprise": {},
    "qdrant": {}
  }
}
```

The Rust caller in Task 4 parses the full manifest, holds `secrets` in memory only, and strips it before writing `artifacts/live/<run_id>/manifest.json`. Credentials never touch disk.

**Credential extraction by auth_type** — `bin/live-host` extracts secrets per service after containers start:

```bash
extract_secrets() {
    local svc="$1" run_id="$2"
    case "$svc" in
        radarr|sonarr|prowlarr)
            # API key from config.xml
            API_KEY=$(docker exec "${svc}-${run_id}" grep -oP '(?<=<ApiKey>)[^<]+' /config/config.xml)
            printf '"api_key":"%s"' "$API_KEY"
            ;;
        tautulli)
            # API key from config.ini
            API_KEY=$(docker exec "${svc}-${run_id}" grep -oP '(?<=api_key = )[^\s]+' /config/config.ini)
            printf '"api_key":"%s"' "$API_KEY"
            ;;
        sabnzbd)
            # API key from sabnzbd.ini
            API_KEY=$(docker exec "${svc}-${run_id}" grep -oP '(?<=api_key = )[^\s]+' /config/sabnzbd.ini)
            printf '"api_key":"%s"' "$API_KEY"
            ;;
        qbittorrent)
            # Static credentials baked into golden snapshot
            printf '"username":"admin","password":"lab-test-golden"'
            ;;
        memos)
            # JWT access token — must be created via API after startup (done in golden, persisted in snapshot)
            # The access token is stored in the DB; read from the golden snapshot's known token
            printf '"access_token":"<from-golden>"'
            ;;
        linkding)
            # Token via ApiToken model — baked into golden snapshot DB
            printf '"token":"<from-golden>"'
            ;;
        bytestash)
            printf '"username":"admin","password":"lab-test-golden","jwt_secret":"test-golden-secret-key-for-lab"'
            ;;
        paperless)
            # DRF Token — baked into golden snapshot DB
            printf '"token":"25a9e0b34ca3485d620783a9bcc7f7a7febff339"'
            ;;
        gotify)
            # App token — baked into golden snapshot DB
            printf '"app_token":"AjoHfnf2U3AOJ5d"'
            ;;
        plex|overseerr|apprise|qdrant)
            # No secrets needed
            ;;
    esac
}
```

**Note:** Services with DB-stored tokens (memos, linkding, paperless, gotify) have their tokens baked into the golden snapshot. Since clones are copy-on-write from the same snapshot, the same tokens work in every run. The `extract_secrets` function returns these known values. Only config-file-based credentials (servarr, tautulli, sabnzbd) need runtime extraction from the container filesystem.

- [ ] **Step 6: Re-run the host contract test**

Run: `bash tests/live_host_contract_test.sh`
Expected: PASS against real `backup-node` provisioning.

- [ ] **Step 7: Run contract tests for multiple profiles**

Test at minimum `servarr-core` and one non-servarr profile (e.g., `notifications` — fast, no sidecars, no auth):

```bash
bash tests/live_host_contract_test.sh servarr-core
bash tests/live_host_contract_test.sh notifications
```

- [ ] **Step 8: Run contract test for the `notes` profile (sidecar validation)**

The `notes` profile includes Paperless which needs a Redis sidecar. This exercises the sidecar lifecycle:

```bash
bash tests/live_host_contract_test.sh notes
```

Verify the manifest includes `paperless` with a valid URL and `secrets.paperless.token`.

- [ ] **Step 9: Commit all-profile provisioning**

```bash
git add bin/live-host fixtures/live/profiles/ fixtures/live/services/ tests/live_host_contract_test.sh
git commit -m "feat: provision all 15 live services on backup-node with profile-driven orchestration"
```

## Task 9: Implement the Live Runner and First End-to-End Slice

**Files:**
- Create: `crates/lab/src/live/runner.rs`
- Create: `crates/lab/src/live/report.rs`
- Create: `tests/live_servarr_core_e2e_test.sh`
- Modify: `crates/lab/src/cli/live.rs`
- Modify: `Justfile`
- Test: `crates/lab/tests/live_runner.rs`
- Test: `tests/live_servarr_core_e2e_test.sh`

- [ ] **Step 1: Write the failing report serialization test**

```rust
use lab::live::report::write_results;

#[test]
fn writes_machine_readable_results() {
    let dir = tempfile::tempdir().unwrap();
    write_results(dir.path(), &[]).unwrap();
    assert!(dir.path().join("results.json").exists());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p lab --test live_runner writes_machine_readable_results -- --exact`
Expected: FAIL because report writing does not exist yet.

- [ ] **Step 3: Implement the runner for one vertical slice**

**Disk-space preflight** — add this check at the start of `report.rs::init_run_dir()`:
```rust
fn check_disk_space(dir: &std::path::Path) -> anyhow::Result<()> {
    // statvfs or nix::sys::statvfs; require at least 50MB free
    // Bail with a clear error before any case executes — a failed flush mid-run
    // loses completed destructive case results with no recovery path
    let stat = nix::sys::statvfs::statvfs(dir)?;
    let free_mb = stat.blocks_free() * stat.block_size() / (1024 * 1024);
    anyhow::ensure!(free_mb >= 50, "insufficient disk space in {}: {}MB free (need 50MB)", dir.display(), free_mb);
    Ok(())
}
```

For `servarr-core`, execute at least:
- one CLI case for Radarr
- one MCP case for Radarr
- one API case for Radarr
- one expected failure case

**Case execution is sequential** (no semaphore for this slice). Sequential execution produces non-interleaved logs and cleaner failure attribution. Add parallelism in Task 10 only if profiling shows it is needed.

- [ ] **Step 4: Write the shell E2E test first**

```bash
#!/usr/bin/env bash
set -euo pipefail

run_json=$(cargo run --all-features -- live up servarr-core --json)
run_id=$(echo "$run_json" | jq -r '.run_id')
trap 'cargo run --all-features -- live down "$run_id"' EXIT

cargo run --all-features -- live test servarr-core
test -f "artifacts/live/$run_id/results.json"
jq -e '.cases | length > 0' "artifacts/live/$run_id/results.json"
```

- [ ] **Step 5: Run the shell E2E test to verify it fails**

Run: `bash tests/live_servarr_core_e2e_test.sh`
Expected: FAIL because `live test` and report writing are not implemented yet.

- [ ] **Step 6: Implement result writing with incremental checkpoint**

**Incremental checkpoint:** Append each case result to a newline-delimited log (`results.ndjson`) as it completes. Write the final `results.json` only after all cases finish. This ensures partial results survive a SIGKILL mid-run:

```rust
// After each case:
let line = serde_json::to_string(&case_result)? + "\n";
results_log.write_all(line.as_bytes())?;
results_log.flush()?;
results_log.get_ref().sync_data()?;  // required for SIGKILL durability (flush() alone is not enough)
// After all cases:
write_results_json(dir, &all_results)?;
```

**Credential handling:** Credentials are held in-memory from the manifest pipe (never written to `artifacts/`). The `secrets` key is stripped from the manifest before writing `artifacts/live/<run_id>/manifest.json`.

Artifacts written to `artifacts/live/<run_id>/`:
- `manifest.json` — **public fields only** (run_id, profile, services.{name}.url, snapshot_versions, network)
- `results.ndjson` — incremental append log
- `results.json` — final consolidated results
- `summary.txt` — human-readable summary

`artifacts/` is already in `.gitignore` (added in Task 1 Step 0).

- [ ] **Step 7: Re-run the Rust and shell tests**

Run:
- `cargo test -p lab --test live_runner`
- `bash tests/live_servarr_core_e2e_test.sh`

Expected: PASS.

- [ ] **Step 8: Commit the first end-to-end slice**

```bash
git add crates/lab/src/live/runner.rs crates/lab/src/live/report.rs crates/lab/src/cli/live.rs Justfile crates/lab/tests/live_runner.rs tests/live_servarr_core_e2e_test.sh
git commit -m "feat: add servarr-core live end-to-end runner"
```

## Task 10: Add Catalog-Driven Gap Analysis and Full Matrix Execution

> This task introduces `live/matrix.rs` (which absorbs catalog enumeration as a private function — no separate `live/catalog.rs` module). Deferred from the original Tasks 6 and 7. These are now grounded — the runner from Task 9 is proven, so the catalog enumeration and matrix classification have a concrete foundation.

**Files:**
- Create: `crates/lab/src/live/matrix.rs` (owns both `enumerate_surface_items()` as a private function and the `MatrixRow` matching logic — no separate `catalog.rs` needed)
- Modify: `crates/lab/src/live/runner.rs`
- Modify: `crates/lab/src/live/types.rs`
- Modify: `fixtures/live/services/radarr.json`
- Modify: `fixtures/live/services/sonarr.json`
- Modify: `fixtures/live/services/prowlarr.json`
- Test: `crates/lab/tests/live_matrix.rs` (replaces the previously planned `live_catalog.rs` — no separate catalog test file needed)
- Test: `tests/live_servarr_core_e2e_test.sh`

- [ ] **Step 1: Add the `CaseOutcome` enum to `types.rs`**

```rust
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum CaseOutcome {
    Pass,
    Fail { kind: String, message: String },
    /// Destructive case excluded because `--allow-destructive` was not passed.
    /// Named SkipDestructive (not SkipReadonly) — there is no readonly mode.
    SkipDestructive,
    /// Catalog item has no corresponding fixture case.
    SkipNoFixture,
    /// Destructive MCP case excluded because MCP elicitation is not yet implemented.
    /// Remove this variant once MCP elicitation is wired and tested.
    SkipNoElicitation,
}
```

The report summary must show counts for all five buckets, not just pass/fail.

- [ ] **Step 2: Implement catalog enumeration**

Source MCP actions from `build_catalog()` — **not** from source file parsing. The `enumerate_surface_items()` function is private to `matrix.rs` (one caller, no reason for a separate module). The result is `("mcp", service_name, action_name)` tuples, derived by iterating the existing `Catalog` type. Source API surface the same way. Avoid dynamic CLI enumeration for Tier-2 stub services — hardcode the 3 servarr CLI surfaces for now.

Take the catalog snapshot once at `lab live test` startup; pass the derived cases as plain data to the runner. Do not re-query the registry inside each case loop iteration.

- [ ] **Step 3: Write and verify matrix tests**

```rust
// crates/lab/tests/live_matrix.rs
// (no separate live_catalog.rs — enumerate_surface_items is private to matrix.rs)
#[test]
fn enumerates_radarr_mcp_surface() {
    // call through the public matrix::build_matrix() API
    let items = lab::live::matrix::enumerate_for_test();  // expose test-only via #[cfg(test)]
    assert!(items.iter().any(|it| it.surface == "mcp" && it.service == "radarr"));
}
#[test]
fn no_duplicate_surface_items() {
    let items = lab::live::matrix::enumerate_for_test();
    let mut seen = std::collections::HashSet::new();
    for it in &items { assert!(seen.insert((&it.surface, &it.service, &it.operation))); }
}
```

- [ ] **Step 4: Implement matrix matching**

Rules:
- exact match on `surface/service/operation`
- unmatched generated items → `CaseOutcome::SkipNoFixture`
- fixture-defined destructive cases without `--allow-destructive` → `CaseOutcome::SkipDestructive`
- fixture-defined destructive cases on MCP surface → `CaseOutcome::SkipNoElicitation` (regardless of `--allow-destructive`)
- duplicate live case definitions are rejected at load time

- [ ] **Step 5: Write and verify matrix tests**

- [ ] **Step 6: Extend runner to use full catalog matrix for `servarr-core`**

Requirements:
- enumerate all generated items for services in the profile
- match to fixture cases
- **sequential execution** — no semaphore for this task. Add concurrency only if profiling shows sequential is a bottleneck (natural threshold: >30s for a full profile run).
- record all five outcome buckets without hiding any

- [ ] **Step 7: Extend `tests/live_servarr_core_e2e_test.sh`**

Add assertions that:
- result file contains multiple surfaces
- summary includes at least one `Pass` and any `SkipNoFixture` count

- [ ] **Step 8: Re-run the full test suite**

Run:
- `cargo test -p lab --test live_matrix`
- `bash tests/live_servarr_core_e2e_test.sh`

Expected: PASS.

- [ ] **Step 9: Commit matrix execution**

```bash
git add crates/lab/src/live/matrix.rs crates/lab/src/live/runner.rs crates/lab/src/live/types.rs fixtures/live/services/radarr.json fixtures/live/services/sonarr.json fixtures/live/services/prowlarr.json crates/lab/tests/live_matrix.rs tests/live_servarr_core_e2e_test.sh
git commit -m "feat: catalog-driven gap analysis and full matrix execution for servarr-core"
```

## Task 11: Add Documentation and Repo-Level Integration

**Files:**
- Modify: `docs/README.md`
- Modify: `docs/TESTING.md`
- Modify: `docs/OPERATIONS.md`
- Modify: `docs/OBSERVABILITY.md`
- Create: `docs/coverage/live.md`
- Modify: `docs/LIVE_TESTING.md`
- Modify: `Justfile`

> `.gitignore` is NOT in this file list — it was handled in Task 1 Step 0.

- [ ] **Step 1: Verify `.gitignore` is already in place**

Run: `git check-ignore artifacts/live/test-run-123/manifest.json`
Expected: match. If not, something went wrong in Task 1 Step 0 — fix before continuing.

- [ ] **Step 2: Write docs assertions**

Document:
- `backup-node` is the canonical automated live environment
- live tests are opt-in and excluded from normal CI
- artifacts live under `artifacts/live/<run_id>/` (gitignored since Task 1)
- destructive live tests require `--allow-destructive` flag; MCP destructive cases are always `SkipNoElicitation` until elicitation is implemented
- golden snapshots must use clean-seed test-only credentials — never snapshot a production service
- one-time setup: `just install-live-host` (stages `bin/live-host` onto backup-node) and `ssh-keyscan backup-node >> ~/.labby/known_hosts` (adds backup-node host key)

- [ ] **Step 3: Update the docs**

Touch:
- docs index
- testing contract
- operations workflow (one-time setup: `just install-live-host`, `ssh-keyscan backup-node >> ~/.labby/known_hosts`)
- observability contract if artifact fields are now part of verification
- new live coverage doc with current fixture/gap status

- [ ] **Step 4: Add final `Justfile` targets**

Targets:
- `live-test PROFILE` — full run: up, test, down
- `live-test-integration` — alias for the first real live profile
- `live-cleanup` — run orphan cleanup on backup-node

All targets must include SSH timeout options.

- [ ] **Step 5: Run narrow doc-adjacent verification**

Run:
- `just live-test servarr-core`
- `rg -n "LIVE_TESTING|backup-node|artifacts/live" docs Justfile`
- `git check-ignore artifacts/live/test-run-123/manifest.json`

Expected:
- live run succeeds
- docs point to the right commands and paths
- artifacts/ is gitignored

- [ ] **Step 6: Commit docs and integration wiring**

```bash
git add docs/README.md docs/TESTING.md docs/OPERATIONS.md docs/OBSERVABILITY.md docs/LIVE_TESTING.md docs/coverage/live.md Justfile
git commit -m "docs: add live testing workflow and coverage docs"
```

## Task 12: Final Verification

**Files:**
- Verify only; no new files required.

- [ ] **Step 1: Run CI-safe crate verification**

Run:
- `cargo test -p lab --test live_config`
- `cargo test -p lab --test live_matrix`
- `cargo test -p lab --test live_runner`

Expected: PASS. (No `live_catalog` test — catalog enumeration is tested via `live_matrix`.)

- [ ] **Step 2: Run broader crate verification**

Run: `cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features`
Expected: PASS.

- [ ] **Step 3: Run the live vertical-slice verification**

Run:
- `bash tests/live_host_contract_test.sh`
- `bash tests/live_servarr_core_e2e_test.sh`

Expected: PASS against `backup-node`.

- [ ] **Step 4: Inspect artifacts and teardown behavior**

Verify:
- artifact directory exists and contains manifest/results/summary
- repeated `live down` or `cleanup` calls are idempotent
- no leaked per-run datasets or networks remain on `backup-node`

- [ ] **Step 5: Record any pending gaps explicitly**

If additional services are not yet onboarded, write them down in:
- `docs/coverage/live.md`
- fixture backlog notes in `fixtures/live/README.md`

- [ ] **Step 6: Final commit**

```bash
git status --short
# Stage specific files only — never git add -A (would stage artifacts/ if gitignore missed)
git add crates/lab/src/live/ crates/lab/src/cli/live.rs crates/lab/tests/live_*.rs \
    bin/live-host bin/live-cleanup fixtures/live/ tests/live_*.sh docs/ Justfile
git commit -m “feat: add backup-node-backed live end-to-end test infrastructure”
```

## Notes for the Implementer

- Keep TDD real. Write the failing test or shell contract check first for each task.
- Do not push `backup-node` orchestration logic into `lab-apis`.
- Prefer small, typed Rust modules for in-repo logic and thin shell for host-side orchestration.
- Treat fixture manifests and service fixture files as product contracts for the test system.
- Use `ssh -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -o StrictHostKeyChecking=yes -o UserKnownHostsFile=~/.labby/known_hosts` for all automation — never bare `ssh`.
- Use `tokio::process::Command` with `kill_on_drop(true)` for every SSH subprocess. See `crates/lab/src/live/CLAUDE.md` for the full rules.
- Validate `run_id` and `profile` against `^[a-z0-9][a-z0-9_-]{0,63}$` before any SSH or ZFS operation. The Rust validator in `validation.rs` and the bash `validate_id()` must match exactly — add `# SYNC:` comments cross-referencing both.
- Docker port binding MUST use `127.0.0.1:0:<port>` — never `0:<port>`. Using `0.0.0.0` exposes live test services to the LAN.
- `bin/live-host up` exits after `docker run` completes. Readiness polling is done in Rust via `wait_for_services()` (parallel, not sequential).
- Credentials live only in the stdout pipe and Rust process memory. Never write credentials to disk on any host.
- Never use `git add -A` — always stage specific paths to avoid committing artifacts/.
- The first milestone is not “all services”; it is “one profile works end to end with teardown and artifacts.” Expand only after that is solid.
- All 15 services have golden snapshots on backup-node — see the recon section for credentials and mount paths.
- Services with limited API (Plex unclaimed, Overseerr setup-pending) should have minimal fixture cases that test what IS available.
- Paperless requires a Redis sidecar — `bin/live-host` must start redis before paperless and inject the IP via `PAPERLESS_REDIS` env var.
- Volume mount paths vary by service — see the readiness endpoints table in the recon section. The `VMOUNT` case statement in `bin/live-host` is the single source of truth for mount paths.
- The `PROFILE_SERVICES` associative array in `bin/live-host` is the single source of truth for which services belong to which profile. Keep it in sync with `fixtures/live/profiles/*.json`.
