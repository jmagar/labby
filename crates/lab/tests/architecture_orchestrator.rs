#![allow(
    clippy::bool_assert_comparison,
    clippy::err_expect,
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::len_zero,
    clippy::manual_string_new,
    clippy::needless_raw_string_hashes,
    clippy::panic,
    clippy::single_char_pattern,
    clippy::unnested_or_patterns
)]
//! Architecture guards for the shared dispatch layer.
//!
//! Two enforcement concerns live here:
//!
//! 1. **Inter-service coupling allowlist (Arch-M1).** Every cross-service edge
//!    `dispatch::<a> → dispatch::<b>` must be justified. The allowlist below is
//!    the single source of truth; any new edge fails the test until it is added
//!    with a rationale. This generalizes the earlier single-edge guard (which
//!    only forbade peer imports of `crate::dispatch::setup`).
//!
//! 2. **Action-naming lint (Arch-M3).** Every non-builtin, non-deprecated-alias
//!    action name in a service catalog must match `<resource>.<verb>` dotted
//!    form (`^[a-z0-9_]+(\.[a-z0-9_]+)+$`). `help` / `schema` are exempt.
//!
//! Both checks use a source-text scan rather than a clippy lint because:
//! 1. Tests run automatically in CI.
//! 2. The failure message can name the architecture clause directly.
//! 3. No external clippy.toml configuration to keep in sync.
//!
//! The Bootstrap orchestrator exception (`setup → doctor`, see
//! `crates/lab/src/dispatch/CLAUDE.md` § "Orchestrator Exception") is encoded
//! as a normal allowlist edge below.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Dispatch services (top-level `dispatch/<service>.rs` entrypoints) whose
/// cross-service imports are governed by the allowlist. Shared subsystems
/// (`node`, `security`, `upstream`, `code_mode`) and shared leaf modules
/// (`error`, `helpers`, `redact`, `path_safety`, `clients`) are NOT
/// action-dispatched services and are always importable — they are the common
/// substrate, not peers. See `dispatch/CLAUDE.md` § "Shared subsystems".
const SHARED_NON_SERVICES: &[&str] = &[
    // Shared subsystems (not 4-file action-dispatched services):
    "node",
    "security",
    "upstream",
    // Shared leaf modules (utility substrate every service may use):
    "error",
    "helpers",
    "redact",
    "path_safety",
    "clients",
];

/// The full set of dispatch services that participate in the coupling check.
/// Derived from the directory listing at test time but pinned here so a newly
/// added service is forced through review (an unlisted service dir triggers a
/// failure in `services_list_is_current`).
const KNOWN_SERVICES: &[&str] = &[
    "acp",
    "deploy",
    "doctor",
    "fs",
    "gateway",
    "lab_admin",
    "logs",
    "marketplace",
    "setup",
    "snippets",
    "stash",
];

