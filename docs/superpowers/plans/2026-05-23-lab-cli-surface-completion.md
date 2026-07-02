# Lab CLI Surface Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish or remove the misleading incomplete Lab CLI surfaces so `labby --help`, generated docs, MCP/API action catalogs, and the Lab plugin `using-lab-cli` skill describe behavior that actually works.

**Architecture:** Keep filesystem and environment mutation in the `labby` binary crate, not `lab-apis`. Route `extract.apply` and `extract.diff` through the canonical `config::env_merge` preview/write path so diff and apply semantics cannot drift. Remove the legacy top-level install/init stubs because their real ownership now lives under `setup`, `marketplace`, and `registry`.

**Tech Stack:** Rust 2024, clap/clap_complete, serde/serde_json, axum dispatch helpers, rmcp action catalog, cargo-nextest, generated docs, Lab plugin skills.

---

## Research And Review Decisions

- `labby install`, `labby uninstall`, and `labby init` are visible but stubbed. Remove them from clap instead of hiding them, so scripts fail clearly and users use the real surfaces: `labby setup`, `labby setup install-plugin`, `labby marketplace`, and `labby registry install`.
- `labby completions <shell>` already has an implementation file but is not wired into `cli.rs`. Make `clap_complete` a normal dependency because completions are a normal CLI surface.
- `labby extract` remains flag-based: `labby extract [URI] --apply|--diff`. Do not introduce `labby extract scan/apply` subcommands.
- `extract.apply` and `extract.diff` are advertised through MCP/API today but return stubs. Implement them or remove the advertised actions. This plan implements them.
- Do not implement old homelab service stubs. The repo has pivoted to gateway/operator surfaces.
- `labby mcp` is stdio MCP. `labby serve` is HTTP/node runtime.
- Engineering review requirements applied here: canonical env preview, no duplicate env parser, redacted conflict output, constrained `env_path`, symlink/path-safety tests, `services` filtering, mtime conflict detection, and diff-first skill guidance.

## Implementation Progress

- Completed: removed top-level `install`/`uninstall`/`init` stubs and regenerated CLI docs.
- Completed: wired `labby completions <shell>` and made `clap_complete` a normal dependency.
- Completed: added canonical `.env` preview classification with redacted conflict warnings.
- Completed: switched CLI `extract --apply/--diff` to the canonical merge path.
- Completed: implemented MCP/API `extract.apply` and `extract.diff`, including `services` filtering and gated `env_path` overrides.
- Completed: updated `plugins/lab/skills/using-lab-cli` and its config reference.
- Verification: `cargo check -p labby --lib`, `cargo run -p labby -- docs check`, `target/debug/labby completions bash`, and `target/debug/labby install` rejection passed.
- Blocked verification: `cargo test -p labby cli::tests:: --lib` still fails before running CLI tests because `crates/lab/src/mcp/server.rs` references missing `tool_search_schema_visible` in its test module.

## File Map

- Modify: `crates/lab/Cargo.toml` — make `clap_complete` non-optional.
- Modify: `crates/lab/src/cli.rs` — wire `completions`, remove stub install/init commands, remove stale service enum comments.
- Delete if unused: `crates/lab/src/cli/install.rs`.
- Modify: `crates/lab/src/cli/completions.rs` — update comments and add focused output tests.
- Modify: `crates/lab/src/cli/extract.rs` — keep human output stable; switch apply writes to canonical merge helper.
- Modify: `crates/lab/src/dispatch/extract.rs` — implement safe redacted `diff` and `apply`.
- Modify: `crates/lab/src/config.rs` — expose credential-to-env-entry conversion and mtime-aware `write_service_creds`.
- Modify: `crates/lab/src/config/env_merge.rs` — add redacted preview/classification and redact skipped conflict output.
- Modify: `docs/services/EXTRACT.md`, `docs/surfaces/CLI.md`, `docs/generated/*`.
- Modify: `plugins/lab/skills/using-lab-cli/SKILL.md` and references.

## Task 1: Remove Misleading Top-Level Install/Init Stubs

**Files:**
- Modify: `crates/lab/src/cli.rs`
- Delete if unused: `crates/lab/src/cli/install.rs`
- Modify: `docs/surfaces/CLI.md`
- Test: `crates/lab/src/cli.rs`

