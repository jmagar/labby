//! Build script: embed the gateway-admin web bundle into the binary.
//!
//! The frontend (`apps/gateway-admin`) is exported by Next.js into
//! `apps/gateway-admin/out/`. We generate `$OUT_DIR/embedded_web_assets.rs`
//! containing one `include_bytes!` per file so the assets ship inside the
//! `labby` binary.
//!
//! Why a build script instead of the `include_dir!` proc macro:
//!
//! - `include_dir!` walks the asset directory **at macro-expansion time**. Under
//!   distributed/remote compilation (sccache-dist) expansion happens on a remote
//!   that does not have `apps/gateway-admin/out`, so the build fails there.
//! - `include_dir!` also **panics** when the directory is absent (e.g. CI jobs
//!   that build the backend without first building the frontend), turning a
//!   benign "frontend not built" state into a hard compile error.
//!
//! This build script runs locally (never on the dist remote), reads the
//! directory itself, and emits an **empty** asset set when the directory is
//! missing — a valid state for backend-only work. A genuine filesystem error
//! (permissions, mid-walk read failure) fails the build via `Err` rather than
//! being masked as "missing" — but we never `panic!` (clippy bans it). The
//! generated `EMBEDDED_WEB_FILES` slice is consumed by `crate::api::web`.

use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")?;
    let out_dir = env::var("OUT_DIR")?;
    let assets_dir = Path::new(&manifest_dir).join("../../apps/gateway-admin/out");
    let dest = Path::new(&out_dir).join("embedded_web_assets.rs");

    // Re-run when the bundle appears/disappears or its top level changes.
    println!("cargo:rerun-if-changed={}", assets_dir.display());

    let mut files: Vec<(String, PathBuf)> = Vec::new();
    match fs::canonicalize(&assets_dir) {
        Ok(base) if base.is_dir() => collect_files(&base, &base, &mut files)?,
        Ok(base) => {
            return Err(format!(
                "web assets path {} exists but is not a directory",
                base.display()
            )
            .into());
        }
        // A missing bundle is a valid backend-only state: embed nothing.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "cargo:warning=apps/gateway-admin/out not found — embedding empty web assets \
                 (run `pnpm --filter gateway-admin build` to populate the bundle)"
            );
        }
        // Any other error (permissions, I/O) is a real failure — fail the build
        // rather than silently shipping a binary with no UI.
        Err(error) => {
            return Err(format!(
                "failed to access web assets at {}: {error}",
                assets_dir.display()
            )
            .into());
        }
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut code = String::with_capacity(files.len() * 96 + 64);
    code.push_str("pub static EMBEDDED_WEB_FILES: &[(&str, &[u8])] = &[\n");
    for (rel, abs) in &files {
        code.push_str(&format!(
            "    ({rel:?}, include_bytes!({abs:?})),\n",
            rel = rel,
            abs = abs.to_string_lossy(),
        ));
    }
    code.push_str("];\n");

    fs::write(&dest, code)?;
    Ok(())
}

/// Recursively collect every file under `dir`, keying it by its forward-slash
/// path relative to `base`. A read error (the directory or a single entry)
/// propagates as `Err` so an incomplete bundle fails the build instead of
/// silently embedding a partial asset set.
fn collect_files(
    base: &Path,
    dir: &Path,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), Box<dyn Error>> {
    let entries = fs::read_dir(dir)
        .map_err(|error| format!("failed to read web asset dir {}: {error}", dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "failed to read a web asset entry in {}: {error}",
                dir.display()
            )
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(base, &path, out)?;
        } else if path.is_file() {
            // Content changes must retrigger the build.
            println!("cargo:rerun-if-changed={}", path.display());
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}
