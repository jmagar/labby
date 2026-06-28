use std::path::PathBuf;

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::ToolError;

use super::path::VirtualPath;
use super::quota::StateWorkspaceLimits;

#[derive(Debug, Clone)]
pub(crate) struct StateWorkspace {
    root: PathBuf,
    limits: StateWorkspaceLimits,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ReadFileResult {
    pub(crate) path: String,
    pub(crate) content: String,
    pub(crate) bytes: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ListResult {
    pub(crate) entries: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GlobResult {
    pub(crate) matches: Vec<String>,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SearchMatch {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SearchFilesResult {
    pub(crate) matches: Vec<SearchMatch>,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReplaceInFilesResult {
    pub(crate) changed: Vec<String>,
    pub(crate) dry_run: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct FileEdit {
    pub(crate) path: String,
    pub(crate) search: String,
    pub(crate) replace: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct EditPlanResult {
    pub(crate) plan_id: String,
    pub(crate) edits: Vec<FileEdit>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ApplyEditPlanResult {
    pub(crate) ok: bool,
    pub(crate) changed: Vec<String>,
}

pub(crate) fn default_search_limit() -> usize {
    200
}

pub(crate) fn default_true() -> bool {
    true
}

impl StateWorkspace {
    pub(crate) fn new(root: PathBuf, limits: StateWorkspaceLimits) -> Result<Self, ToolError> {
        std::fs::create_dir_all(&root).map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to create code mode workspace root: {err}"),
        })?;
        Ok(Self { root, limits })
    }

    fn resolve(&self, path: &VirtualPath) -> PathBuf {
        self.root.join(path.as_str())
    }

    pub(crate) async fn write_file(
        &self,
        path: &VirtualPath,
        content: &str,
    ) -> Result<(), ToolError> {
        if content.len() > self.limits.max_file_bytes {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "state file content is {} bytes; maximum is {}",
                    content.len(),
                    self.limits.max_file_bytes
                ),
                param: "content".to_string(),
            });
        }
        self.check_total_bytes_after_write(path, content.len() as u64)
            .await?;

        let destination = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(internal_io("create state directory"))?;
        }
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;

        let tmp = destination.with_extension("tmp-labby-state");
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)
            .await
            .map_err(internal_io("create state temp file"))?;
        file.write_all(content.as_bytes())
            .await
            .map_err(internal_io("write state temp file"))?;
        file.flush()
            .await
            .map_err(internal_io("flush state temp file"))?;
        drop(file);
        tokio::fs::rename(&tmp, &destination)
            .await
            .map_err(internal_io("move state temp file"))?;
        Ok(())
    }

    async fn check_total_bytes_after_write(
        &self,
        path: &VirtualPath,
        next_file_bytes: u64,
    ) -> Result<(), ToolError> {
        let destination = self.resolve(path);
        let current_file_bytes = match tokio::fs::metadata(&destination).await {
            Ok(metadata) if metadata.is_file() => metadata.len(),
            Ok(_) | Err(_) => 0,
        };
        let total = workspace_total_bytes(&self.root).await?;
        let projected = total
            .saturating_sub(current_file_bytes)
            .saturating_add(next_file_bytes);
        if projected > self.limits.max_total_bytes {
            return Err(ToolError::Sdk {
                sdk_kind: "quota_exceeded".to_string(),
                message: format!(
                    "state workspace would be {projected} bytes; maximum is {}",
                    self.limits.max_total_bytes
                ),
            });
        }
        Ok(())
    }

    pub(crate) async fn read_file(&self, path: &VirtualPath) -> Result<ReadFileResult, ToolError> {
        let destination = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        let file = tokio::fs::File::open(&destination)
            .await
            .map_err(not_found_or_internal("open state file"))?;
        let mut content = String::new();
        file.take(self.limits.max_result_bytes as u64 + 1)
            .read_to_string(&mut content)
            .await
            .map_err(internal_io("read state file"))?;
        if content.len() > self.limits.max_result_bytes {
            return Err(ToolError::Sdk {
                sdk_kind: "response_too_large".to_string(),
                message: "state read result exceeded max result bytes".to_string(),
            });
        }
        Ok(ReadFileResult {
            path: path.as_str().to_string(),
            bytes: content.len(),
            content,
        })
    }

    pub(crate) async fn list(&self, path: &VirtualPath) -> Result<ListResult, ToolError> {
        let dir = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &dir)?;
        let mut read_dir = tokio::fs::read_dir(&dir)
            .await
            .map_err(not_found_or_internal("read state directory"))?;
        let mut entries = Vec::new();
        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(internal_io("read state directory entry"))?
        {
            entries.push(entry.file_name().to_string_lossy().to_string());
            if entries.len() as u64 > self.limits.max_entries {
                return Err(ToolError::Sdk {
                    sdk_kind: "response_too_large".to_string(),
                    message: "state list exceeded max entries".to_string(),
                });
            }
        }
        entries.sort();
        Ok(ListResult { entries })
    }

    pub(crate) async fn glob(&self, pattern: &str, limit: usize) -> Result<GlobResult, ToolError> {
        let limit = normalize_limit(limit);
        let matcher = glob_pattern_regex(pattern)?;
        let mut files = self.walk_files(limit.saturating_add(1)).await?;
        files.sort();
        let mut matches = Vec::new();
        for file in files {
            if matcher.is_match(&file) {
                matches.push(file);
                if matches.len() >= limit {
                    return Ok(GlobResult {
                        matches,
                        truncated: true,
                    });
                }
            }
        }
        Ok(GlobResult {
            matches,
            truncated: false,
        })
    }

    pub(crate) async fn search_files(
        &self,
        pattern: &str,
        query: &str,
        limit: usize,
    ) -> Result<SearchFilesResult, ToolError> {
        if query.is_empty() {
            return Err(ToolError::InvalidParam {
                message: "state search query must not be empty".to_string(),
                param: "query".to_string(),
            });
        }
        let limit = normalize_limit(limit);
        let glob = self.glob(pattern, self.limits.max_entries as usize).await?;
        let mut matches = Vec::new();
        for path in glob.matches {
            let virtual_path = VirtualPath::parse(&path)?;
            let file = self.read_file(&virtual_path).await?;
            for (index, line) in file.content.lines().enumerate() {
                if line.contains(query) {
                    matches.push(SearchMatch {
                        path: path.clone(),
                        line: index + 1,
                        text: cap_line_preview(line),
                    });
                    if matches.len() >= limit {
                        return Ok(SearchFilesResult {
                            matches,
                            truncated: true,
                        });
                    }
                    ensure_serialized_result_fits(&matches, self.limits.max_result_bytes)?;
                }
            }
        }
        Ok(SearchFilesResult {
            matches,
            truncated: glob.truncated,
        })
    }

    pub(crate) async fn replace_in_files(
        &self,
        pattern: &str,
        search: &str,
        replace: &str,
        dry_run: bool,
    ) -> Result<ReplaceInFilesResult, ToolError> {
        if search.is_empty() {
            return Err(ToolError::InvalidParam {
                message: "state replace search must not be empty".to_string(),
                param: "search".to_string(),
            });
        }
        let glob = self.glob(pattern, self.limits.max_entries as usize).await?;
        let mut changed = Vec::new();
        for path in glob.matches {
            let virtual_path = VirtualPath::parse(&path)?;
            let file = self.read_file(&virtual_path).await?;
            if !file.content.contains(search) {
                continue;
            }
            changed.push(path.clone());
            if !dry_run {
                let next = file.content.replace(search, replace);
                self.write_file(&virtual_path, &next).await?;
            }
        }
        Ok(ReplaceInFilesResult { changed, dry_run })
    }

    pub(crate) async fn plan_edits(
        &self,
        edits: Vec<FileEdit>,
    ) -> Result<EditPlanResult, ToolError> {
        let edits = normalize_edits(edits)?;
        let canonical = serde_json::to_vec(&edits).map_err(serialize_error)?;
        let plan_id = hex::encode(Sha256::digest(&canonical));
        let plan_path = self.plan_path(&plan_id);
        if let Some(parent) = plan_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(internal_io("create state edit plan directory"))?;
        }
        tokio::fs::write(&plan_path, canonical)
            .await
            .map_err(internal_io("write state edit plan"))?;
        Ok(EditPlanResult { plan_id, edits })
    }