- [ ] **Step 1: Add parser tests documenting the desired surface**

Add tests in `crates/lab/src/cli.rs`:

```rust
#[test]
fn cli_rejects_legacy_install_uninstall_init_stubs() {
    for command in ["install", "uninstall", "init"] {
        let err = Cli::try_parse_from(["labby", command]).expect_err("legacy stub must be gone");
        assert!(err.to_string().contains("unrecognized subcommand"), "{command}: {err}");
    }
}

#[test]
fn replacement_setup_commands_parse() {
    let cli = Cli::try_parse_from(["labby", "setup"]).expect("setup parses");
    assert!(matches!(cli.command, Command::Setup(_)));

    let cli = Cli::try_parse_from(["labby", "setup", "install-plugin", "gateway", "-y"])
        .expect("setup install-plugin parses");
    assert!(matches!(cli.command, Command::Setup(_)));
}
```

- [ ] **Step 2: Run focused tests and verify failure**

Run:

```bash
cargo test -p labby cli_rejects_legacy_install_uninstall_init_stubs replacement_setup_commands_parse --lib
```

Expected: first test fails because the legacy commands still parse.

- [ ] **Step 3: Remove the legacy clap variants**

In `crates/lab/src/cli.rs`, remove `pub mod install;`, the `Install`, `Uninstall`, and `Init` enum variants, and their dispatch arms. Delete `crates/lab/src/cli/install.rs` if no references remain.

- [ ] **Step 4: Update CLI docs**

In `docs/surfaces/CLI.md`, replace top-level install/init guidance with:

```markdown
- First-run setup: `labby setup`
- Lab service plugin lifecycle: `labby setup install-plugin <service> -y` and `labby setup uninstall-plugin <service> -y`
- MCP Registry installation: `labby marketplace mcp.install --params '{...}' -y` or the `labby registry install` shim where available
```

- [ ] **Step 5: Run focused verification**

Run:

```bash
cargo test -p labby cli_rejects_legacy_install_uninstall_init_stubs replacement_setup_commands_parse --lib
```

Expected: PASS.

## Task 2: Wire `labby completions <shell>`

**Files:**
- Modify: `crates/lab/Cargo.toml`
- Modify: `crates/lab/src/cli.rs`
- Modify: `crates/lab/src/cli/completions.rs`
- Test: `crates/lab/src/cli.rs`, `crates/lab/src/cli/completions.rs`

- [ ] **Step 1: Add failing parser and output tests**

In `crates/lab/src/cli.rs`, add:

```rust
#[test]
fn cli_parses_completions_subcommand() {
    let cli = Cli::try_parse_from(["labby", "completions", "bash"]).expect("completions parses");
    assert!(matches!(cli.command, Command::Completions(_)));
}
```

In `crates/lab/src/cli/completions.rs`, add:

```rust
pub fn render_for_test(shell: Shell) -> String {
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    let mut out = Vec::new();
    generate(shell, &mut cmd, bin_name, &mut out);
    String::from_utf8(out).expect("completion output is utf8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_completion_mentions_labby_and_current_commands() {
        let rendered = render_for_test(Shell::Bash);
        assert!(rendered.contains("labby"));
        assert!(rendered.contains("completions"));
        assert!(rendered.contains("extract"));
        assert!(!rendered.contains(" install "));
        assert!(!rendered.contains(" init "));
        assert!(rendered.len() < 512_000, "completion output unexpectedly large");
    }
}
```

- [ ] **Step 2: Run focused tests and verify failure**

Run:

```bash
cargo test -p labby cli_parses_completions_subcommand bash_completion_mentions_labby_and_current_commands --lib
```

Expected: compile or test failure because `completions` is not wired and `clap_complete` is optional.

- [ ] **Step 3: Make `clap_complete` non-optional**

In `crates/lab/Cargo.toml`, change:

```toml
clap_complete = { workspace = true, optional = true }
```

to:

```toml
clap_complete.workspace = true
```

- [ ] **Step 4: Wire the command**