/// Allowed cross-service edges: `(consumer, permitted_sibling)`.
///
/// Each edge needs a rationale — a cross-service dependency is an
/// architecture decision, not an accident. The review (Arch-M1) found every
/// edge below already justified; do not add a new pair without recording why.
///
/// NOTE: `code_mode` is a submodule of `gateway` (Arch-M2 relocation is
/// DEFERRED), so a `snippets → gateway::code_mode` import is encoded here as
/// the edge `snippets → gateway`.
const ALLOWED_EDGES: &[(&str, &str)] = &[
    // gateway → upstream: gateway owns the upstream connection pool, circuit
    //   breaker, and stdio spawn — upstream IS gateway's runtime substrate.
    ("gateway", "upstream"),
    // doctor → upstream: doctor's gateway health check reads
    //   CIRCUIT_BREAKER_THRESHOLD / UpstreamHealth types to report breaker state.
    ("doctor", "upstream"),
    // doctor → gateway: doctor reads the live GatewayManager
    //   (current_gateway_manager) to audit configured upstreams.
    ("doctor", "gateway"),
    // setup → gateway: setup.settings.update refreshes the gateway's built-in
    //   service registry via current_gateway_manager after a config change.
    ("setup", "gateway"),
    // setup → doctor: the Bootstrap orchestrator exception —
    //   setup.draft.commit gates the .env merge on doctor::dispatch("audit.full").
    //   Dependency direction is one-way (setup → doctor; never doctor → setup).
    ("setup", "doctor"),
    // snippets → gateway: snippets reuses gateway::code_mode (the shared JS
    //   execution kernel) plus the GatewayManager handle. The Arch-M2
    //   relocation of code_mode to a top-level peer is DEFERRED, so this edge
    //   remains sanctioned for now.
    ("snippets", "gateway"),
    // marketplace → gateway: marketplace.mcp.* delegates install/list to
    //   gateway::dispatch and reads current_gateway_manager.
    ("marketplace", "gateway"),
    // marketplace → node: ACP install fan-out uses node::send to push installs
    //   to fleet nodes.
    ("marketplace", "node"),
    // marketplace → stash: marketplace forks persist adopted plugin components
    //   through the shared stash store/service (stash_bridge.rs reuses
    //   stash::store::StashStore + stash::service::adopt_component_from_path)
    //   instead of reimplementing component persistence.
    ("marketplace", "stash"),
    // upstream → gateway: TEST-ONLY. upstream/pool unit tests construct a
    //   GatewayManager + GatewayRuntimeHandle to exercise the pool against a
    //   live manager. Production upstream code does not depend on gateway; the
    //   reference lives entirely inside `#[cfg(test)] mod tests`.
    ("upstream", "gateway"),
];

/// Deprecated action-name aliases exempt from the `<resource>.<verb>` lint.
///
/// These are the legacy flat/bare names kept working for back-compat (Arch-M3);
/// each has a dotted canonical form added alongside it. Remove an entry here
/// only when the legacy name itself is removed from the catalog.
const DEPRECATED_ACTION_ALIASES: &[&str] = &[
    // deploy — bare verbs; canonical dotted forms: deploy.plan/run/rollback.
    "plan",
    "run",
    "rollback",
    // setup — flat snake_case; canonical dotted forms under setup.* added.
    "state",
    "bootstrap",
    "plugin_hook",
    "plugin_sync",
    "plugin_export",
    "plugin_connectivity",
    "check",
    "repair",
    "installed_plugins",
    "services_status",
    "install_plugin",
    "uninstall_plugin",
    "finalize",
];

// ---------------------------------------------------------------------------
// Arch-M1: inter-service coupling allowlist matrix
// ---------------------------------------------------------------------------

#[test]
fn cross_service_imports_match_allowlist() {
    let dispatch_root = crate_src_root().join("dispatch");
    let mut violations: Vec<String> = Vec::new();

    for service in KNOWN_SERVICES.iter().copied().chain(["upstream"]) {
        let edges = scan_service_edges(&dispatch_root, service);
        for target in edges {
            // A service may always import the shared substrate and itself.
            if target == service || SHARED_NON_SERVICES.contains(&target.as_str()) {
                continue;
            }
            let allowed = ALLOWED_EDGES
                .iter()
                .any(|(c, t)| *c == service && *t == target);
            if !allowed {
                violations.push(format!("{service} → {target}"));
            }
        }
    }
    violations.sort();
    violations.dedup();

    assert!(
        violations.is_empty(),
        "Architecture violation — unsanctioned cross-service dispatch edges \
         (see crates/lab/src/dispatch/CLAUDE.md and the ALLOWED_EDGES matrix \
         in this test):\n  {}\n\n\
         If the edge is legitimate, add `(\"{}\", ...)` to ALLOWED_EDGES with a \
         one-line rationale. If it is not, move the shared logic into \
         `crate::dispatch::helpers` or a shared subsystem.",
        violations.join("\n  "),
        violations
            .first()
            .and_then(|v| v.split(" → ").next())
            .unwrap_or("<consumer>"),
    );
}