    pub(crate) async fn apply_edit_plan(
        &self,
        plan_id: &str,
    ) -> Result<ApplyEditPlanResult, ToolError> {
        validate_plan_id(plan_id)?;
        let plan_path = self.plan_path(plan_id);
        let plan = tokio::fs::read(&plan_path)
            .await
            .map_err(not_found_or_internal("read state edit plan"))?;
        let edits: Vec<FileEdit> = serde_json::from_slice(&plan).map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to parse state edit plan: {err}"),
        })?;

        let mut changed = Vec::new();
        let rollback_root = self
            .root
            .join(".labby-state")
            .join("rollback")
            .join(plan_id);
        for edit in edits {
            let path = VirtualPath::parse(&edit.path)?;
            let original = self.read_file(&path).await?;
            if !original.content.contains(&edit.search) {
                continue;
            }
            let rollback_path = rollback_root.join(path.as_str());
            if let Some(parent) = rollback_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(internal_io("create state rollback directory"))?;
            }
            tokio::fs::write(&rollback_path, original.content.as_bytes())
                .await
                .map_err(internal_io("write state rollback file"))?;
            let next = original.content.replace(&edit.search, &edit.replace);
            if let Err(err) = self.write_file(&path, &next).await {
                self.restore_rollbacks(&rollback_root, &changed).await?;
                return Err(err);
            }
            changed.push(path.as_str().to_string());
        }

        Ok(ApplyEditPlanResult { ok: true, changed })
    }

    async fn restore_rollbacks(
        &self,
        rollback_root: &PathBuf,
        changed: &[String],
    ) -> Result<(), ToolError> {
        for path in changed.iter().rev() {
            let rollback_path = rollback_root.join(path);
            let content = tokio::fs::read_to_string(&rollback_path)
                .await
                .map_err(internal_io("read state rollback file"))?;
            self.write_file(&VirtualPath::parse(path)?, &content)
                .await?;
        }
        Ok(())
    }

    fn plan_path(&self, plan_id: &str) -> PathBuf {
        self.root
            .join(".labby-state")
            .join("plans")
            .join(format!("{plan_id}.json"))
    }

    async fn walk_files(&self, limit: usize) -> Result<Vec<String>, ToolError> {
        let mut files = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let mut read_dir = match tokio::fs::read_dir(&dir).await {
                Ok(read_dir) => read_dir,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(internal_io("walk state workspace")(error)),
            };
            while let Some(entry) = read_dir
                .next_entry()
                .await
                .map_err(internal_io("walk state workspace entry"))?
            {
                let path = entry.path();
                let relative = match path.strip_prefix(&self.root) {
                    Ok(relative) => relative,
                    Err(_) => continue,
                };
                let virtual_path = labby_runtime::path_safety::rel_to_unix_string(relative);
                if virtual_path.starts_with(".labby-state/") {
                    continue;
                }
                let metadata = entry
                    .metadata()
                    .await
                    .map_err(internal_io("read state workspace metadata"))?;
                if metadata.is_dir() {
                    stack.push(path);
                } else if metadata.is_file() {
                    files.push(virtual_path);
                    if files.len() > limit {
                        return Ok(files);
                    }
                }
            }
        }
        Ok(files)
    }
}