In `crates/lab/src/cli.rs`, add `pub mod completions;`, add `Completions(completions::CompletionsArgs)` near `Help`, add `Command::Completions(args) => completions::run(&args)`, and move the stale `Generate shell completions` comment so it no longer describes `Extract`.

- [ ] **Step 5: Run focused verification**

Run:

```bash
cargo test -p labby cli_parses_completions_subcommand bash_completion_mentions_labby_and_current_commands --lib --all-features
cargo check -p labby --no-default-features
```

Expected: PASS. The reduced-feature check proves completions wiring does not depend on the `all` feature.

## Task 3: Add Canonical Env Merge Preview And Redaction

**Files:**
- Modify: `crates/lab/src/config/env_merge.rs`
- Modify: `crates/lab/src/config.rs`
- Test: `crates/lab/src/config/env_merge.rs`

- [ ] **Step 1: Add failing preview/redaction tests**

In `crates/lab/src/config/env_merge.rs`, add:

```rust
#[test]
fn preview_classifies_entries_with_canonical_quote_semantics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let env_path = dir.path().join(".env");
    std::fs::write(
        &env_path,
        "RADARR_URL=\"http://radarr.local\"\nRADARR_API_KEY=\"old secret\"\n",
    )
    .expect("write env");

    let preview = preview(
        &env_path,
        MergeRequest {
            entries: vec![
                EnvEntry::new("RADARR_URL", "http://radarr.local"),
                EnvEntry::new("RADARR_API_KEY", "new secret"),
                EnvEntry::new("PROWLARR_URL", "http://prowlarr.local"),
            ],
            force: false,
            expected_mtime: None,
        },
    )
    .expect("preview");

    assert!(preview.entries.iter().any(|entry| entry.key == "RADARR_URL" && entry.status == PreviewStatus::Same));
    assert!(preview.entries.iter().any(|entry| entry.key == "RADARR_API_KEY" && entry.status == PreviewStatus::Conflict));
    assert!(preview.entries.iter().any(|entry| entry.key == "PROWLARR_URL" && entry.status == PreviewStatus::New));
    let rendered = serde_json::to_string(&preview).expect("json");
    assert!(!rendered.contains("old secret"));
    assert!(!rendered.contains("new secret"));
}

#[test]
fn merge_conflict_warning_does_not_include_existing_or_new_value() {
    let dir = tempfile::tempdir().expect("tempdir");
    let env_path = dir.path().join(".env");
    std::fs::write(&env_path, "RADARR_API_KEY=old-secret\n").expect("write env");

    let outcome = merge(
        &env_path,
        MergeRequest {
            entries: vec![EnvEntry::new("RADARR_API_KEY", "new-secret")],
            force: false,
            expected_mtime: None,
        },
    )
    .expect("merge");

    let rendered = format!("{:?}", outcome.skipped);
    assert!(rendered.contains("RADARR_API_KEY"));
    assert!(!rendered.contains("old-secret"));
    assert!(!rendered.contains("new-secret"));
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p labby preview_classifies_entries_with_canonical_quote_semantics merge_conflict_warning_does_not_include_existing_or_new_value --lib --all-features
```

Expected: FAIL because `preview` and `PreviewStatus` do not exist and conflict warnings currently include the existing value.

- [ ] **Step 3: Implement preview types and shared classification**

In `env_merge.rs`, add:

```rust
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct MergePreview {
    pub entries: Vec<PreviewEntry>,
    pub written: usize,
    pub skipped: usize,
    pub force: bool,
    #[serde(skip)]
    pub expected_mtime: Option<SystemTime>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct PreviewEntry {
    pub key: String,
    pub status: PreviewStatus,
}

#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreviewStatus {
    New,
    Same,
    Conflict,
    Overwrite,
}
```

Add `pub fn preview(path: &Path, req: MergeRequest) -> Result<MergePreview, MergeError>`. Extract shared helpers from `merge` so both functions reuse the same file reading, `strip_quotes`, duplicate request-key collapse, and classification rules.

- [ ] **Step 4: Redact merge conflict warnings**

Change skipped conflict output in `merge` to:

```rust
skipped.push(format!(
    "CONFLICT: {} already set; skipping (set force=true to overwrite)",
    entry.key
));
```

- [ ] **Step 5: Add credential entry conversion and mtime-aware write helper**

