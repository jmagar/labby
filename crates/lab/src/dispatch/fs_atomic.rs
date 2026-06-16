//! Durable atomic file writes shared across the dispatch layer.
//!
//! The pattern is: write to a `NamedTempFile` in the destination's parent
//! directory, `fsync` it, then `persist` (rename) over the target. The fsync
//! before rename guarantees the file is never published with unwritten data
//! (which could otherwise surface as a null-byte / truncated read after a crash).
//!
//! This is the single owner of that policy for the `lab` crate — do NOT
//! reintroduce per-module copies. (`lab-apis` keeps its own copy since it cannot
//! depend on this crate.)

use std::io::Write;
use std::path::Path;

use serde::Serialize;
use tempfile::NamedTempFile;

use crate::dispatch::error::ToolError;

fn io_err(error: impl std::fmt::Display) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: error.to_string(),
    }
}

/// Atomically write `bytes` to `path`, creating parent directories as needed.
pub fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(io_err)?;
    let mut temp = NamedTempFile::new_in(parent).map_err(io_err)?;
    temp.write_all(bytes).map_err(io_err)?;
    // fsync before the rename so the file is never published with unwritten data.
    temp.as_file().sync_all().map_err(io_err)?;
    temp.persist(path).map_err(|err| io_err(err.error))?;
    // fsync the parent directory so the rename itself is durable — without this
    // a crash right after `persist` can lose the directory entry even though the
    // file data was synced.
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(io_err)?;
    Ok(())
}

/// Atomically write `value` as pretty-printed JSON to `path`.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), ToolError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(io_err)?;
    write_bytes_atomic(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn write_bytes_atomic_creates_parents_and_persists() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("a/b/c.txt");
        write_bytes_atomic(&target, b"hello").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    }

    #[test]
    fn write_json_atomic_roundtrips() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("v.json");
        write_json_atomic(&target, &json!({"k": 1})).unwrap();
        let read: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(read, json!({"k": 1}));
    }

    #[test]
    fn write_bytes_atomic_overwrites_existing() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("x.txt");
        write_bytes_atomic(&target, b"first").unwrap();
        write_bytes_atomic(&target, b"second").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "second");
    }
}