fn internal_io(action: &'static str) -> impl FnOnce(std::io::Error) -> ToolError {
    move |err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to {action}: {err}"),
    }
}

fn not_found_or_internal(action: &'static str) -> impl FnOnce(std::io::Error) -> ToolError {
    move |err| ToolError::Sdk {
        sdk_kind: if err.kind() == std::io::ErrorKind::NotFound {
            "not_found"
        } else {
            "internal_error"
        }
        .to_string(),
        message: format!("failed to {action}: {err}"),
    }
}

async fn workspace_total_bytes(root: &PathBuf) -> Result<u64, ToolError> {
    let mut total = 0_u64;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let mut read_dir = match tokio::fs::read_dir(&dir).await {
            Ok(read_dir) => read_dir,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(internal_io("scan state workspace")(error)),
        };
        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(internal_io("scan state workspace entry"))?
        {
            let metadata = entry
                .metadata()
                .await
                .map_err(internal_io("read state workspace metadata"))?;
            if metadata.is_dir() {
                stack.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

fn normalize_limit(limit: usize) -> usize {
    limit.clamp(1, 10_000)
}

fn glob_pattern_regex(pattern: &str) -> Result<Regex, ToolError> {
    if pattern.trim().is_empty() {
        return Err(ToolError::InvalidParam {
            message: "state glob pattern must not be empty".to_string(),
            param: "pattern".to_string(),
        });
    }
    if pattern.contains("..") || pattern.starts_with('/') || pattern.contains(':') {
        return Err(ToolError::Sdk {
            sdk_kind: "path_traversal".to_string(),
            message: "state glob pattern must stay inside the workspace".to_string(),
        });
    }
    let mut regex = String::from("^");
    let chars = pattern.chars().collect::<Vec<_>>();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') && chars.get(i + 2) == Some(&'/') => {
                regex.push_str("(?:.*/)?");
                i += 3;
            }
            '*' if chars.get(i + 1) == Some(&'*') => {
                regex.push_str(".*");
                i += 2;
            }
            '*' => {
                regex.push_str("[^/]*");
                i += 1;
            }
            '?' => {
                regex.push_str("[^/]");
                i += 1;
            }
            ch => {
                regex.push_str(&regex::escape(&ch.to_string()));
                i += 1;
            }
        }
    }
    regex.push('$');
    Regex::new(&regex).map_err(|err| ToolError::InvalidParam {
        message: format!("invalid state glob pattern: {err}"),
        param: "pattern".to_string(),
    })
}