In `crates/lab/src/config.rs`, add:

```rust
pub fn service_creds_to_env_entries(creds: &[ServiceCreds]) -> Vec<env_merge::EnvEntry> {
    let mut entries = Vec::new();
    for cred in creds {
        let svc_upper = cred.service.to_uppercase();
        if let Some(url) = &cred.url {
            entries.push(env_merge::EnvEntry::new(format!("{svc_upper}_URL"), url.clone()));
        }
        if let Some(secret) = &cred.secret {
            entries.push(env_merge::EnvEntry::new(cred.env_field.clone(), secret.clone()));
        }
    }
    entries
}
```

Update `write_service_creds` to call this helper and accept `expected_mtime: Option<SystemTime>`.

- [ ] **Step 6: Run focused verification**

Run:

```bash
cargo test -p labby preview_classifies_entries_with_canonical_quote_semantics merge_conflict_warning_does_not_include_existing_or_new_value --lib --all-features
```

Expected: PASS.

## Task 4: Implement Safe `extract.diff` And `extract.apply`

**Files:**
- Modify: `crates/lab/src/dispatch/extract.rs`
- Modify: `crates/lab/src/cli/extract.rs`
- Modify: `crates/lab/src/config.rs`
- Test: `crates/lab/src/dispatch/extract.rs`, `crates/lab/src/cli/extract.rs`

- [ ] **Step 1: Add failing dispatch/helper tests**

Use the current type shape from `crates/lab-apis/src/extract/types.rs`. Add:

```rust
fn test_extract_report_with_radarr_and_sonarr() -> lab_apis::extract::ExtractReport {
    lab_apis::extract::ExtractReport {
        target: ScanTarget::Targeted("/tmp/appdata".parse().expect("uri")),
        uri: Some("/tmp/appdata".parse().expect("uri")),
        found: vec!["radarr".to_string(), "sonarr".to_string()],
        creds: vec![
            lab_apis::extract::ServiceCreds {
                service: "radarr".to_string(),
                url: Some("http://radarr".to_string()),
                secret: Some("radarr-secret".to_string()),
                env_field: "RADARR_API_KEY".to_string(),
                source_host: None,
                probe_host: None,
                runtime: None,
                url_verified: false,
            },
            lab_apis::extract::ServiceCreds {
                service: "sonarr".to_string(),
                url: Some("http://sonarr".to_string()),
                secret: Some("sonarr-secret".to_string()),
                env_field: "SONARR_API_KEY".to_string(),
                source_host: None,
                probe_host: None,
                runtime: None,
                url_verified: false,
            },
        ],
        warnings: vec![],
    }
}

#[tokio::test]
async fn extract_diff_requires_targeted_uri() {
    let err = dispatch("diff", serde_json::json!({})).await.expect_err("missing uri");
    assert_eq!(err.kind(), "missing_param");
}

#[tokio::test]
async fn extract_apply_rejects_missing_targeted_uri() {
    let err = dispatch("apply", serde_json::json!({})).await.expect_err("missing uri");
    assert_eq!(err.kind(), "missing_param");
}

#[test]
fn apply_report_uses_canonical_merge_and_redacted_plan() {
    let dir = tempfile::tempdir().expect("tempdir");
    let env_path = dir.path().join(".env");
    let report = test_extract_report_with_radarr_and_sonarr();

    let applied = apply_report_to_env(&report, &env_path, false, None).expect("apply");
    let written = std::fs::read_to_string(&env_path).expect("env written");
    assert!(written.contains("RADARR_URL=http://radarr"));
    assert!(written.contains("SONARR_API_KEY=sonarr-secret"));
    let json = serde_json::to_string(&applied).expect("json");
    assert!(!json.contains("radarr-secret"));
    assert!(!json.contains("sonarr-secret"));
}

#[test]
fn apply_report_respects_services_filter() {
    let dir = tempfile::tempdir().expect("tempdir");
    let env_path = dir.path().join(".env");
    let filtered = filter_report_services(
        test_extract_report_with_radarr_and_sonarr(),
        &["sonarr".to_string()],
    )
    .expect("filter");

    apply_report_to_env(&filtered, &env_path, false, None).expect("apply");

    let written = std::fs::read_to_string(&env_path).expect("env written");
    assert!(written.contains("SONARR_URL="));
    assert!(!written.contains("RADARR_URL="));
}

#[cfg(unix)]
#[test]
fn env_path_rejects_symlink() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("target.env");
    let link = dir.path().join(".env");
    std::fs::write(&target, "").expect("target");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");
    assert!(validate_env_path_for_write(&link).is_err());
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p labby extract_diff_requires_targeted_uri extract_apply_rejects_missing_targeted_uri apply_report_uses_canonical_merge_and_redacted_plan apply_report_respects_services_filter env_path_rejects_symlink --lib --all-features
```

