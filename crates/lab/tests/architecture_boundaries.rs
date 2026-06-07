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

/// Dispatch files allowed to reference `crate::mcp`, by `src`-relative path:
/// - `connect_stdio.rs` constructs an in-process `LabMcpServer` peer; inverting
///   that dependency is a larger refactor tracked separately and deferred here.
/// - `prompts_list.rs` / `resources_read.rs` reference the MCP surface only from
///   `#[cfg(test)]` fixtures, not the shipped dispatch path.
const ALLOWED_MCP_IMPORTS: &[&str] = &[
    "dispatch/upstream/pool/connect_stdio.rs",
    "dispatch/upstream/pool/prompts_list.rs",
    "dispatch/upstream/pool/resources_read.rs",
];

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