fn cap_line_preview(line: &str) -> String {
    line.chars().take(512).collect()
}

fn ensure_serialized_result_fits<T: Serialize>(value: &T, max: usize) -> Result<(), ToolError> {
    let len = serde_json::to_vec(value).map_err(serialize_error)?.len();
    if len > max {
        return Err(ToolError::Sdk {
            sdk_kind: "response_too_large".to_string(),
            message: "state search result exceeded max result bytes".to_string(),
        });
    }
    Ok(())
}

fn normalize_edits(edits: Vec<FileEdit>) -> Result<Vec<FileEdit>, ToolError> {
    if edits.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "state edit plan must include at least one edit".to_string(),
            param: "edits".to_string(),
        });
    }
    edits
        .into_iter()
        .map(|edit| {
            if edit.search.is_empty() {
                return Err(ToolError::InvalidParam {
                    message: "state edit search must not be empty".to_string(),
                    param: "search".to_string(),
                });
            }
            let path = VirtualPath::parse(&edit.path)?.as_str().to_string();
            Ok(FileEdit {
                path,
                search: edit.search,
                replace: edit.replace,
            })
        })
        .collect()
}

fn validate_plan_id(plan_id: &str) -> Result<(), ToolError> {
    let valid = plan_id.len() == 64 && plan_id.chars().all(|ch| ch.is_ascii_hexdigit());
    if !valid {
        return Err(ToolError::InvalidParam {
            message: "state edit plan id must be a sha256 hex string".to_string(),
            param: "planId".to_string(),
        });
    }
    Ok(())
}