Expected: helper tests fail because the helpers do not exist and apply/diff still hit stubs.

- [ ] **Step 3: Add redacted extract plan/outcome types**

In `dispatch/extract.rs`, add serializable output types that contain keys/status only, no values:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct ExtractWritePlan {
    pub env_path: PathBuf,
    pub entries: Vec<crate::config::env_merge::PreviewEntry>,
    pub written: usize,
    pub skipped: usize,
    pub force: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtractApplyOutcome {
    pub plan: ExtractWritePlan,
    pub backup_path: Option<PathBuf>,
    pub skipped: Vec<String>,
    pub pruned_backups: usize,
}
```

- [ ] **Step 4: Add safe env-path resolution**

Keep arbitrary `--env-path` as local CLI behavior only. For dispatch/API/MCP, default to `$HOME/.labby/.env` and reject `env_path` unless `LAB_ALLOW_EXTRACT_ENV_PATH_OVERRIDE=1` is set for tests.

Add `validate_env_path_for_write(path)`:

- reject symlink final components using `std::fs::symlink_metadata`
- canonicalize an existing parent
- reject file names other than `.env`
- map invalid paths to `ToolError::InvalidParam`

- [ ] **Step 5: Add force and services parsing**

Add:

```rust
fn parse_force(params: &Value) -> Result<bool, ToolError> { /* bool only */ }
fn parse_services(params: &Value) -> Result<Option<Vec<String>>, ToolError> { /* array of strings only */ }
fn filter_report_services(report: ExtractReport, services: &[String]) -> Result<ExtractReport, ToolError> { /* lowercase filter */ }
```

Do not leave `services` advertised and ignored.

- [ ] **Step 6: Add preview/apply helpers**

Use `env_merge::snapshot_mtime`, `env_merge::preview`, and mtime-aware `write_service_creds`. Do not parse `.env` in dispatch.

```rust
pub(crate) fn build_write_plan(
    report: &lab_apis::extract::ExtractReport,
    env_path: &Path,
    force: bool,
) -> Result<(ExtractWritePlan, Option<std::time::SystemTime>), ToolError> {
    validate_env_path_for_write(env_path)?;
    let mtime = crate::config::env_merge::snapshot_mtime(env_path);
    let preview = crate::config::env_merge::preview(
        env_path,
        crate::config::env_merge::MergeRequest {
            entries: crate::config::service_creds_to_env_entries(&report.creds),
            force,
            expected_mtime: mtime,
        },
    )
    .map_err(map_merge_error)?;
    Ok((
        ExtractWritePlan {
            env_path: env_path.to_path_buf(),
            entries: preview.entries,
            written: preview.written,
            skipped: preview.skipped,
            force,
        },
        mtime,
    ))
}
```

`apply_report_to_env(report, env_path, force, expected_mtime)` must call `write_service_creds(env_path, &report.creds, force, expected_mtime)` and return redacted `ExtractApplyOutcome`.

- [ ] **Step 7: Wire dispatch actions**

Replace the current `apply`/`diff` stubs. Both actions must require a targeted `uri`, parse `force`, parse and apply `services`, resolve the safe env path, scan the target, and return redacted plan/outcome JSON. `apply` remains destructive in `ACTIONS`. Add `force` to `apply` and `diff` params; keep `services` only if implemented.

- [ ] **Step 8: Keep CLI output stable but use canonical merge for writes**

Do not rewrite CLI human output wholesale. Preserve current `labby extract --diff` and `--apply --dry-run` behavior unless tests prove it must change. Replace the write path so `--apply` uses the mtime-aware canonical `write_service_creds` instead of `backup_env` + `write_env`.

If local CLI diff continues to print raw values, document that it is a local-only terminal workflow and never used by API/MCP. Add a test or smoke check showing API/MCP JSON never includes raw secrets.

- [ ] **Step 9: Run focused verification**

Run:

```bash
cargo test -p labby extract_ --lib --all-features
```

Expected: extract tests pass and `apply not yet implemented` no longer appears in dispatch tests.

## Task 5: Align Docs, Generated Artifacts, And Skill References

**Files:**
- Modify: `docs/services/EXTRACT.md`
- Modify: `docs/surfaces/CLI.md`
- Modify: `docs/generated/*`
- Modify: `plugins/lab/skills/using-lab-cli/SKILL.md`
- Modify: `plugins/lab/skills/using-lab-cli/references/service-catalog.md`
- Modify: `plugins/lab/skills/using-lab-cli/references/config-reference.md`

- [ ] **Step 1: Update hand-written docs**

Document:

```markdown
- `labby mcp` is the stdio MCP entrypoint.
- `labby serve` is the HTTP/node runtime.
- `labby extract [URI] --apply|--diff` is the CLI shape.
- `extract.diff` and `extract.apply` are available through MCP/API after destructive confirmation and admin-capable auth.
- `extract.diff` and `extract.apply` machine outputs are redacted and never include discovered or existing secret values.
- `env_path` override is local/test-only; API/MCP use the canonical Lab env file.
- Old homelab service stubs are not product surfaces; Lab is gateway/operator focused.
```

- [ ] **Step 2: Update plugin skill examples**

In `plugins/lab/skills/using-lab-cli/SKILL.md`, replace all `lab ...` examples with `labby ...`. Replace `labby serve # stdio` with:

```bash
labby mcp       # stdio MCP for local MCP clients
labby serve     # HTTP/node runtime, including /mcp
```

Replace extract examples with diff-first guidance:

```bash
labby extract
labby extract host:/path/to/appdata --diff
# Only after the user explicitly approves the diff:
labby extract host:/path/to/appdata --apply -y
```

Add prose: agents must run `--diff` first and must not pass `--env-path` unless the user names a Lab env file explicitly.

- [ ] **Step 3: Regenerate docs**

Run:

```bash
cargo run -p labby --all-features -- docs generate
```

Expected:

- generated CLI help includes `labby completions`
- generated help excludes top-level `install`, `uninstall`, and `init`
- generated extract action catalog includes implemented `extract.apply` and `extract.diff`
- generated docs contain no `not yet implemented` language for implemented actions
- generated docs do not contain concrete secret-looking values such as `API_KEY=`, `TOKEN=`, `PASSWORD=`, `SECRET=`, or bearer-looking sample tokens except obvious placeholders

- [ ] **Step 4: Run docs check**

Run:

```bash
just docs-check
```

Expected: PASS.

## Task 6: Final Verification

**Files:**
- All files touched by prior tasks.

- [ ] **Step 1: Focused Rust tests**

Run:

```bash
cargo test -p labby cli_ completions extract env_merge --lib --all-features
```

Expected: PASS.

- [ ] **Step 2: Early workspace compile check**

Run:

```bash
cargo check --workspace --all-features
```

Expected: PASS. This is an early compile gate; `just check` below may repeat it as part of the repo-standard final gate. If this fails on the pre-existing `tool_search_schema_visible` compile issue, fix that compile break in the same worktree before continuing because work-it requires a green worktree.

- [ ] **Step 3: Required repo gates**

Run:

```bash
just check
just lint
just test
just build
```

Expected: PASS.

- [ ] **Step 4: Runtime smoke checks**

Run:

```bash
LAB_LOG_DIR=/tmp/lab-logs ./target/debug/labby --help
LAB_LOG_DIR=/tmp/lab-logs ./target/debug/labby completions bash | head -20
LAB_LOG_DIR=/tmp/lab-logs ./target/debug/labby extract --help
```

Expected:

- `--help` includes `completions`
- `--help` does not include top-level `install`, `uninstall`, or `init`
- completion output mentions `labby`
- extract help shows `[URI]` plus `--apply` and `--diff`
