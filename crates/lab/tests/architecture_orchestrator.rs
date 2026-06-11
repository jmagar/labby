//! Architecture guard for the lab-bg3e.3 orchestrator-exception clause.
//!
//! `setup` is a Bootstrap orchestrator: it is the ONLY service that may
//! invoke peer dispatch actions (`setup.draft.commit` calls
//! `doctor::dispatch("audit.full", _)`, see
//! `crates/lab/src/dispatch/CLAUDE.md` § "Orchestrator Exception").
//! Dependency direction must stay one-way:
//!
//! ```text
//!   setup → doctor               (allowed)
//!   doctor → setup               (FORBIDDEN)
//!   any non-orchestrator → setup (FORBIDDEN)
//! ```
//!
//! This test fails when a regression imports `crate::dispatch::setup`
//! from a peer dispatch service. Chosen over a clippy `disallowed-methods`
//! rule because:
//! 1. Tests run automatically in CI.
//! 2. The error message can name the architecture clause directly.
//! 3. No external clippy.toml configuration to keep in sync.

use std::path::{Path, PathBuf};

const FORBIDDEN_IMPORT: &str = "crate::dispatch::setup";

/// Files / directories permitted to depend on `crate::dispatch::setup`.
/// Everything else is a violation. Paths are checked relative to the
/// crate root (`crates/lab/src/`).
const ALLOWED_PATHS: &[&str] = &[
    // The orchestrator's own modules:
    "dispatch/setup.rs",
    "dispatch/setup/",
    // Surfaces that mount the dispatch (CLI / API / registry):
    "cli/setup.rs",
    "cli/serve.rs",
    "api/services/setup.rs",
    "registry.rs",
];

#[test]
fn no_peer_service_imports_setup_dispatch() {
    let crate_root = crate_src_root();
    let mut violations: Vec<PathBuf> = Vec::new();
    walk_rs_files(&crate_root, &crate_root, &mut |rel_path, contents| {
        if contents.contains(FORBIDDEN_IMPORT) && !is_allowed(rel_path) {
            violations.push(rel_path.to_path_buf());
        }
    });
    assert!(
        violations.is_empty(),
        "Architecture violation — these files import `{FORBIDDEN_IMPORT}` \
         outside the orchestrator (see crates/lab/src/dispatch/CLAUDE.md \
         § Orchestrator Exception):\n  {}\n\n\
         Bootstrap dependency direction is one-way: setup → doctor; \
         peers MUST NOT depend on setup. If you need shared logic, extract \
         it into `crate::dispatch::helpers` or a new shared module.",
        violations
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

fn crate_src_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn is_allowed(rel_path: &Path) -> bool {
    let rel = rel_path.to_string_lossy().replace('\\', "/");
    ALLOWED_PATHS
        .iter()
        .any(|allowed| rel == *allowed || rel.starts_with(allowed))
}

fn walk_rs_files(root: &Path, base: &Path, visit: &mut dyn FnMut(&Path, &str)) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs_files(&path, base, visit);
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