fn serialize_error(err: serde_json::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to serialize state value: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::quota::StateWorkspaceLimits;

    #[tokio::test]
    async fn workspace_writes_reads_and_reopens() {
        let temp = tempfile::tempdir().unwrap();
        let ws = StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
            .unwrap();
        ws.write_file(
            &VirtualPath::parse("/src/app.rs").unwrap(),
            "fn main() {}\n",
        )
        .await
        .unwrap();
        assert_eq!(
            ws.read_file(&VirtualPath::parse("src/app.rs").unwrap())
                .await
                .unwrap()
                .content,
            "fn main() {}\n"
        );
        let ws2 = StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
            .unwrap();
        assert_eq!(
            ws2.list(&VirtualPath::parse("src").unwrap())
                .await
                .unwrap()
                .entries
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn workspace_rejects_large_writes_and_reads() {
        let temp = tempfile::tempdir().unwrap();
        let limits = StateWorkspaceLimits {
            max_file_bytes: 4,
            max_result_bytes: 4,
            ..StateWorkspaceLimits::default()
        };
        let ws = StateWorkspace::new(temp.path().to_path_buf(), limits).unwrap();
        let err = ws
            .write_file(&VirtualPath::parse("too-big.txt").unwrap(), "12345")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "invalid_param");

        std::fs::write(temp.path().join("existing.txt"), "12345").unwrap();
        let err = ws
            .read_file(&VirtualPath::parse("existing.txt").unwrap())
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "response_too_large");
    }

    #[tokio::test]
    async fn workspace_enforces_total_byte_limit() {
        let temp = tempfile::tempdir().unwrap();
        let limits = StateWorkspaceLimits {
            max_file_bytes: 10,
            max_total_bytes: 6,
            ..StateWorkspaceLimits::default()
        };
        let ws = StateWorkspace::new(temp.path().to_path_buf(), limits).unwrap();
        ws.write_file(&VirtualPath::parse("a.txt").unwrap(), "1234")
            .await
            .unwrap();
        let err = ws
            .write_file(&VirtualPath::parse("b.txt").unwrap(), "1234")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "quota_exceeded");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn workspace_rejects_symlink_ancestors() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let ws = StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), temp.path().join("link")).unwrap();
        let err = ws
            .write_file(&VirtualPath::parse("link/file.txt").unwrap(), "x")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "symlink_rejected");
    }

    #[tokio::test]
    async fn workspace_glob_search_replace_and_edit_plan() {
        let temp = tempfile::tempdir().unwrap();
        let ws = StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
            .unwrap();
        ws.write_file(
            &VirtualPath::parse("src/app.rs").unwrap(),
            "fn main() { println!(\"hi\"); }\n",
        )
        .await
        .unwrap();

        let glob = ws.glob("src/**/*.rs", 10).await.unwrap();
        assert_eq!(glob.matches, vec!["src/app.rs"]);

        let matches = ws.search_files("src/**/*.rs", "println", 10).await.unwrap();
        assert_eq!(matches.matches.len(), 1);
        assert_eq!(matches.matches[0].line, 1);

        let dry = ws
            .replace_in_files("src/**/*.rs", "println", "eprintln", true)
            .await
            .unwrap();
        assert_eq!(dry.changed, vec!["src/app.rs"]);
        assert!(dry.dry_run);

        let plan = ws
            .plan_edits(vec![FileEdit {
                path: "src/app.rs".to_string(),
                search: "println".to_string(),
                replace: "eprintln".to_string(),
            }])
            .await
            .unwrap();
        let applied = ws.apply_edit_plan(&plan.plan_id).await.unwrap();
        assert_eq!(applied.changed, vec!["src/app.rs"]);
        let updated = ws
            .read_file(&VirtualPath::parse("src/app.rs").unwrap())
            .await
            .unwrap();
        assert!(updated.content.contains("eprintln"));
    }
}
