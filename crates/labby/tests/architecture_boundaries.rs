#![allow(
    clippy::bool_assert_comparison,
    clippy::err_expect,
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::len_zero,
    clippy::manual_string_new,
    clippy::needless_raw_string_hashes,
    clippy::single_char_pattern,
    clippy::unnested_or_patterns
)]
use std::fs;
use std::path::{Path, PathBuf};

fn lab_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// Dispatch files allowed to reference `crate::mcp`, by `src`-relative path.
///
/// The upstream proxy pool (formerly `dispatch/upstream/**`) moved to the
/// standalone `lab-gateway` crate, so the previously-allowlisted
/// `prompts_list.rs` / `resources_read.rs` fixtures no longer live under
/// `crate::dispatch` here — the list is empty. In-process peer construction
/// lives exclusively in `crate::mcp::in_process_peer` and is injected into the
/// pool via the `InProcessConnector` IoC seam, so the boundary stays clean.
const ALLOWED_MCP_IMPORTS: &[&str] = &[];

#[test]
fn dispatch_layer_does_not_import_mcp_surface_modules() {
    let dispatch = lab_src().join("dispatch");
    let mut files = Vec::new();
    rust_files(&dispatch, &mut files);

    let offenders: Vec<_> = files
        .into_iter()
        .filter_map(|path| {
            let rel = path
                .strip_prefix(lab_src())
                .unwrap()
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if ALLOWED_MCP_IMPORTS.contains(&rel.as_str()) {
                return None;
            }
            let content = fs::read_to_string(&path).expect("read source");
            let has_import = content
                .lines()
                .map(str::trim)
                .any(|line| line.starts_with("use crate::mcp") || line.contains(" crate::mcp::"));
            has_import.then_some(rel)
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "dispatch must not import the MCP surface; offenders: {offenders:?}"
    );
}

fn lab_gateway_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("lab-gateway")
        .join("src")
}

/// The standalone gateway runtime crate must stay free of product-surface
/// adapter dependencies (axum/clap/utoipa) and must not depend on Labby itself.
/// A manifest gate is cheaper and more robust than scanning sources for use
/// statements.
#[test]
fn lab_gateway_manifest_does_not_depend_on_product_surfaces() {
    let manifest = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("lab-gateway")
            .join("Cargo.toml"),
    )
    .expect("read lab-gateway manifest");
    for banned in ["axum", "clap", "utoipa", "javy", "wasmtime", "labby"] {
        assert!(
            !manifest.contains(banned),
            "lab-gateway runtime crate must not depend on {banned}"
        );
    }
}

/// `lab-gateway` receives its registry/service composition by injection through
/// the `InProcessServiceRegistry` trait; it must never reach for Labby's default
/// registry builder.
#[test]
fn lab_gateway_does_not_call_labby_default_registry() {
    let mut files = Vec::new();
    rust_files(&lab_gateway_src(), &mut files);
    let offenders: Vec<_> = files
        .into_iter()
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read lab-gateway source")
                .contains("build_default_registry")
        })
        .map(|path| path.display().to_string())
        .collect();
    assert!(
        offenders.is_empty(),
        "lab-gateway must not call build_default_registry: {offenders:?}"
    );
}