/// The earlier single-edge guard, preserved as an explicit regression: no
/// non-orchestrator peer may import `crate::dispatch::setup`. (Subsumed by the
/// allowlist matrix above, which lists no `* → setup` edge, but kept as a
/// named test so the orchestrator clause stays self-documenting.)
#[test]
fn no_peer_service_imports_setup_dispatch() {
    let dispatch_root = crate_src_root().join("dispatch");
    let mut consumers: BTreeSet<String> = BTreeSet::new();
    for service in KNOWN_SERVICES.iter().copied().chain(["upstream"]) {
        if service == "setup" {
            continue;
        }
        if scan_service_edges(&dispatch_root, service).contains("setup") {
            consumers.insert(service.to_string());
        }
    }
    assert!(
        consumers.is_empty(),
        "Architecture violation — these services import `crate::dispatch::setup` \
         outside the orchestrator (see crates/lab/src/dispatch/CLAUDE.md \
         § Orchestrator Exception):\n  {}\n\n\
         Bootstrap dependency direction is one-way: setup → doctor; \
         peers MUST NOT depend on setup.",
        consumers.into_iter().collect::<Vec<_>>().join("\n  "),
    );
}

#[test]
fn services_list_is_current() {
    let dispatch_root = crate_src_root().join("dispatch");
    let mut on_disk: BTreeSet<String> = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(&dispatch_root) else {
        panic!("dispatch root not readable: {}", dispatch_root.display());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        // Only directories that have a sibling `<name>.rs` entrypoint are
        // dispatch services / subsystems.
        if dispatch_root.join(format!("{name}.rs")).exists() {
            on_disk.insert(name.to_string());
        }
    }

    let known: BTreeSet<String> = KNOWN_SERVICES
        .iter()
        .map(|s| s.to_string())
        .chain(SHARED_NON_SERVICES.iter().map(|s| s.to_string()))
        .collect();

    let unlisted: Vec<&String> = on_disk.difference(&known).collect();
    assert!(
        unlisted.is_empty(),
        "New dispatch service/subsystem(s) not classified in \
         architecture_orchestrator.rs: {unlisted:?}. Add each to KNOWN_SERVICES \
         (action-dispatched service) or SHARED_NON_SERVICES (shared subsystem), \
         and add any new cross-service edges to ALLOWED_EDGES."
    );
}

/// Collect the set of *other* `dispatch::<x>` module names referenced from a
/// given service's `.rs` files (the `<service>.rs` entrypoint plus everything
/// under `<service>/`).
fn scan_service_edges(dispatch_root: &Path, service: &str) -> BTreeSet<String> {
    let mut edges: BTreeSet<String> = BTreeSet::new();
    let mut visit = |contents: &str| {
        for target in extract_dispatch_targets(contents) {
            edges.insert(target);
        }
    };

    let entry = dispatch_root.join(format!("{service}.rs"));
    if let Ok(contents) = std::fs::read_to_string(&entry) {
        visit(&contents);
    }
    let dir = dispatch_root.join(service);
    walk_rs_files(&dir, &mut |_rel, contents| visit(contents));
    edges
}

/// Pull every `crate::dispatch::<ident>` head segment out of source text.
fn extract_dispatch_targets(contents: &str) -> Vec<String> {
    const NEEDLE: &str = "crate::dispatch::";
    let mut out = Vec::new();
    let mut rest = contents;
    while let Some(idx) = rest.find(NEEDLE) {
        let after = &rest[idx + NEEDLE.len()..];
        let ident: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let ident_len = ident.len();
        if !ident.is_empty() {
            out.push(ident);
        }
        rest = &after[ident_len..];
    }
    out
}

// ---------------------------------------------------------------------------
// Arch-M3: action-naming `<resource>.<verb>` lint
// ---------------------------------------------------------------------------

