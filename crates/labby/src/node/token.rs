use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::fs;
use tokio::io::AsyncWriteExt as _;
use uuid::Uuid;

pub async fn load_or_create(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    if let Ok(raw) = fs::read_to_string(path).await {
        let token = raw.trim();
        if !token.is_empty() {
            return Ok(token.to_string());
        }
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }

    let token = Uuid::new_v4().to_string();
    let tmp_path = temp_path(path);
    let mut open_options = fs::OpenOptions::new();
    open_options.create_new(true).write(true);
    #[cfg(unix)]
    {
        open_options.mode(0o600);
    }
    let mut file = open_options
        .open(&tmp_path)
        .await
        .with_context(|| format!("create {}", tmp_path.display()))?;
    file.write_all(token.as_bytes())
        .await
        .with_context(|| format!("write {}", tmp_path.display()))?;
    file.sync_all()
        .await
        .with_context(|| format!("sync {}", tmp_path.display()))?;
    drop(file);
    fs::rename(&tmp_path, path)
        .await
        .with_context(|| format!("rename {} -> {}", tmp_path.display(), path.display()))?;

    Ok(token)
}

fn temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("node-token");
    path.with_file_name(format!("{file_name}.{}.tmp", Uuid::new_v4()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn load_or_create_round_trips_same_token() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("node-token");
        let first = load_or_create(&path).await.expect("create token");
        let second = load_or_create(&path).await.expect("load token");
        assert_eq!(first, second);
    }
}