#[test]
fn catalog_action_names_are_dotted() {
    let dispatch_root = crate_src_root().join("dispatch");
    let mut violations: Vec<String> = Vec::new();

    walk_rs_files(&dispatch_root, &mut |rel, contents| {
        // Lint any file that declares `ActionSpec` literals — the canonical
        // `<service>/catalog.rs` files plus split catalogs like
        // `marketplace/{acp,mcp}_catalog.rs`. Test modules with synthetic
        // ActionSpec fixtures are excluded by skipping `#[cfg(test)]` regions
        // is unnecessary here because no catalog fixture uses bare names.
        if !contents.contains("ActionSpec {") {
            return;
        }
        for name in extract_action_names(contents) {
            if is_exempt_action(&name) {
                continue;
            }
            if !is_dotted_action(&name) {
                violations.push(format!("{} :: \"{name}\"", rel.display()));
            }
        }
    });
    violations.sort();
    violations.dedup();

    assert!(
        violations.is_empty(),
        "Action-naming violation — these catalog action names are not \
         `<resource>.<verb>` dotted form (`^[a-z0-9_]+(\\.[a-z0-9_]+)+$`):\n  {}\n\n\
         Add a dotted canonical name (keeping any legacy name as a deprecated \
         alias in DEPRECATED_ACTION_ALIASES). `help`/`schema` are exempt.",
        violations.join("\n  "),
    );
}

/// Extract action names from a catalog file: every `name: "<x>"` occurrence.
/// (`ParamSpec` also has a `name` field, but param names are not dotted action
/// strings; to avoid false positives we only accept `name:` lines whose value
/// already looks like an action — i.e. we exempt them via the alias/builtin
/// lists or they pass the dotted check. Param names like "action", "targets"
/// would otherwise trip the lint, so we additionally require the value to be a
/// declared ACTIONS entry by scanning only top-of-struct `name:` fields.)
fn extract_action_names(contents: &str) -> Vec<String> {
    // Only consider `name:` fields that are the FIRST field of an `ActionSpec`
    // literal. ActionSpec literals open with `ActionSpec {` then `name:`.
    const ANCHOR: &str = "ActionSpec {";
    let mut out = Vec::new();
    let mut rest = contents;
    while let Some(idx) = rest.find(ANCHOR) {
        let after = &rest[idx + ANCHOR.len()..];
        if let Some(name) = first_name_field(after) {
            out.push(name);
        }
        rest = after;
    }
    out
}

/// Given the text right after `ActionSpec {`, return the value of the first
/// `name: "..."` field.
fn first_name_field(after: &str) -> Option<String> {
    let name_idx = after.find("name:")?;
    let q1 = after[name_idx..].find('"')? + name_idx + 1;
    let q2 = after[q1..].find('"')? + q1;
    Some(after[q1..q2].to_string())
}

fn is_exempt_action(name: &str) -> bool {
    name == "help" || name == "schema" || DEPRECATED_ACTION_ALIASES.contains(&name)
}

/// `^[a-z0-9_]+(\.[a-z0-9_]+)+$` without a regex dependency.
fn is_dotted_action(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let segments: Vec<&str> = name.split('.').collect();
    if segments.len() < 2 {
        return false;
    }
    segments.iter().all(|seg| {
        !seg.is_empty()
            && seg
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    })
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

fn crate_src_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn walk_rs_files(root: &Path, visit: &mut dyn FnMut(&Path, &str)) {
    walk_rs_files_rel(root, root, visit);
}

fn walk_rs_files_rel(root: &Path, base: &Path, visit: &mut dyn FnMut(&Path, &str)) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs_files_rel(&path, base, visit);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            let Ok(rel) = path.strip_prefix(base) else {
                continue;
            };
            let Ok(contents) = std::fs::read_to_string(&path) else {
                continue;
            };
            visit(rel, &contents);
        }
    }
}
