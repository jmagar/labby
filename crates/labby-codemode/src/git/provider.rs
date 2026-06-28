use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::error::ToolError;
use crate::state::workspace::StateWorkspace;

use super::command::{GitCommandSpec, git_base_args, validate_remote_url};

const MAX_GIT_STDOUT_BYTES: usize = 64 * 1024;
const MAX_GIT_STDERR_BYTES: usize = 16 * 1024;

pub(crate) async fn dispatch_git_method(
    workspace: &StateWorkspace,
    method: &str,
    params: Value,
) -> Result<Value, ToolError> {
    let spec = GitCommandSpec::for_method(method, params)?;
    let workdir = git_workdir(workspace, spec.cwd.as_ref()).await?;
    if let Some(remote) = &spec.remote_preflight {
        ensure_remote_urls_allowed(&workdir, remote, RemoteUrlMode::Fetch).await?;
    }
    if let Some(remote) = &spec.push_remote_preflight {
        ensure_remote_urls_allowed(&workdir, remote, RemoteUrlMode::Push).await?;
    }
    if let Some(branch) = &spec.branch_preflight {
        ensure_branch_ref_allowed(&workdir, branch).await?;
    }
    if let Some(destination) = &spec.clone_destination {
        ensure_clone_destination_allowed(&workdir, destination).await?;
    }
    let mut stdout = run_git(&workdir, &spec.args).await?;
    if matches!(method, "status" | "diff") {
        stdout = scrub_git_reserved_metadata(&stdout);
    }
    if git_method_mutates_workspace(method) {
        if let Err(err) = workspace.enforce_total_bytes().await {
            if let Some(destination) = &spec.clone_destination {
                cleanup_clone_destination(&workdir, destination, err).await?;
            } else {
                return Err(err);
            }
        }
    }
    if method == "remoteList" {
        return Ok(json!({ "ok": true, "stdout": stdout, "remotes": parse_remote_list(&stdout) }));
    }
    Ok(json!({ "ok": true, "stdout": stdout }))
}

fn git_method_mutates_workspace(method: &str) -> bool {
    !matches!(method, "status" | "log" | "diff" | "remoteList")
}

async fn git_workdir(
    workspace: &StateWorkspace,
    cwd: Option<&crate::state::path::VirtualPath>,
) -> Result<PathBuf, ToolError> {
    let Some(cwd) = cwd else {
        return Ok(workspace.root_path().clone());
    };
    let workdir = workspace.root_path().join(cwd.as_str());
    labby_runtime::path_safety::reject_existing_symlink_ancestors(workspace.root_path(), &workdir)?;
    match tokio::fs::symlink_metadata(&workdir).await {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ToolError::Sdk {
            sdk_kind: "permission_denied".to_string(),
            message: "git cwd is denied because it is a symlink".to_string(),
        }),
        Ok(metadata) if metadata.is_dir() => Ok(workdir),
        Ok(_) => Err(ToolError::InvalidParam {
            message: "git cwd must be a directory".to_string(),
            param: "cwd".to_string(),
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(ToolError::InvalidParam {
            message: "git cwd does not exist".to_string(),
            param: "cwd".to_string(),
        }),
        Err(err) => Err(ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to inspect git cwd: {err}"),
        }),
    }
}

async fn ensure_branch_ref_allowed(workspace_root: &Path, branch: &str) -> Result<(), ToolError> {
    let mut args = git_base_args(["check-ref-format", "--branch"]);
    args.push(branch.to_string());
    run_git(workspace_root, &args)
        .await
        .map(|_| ())
        .map_err(|_| ToolError::InvalidParam {
            message: "git ref is not allowed".to_string(),
            param: "ref".to_string(),
        })
}

async fn ensure_clone_destination_allowed(
    workspace_root: &Path,
    destination: &crate::state::path::VirtualPath,
) -> Result<(), ToolError> {
    let path = workspace_root.join(destination.as_str());
    labby_runtime::path_safety::reject_existing_symlink_ancestors(workspace_root, &path)?;
    match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ToolError::Sdk {
            sdk_kind: "permission_denied".to_string(),
            message: "git clone destination is denied because it is a symlink".to_string(),
        }),
        Ok(metadata) if metadata.is_dir() => Err(ToolError::InvalidParam {
            message: "git clone destination must be absent".to_string(),
            param: "directory".to_string(),
        }),
        Ok(_) => Err(ToolError::InvalidParam {
            message: "git clone destination must be absent".to_string(),
            param: "directory".to_string(),
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to inspect git clone destination: {err}"),
        }),
    }
}

async fn cleanup_clone_destination(
    workspace_root: &Path,
    destination: &crate::state::path::VirtualPath,
    original: ToolError,
) -> Result<(), ToolError> {
    let path = workspace_root.join(destination.as_str());
    let cleanup = match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            tokio::fs::remove_dir_all(&path).await
        }
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            tokio::fs::remove_file(&path).await
        }
        Ok(_) | Err(_) => return Err(original),
    };
    cleanup.map_err(|err| ToolError::Sdk {
        sdk_kind: "quota_cleanup_failed".to_string(),
        message: format!(
            "git clone exceeded quota and cleanup of `{}` failed: {err}",
            destination.as_str()
        ),
    })?;
    Err(original)
}

fn scrub_git_reserved_metadata(stdout: &str) -> String {
    stdout
        .lines()
        .filter(|line| !line_mentions_reserved_metadata(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_mentions_reserved_metadata(line: &str) -> bool {
    line.split_whitespace()
        .any(|part| part == ".labby-state" || part.starts_with(".labby-state/"))
}

#[derive(Clone, Copy)]
enum RemoteUrlMode {
    Fetch,
    Push,
}

async fn ensure_remote_urls_allowed(
    workspace_root: &Path,
    remote: &str,
    mode: RemoteUrlMode,
) -> Result<(), ToolError> {
    let mut args = match mode {
        RemoteUrlMode::Fetch => git_base_args(["remote", "get-url", "--all"]),
        RemoteUrlMode::Push => git_base_args(["remote", "get-url", "--push", "--all"]),
    };
    args.push(remote.to_string());
    let urls = run_git(workspace_root, &args).await?;
    for url in urls.lines().map(str::trim).filter(|line| !line.is_empty()) {
        validate_remote_url(url, "remote")?;
    }
    Ok(())
}

pub(crate) async fn run_git(workspace_root: &Path, args: &[String]) -> Result<String, ToolError> {
    let git = git_binary();
    let mut command = Command::new(git);
    command
        .args(args)
        .current_dir(workspace_root)
        .env_clear()
        .env("PATH", git_search_path())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_device())
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let result = tokio::time::timeout(Duration::from_secs(10), run_capped_command(command))
        .await
        .map_err(|_| ToolError::Sdk {
            sdk_kind: "timeout".to_string(),
            message: "git command timed out".to_string(),
        })?;
    let output = result?;

    if output.stdout_truncated {
        return Err(git_output_too_large(MAX_GIT_STDOUT_BYTES, "stdout"));
    }
    if output.stderr_truncated {
        return Err(git_output_too_large(MAX_GIT_STDERR_BYTES, "stderr"));
    }
    let stdout = bounded_git_output(&output.stdout, MAX_GIT_STDOUT_BYTES, "stdout")?;
    let stderr = bounded_git_output(&output.stderr, MAX_GIT_STDERR_BYTES, "stderr")?;
    if !output.status.success() {
        return Err(ToolError::Sdk {
            sdk_kind: "git_failed".to_string(),
            message: format!("git failed: {stderr}"),
        });
    }
    Ok(stdout)
}

struct CappedGitOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stdout_truncated: bool,
    stderr: Vec<u8>,
    stderr_truncated: bool,
}

struct CappedPipe {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn run_capped_command(mut command: Command) -> Result<CappedGitOutput, ToolError> {
    let mut child = command.spawn().map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to run git: {err}"),
    })?;
    let stdout = child.stdout.take().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "failed to capture git stdout".to_string(),
    })?;
    let stderr = child.stderr.take().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "failed to capture git stderr".to_string(),
    })?;

    let mut stdout_task = tokio::spawn(read_capped_pipe(stdout, MAX_GIT_STDOUT_BYTES));
    let mut stderr_task = tokio::spawn(read_capped_pipe(stderr, MAX_GIT_STDERR_BYTES));
    let mut stdout_result = None;
    let mut stderr_result = None;

    while stdout_result.is_none() || stderr_result.is_none() {
        tokio::select! {
            result = &mut stdout_task, if stdout_result.is_none() => {
                let pipe = join_capped_pipe(result)?;
                if pipe.truncated {
                    drop(child.start_kill());
                }
                stdout_result = Some(pipe);
            }
            result = &mut stderr_task, if stderr_result.is_none() => {
                let pipe = join_capped_pipe(result)?;
                if pipe.truncated {
                    drop(child.start_kill());
                }
                stderr_result = Some(pipe);
            }
        }
    }

    let status = child.wait().await.map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to wait for git: {err}"),
    })?;
    Ok(CappedGitOutput {
        status,
        stdout: stdout_result
            .as_ref()
            .expect("stdout result set")
            .bytes
            .clone(),
        stdout_truncated: stdout_result.expect("stdout result set").truncated,
        stderr: stderr_result
            .as_ref()
            .expect("stderr result set")
            .bytes
            .clone(),
        stderr_truncated: stderr_result.expect("stderr result set").truncated,
    })
}

async fn read_capped_pipe<R>(reader: R, max_bytes: usize) -> Result<CappedPipe, std::io::Error>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut bytes = Vec::with_capacity(max_bytes.min(8192));
    reader
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    let truncated = bytes.len() > max_bytes;
    bytes.truncate(max_bytes);
    Ok(CappedPipe { bytes, truncated })
}

fn join_capped_pipe(
    result: Result<Result<CappedPipe, std::io::Error>, tokio::task::JoinError>,
) -> Result<CappedPipe, ToolError> {
    result
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to join git output reader: {err}"),
        })?
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to read git output: {err}"),
        })
}

fn git_binary() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from("git.exe")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/usr/bin/git")
    }
}

fn git_search_path() -> &'static str {
    #[cfg(windows)]
    {
        r"C:\Program Files\Git\cmd;C:\Program Files\Git\bin;C:\Windows\System32;C:\Windows"
    }
    #[cfg(not(windows))]
    {
        "/usr/local/bin:/usr/bin:/bin"
    }
}

fn null_device() -> &'static str {
    #[cfg(windows)]
    {
        "NUL"
    }
    #[cfg(not(windows))]
    {
        "/dev/null"
    }
}

fn redact_git_output(value: &str) -> String {
    let https_userinfo =
        regex::Regex::new(r"https://[^/\s@]+@").expect("static https userinfo redaction regex");
    let value = https_userinfo
        .replace_all(value, "https://[REDACTED]@")
        .to_string();
    let tokenish = regex::Regex::new(r"(ghp_|github_pat_|glpat-)[A-Za-z0-9_]+")
        .expect("static git token redaction regex");
    tokenish.replace_all(&value, "[REDACTED]").to_string()
}

fn bounded_git_output(bytes: &[u8], max_bytes: usize, stream: &str) -> Result<String, ToolError> {
    if bytes.len() > max_bytes {
        return Err(git_output_too_large(max_bytes, stream));
    }
    Ok(redact_git_output(&String::from_utf8_lossy(bytes)))
}

fn git_output_too_large(max_bytes: usize, stream: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "response_too_large".to_string(),
        message: format!("git {stream} exceeded maximum output of {max_bytes} bytes"),
    }
}

fn parse_remote_list(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let name = parts.next()?;
            let url = parts.next()?;
            let kind = parts.next().unwrap_or_default().trim_matches(['(', ')']);
            Some(json!({ "name": name, "url": url, "kind": kind }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::quota::StateWorkspaceLimits;

    #[tokio::test]
    async fn git_provider_initializes_and_commits_workspace_file() {
        let temp = tempfile::tempdir().unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();
        workspace
            .write_file(
                &crate::state::path::VirtualPath::parse("src/app.rs").unwrap(),
                "fn main() {}\n",
            )
            .await
            .unwrap();

        dispatch_git_method(&workspace, "init", json!({}))
            .await
            .unwrap();
        dispatch_git_method(&workspace, "add", json!({"path": "src/app.rs"}))
            .await
            .unwrap();
        dispatch_git_method(
            &workspace,
            "commit",
            json!({
                "message": "initial state",
                "authorName": "Lab",
                "authorEmail": "lab@example.invalid"
            }),
        )
        .await
        .unwrap();
        let log = dispatch_git_method(&workspace, "log", json!({"limit": 1}))
            .await
            .unwrap();
        assert!(log["stdout"].as_str().unwrap().contains("initial state"));
    }
    #[test]
    fn redacts_only_https_userinfo_and_tokens() {
        let redacted = redact_git_output(
            "https://example.com/a https://user:pass@example.com/b ghp_abcdefghijklmnopqrstuvwxyz",
        );
        assert!(redacted.contains("https://example.com/a"));
        assert!(redacted.contains("https://[REDACTED]@example.com/b"));
        assert!(!redacted.contains("user:pass"));
        assert!(!redacted.contains("ghp_abcdefghijklmnopqrstuvwxyz"));
    }

    #[tokio::test]
    async fn git_v2_branch_checkout_and_remote_list_work_locally() {
        let temp = tempfile::tempdir().unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();
        workspace
            .write_file(
                &crate::state::path::VirtualPath::parse("README.md").unwrap(),
                "hi\n",
            )
            .await
            .unwrap();
        dispatch_git_method(&workspace, "init", json!({}))
            .await
            .unwrap();
        dispatch_git_method(&workspace, "add", json!({"path": "README.md"}))
            .await
            .unwrap();
        dispatch_git_method(
            &workspace,
            "commit",
            json!({"message": "init", "authorName": "Lab", "authorEmail": "lab@example.invalid"}),
        )
        .await
        .unwrap();
        dispatch_git_method(&workspace, "branch", json!({"name": "feature/demo"}))
            .await
            .unwrap();
        let branches = dispatch_git_method(&workspace, "branch", json!({}))
            .await
            .unwrap();
        assert!(
            branches["stdout"]
                .as_str()
                .unwrap()
                .contains("feature/demo")
        );
        dispatch_git_method(&workspace, "checkout", json!({"ref": "feature/demo"}))
            .await
            .unwrap();
        let remotes = dispatch_git_method(&workspace, "remoteList", json!({}))
            .await
            .unwrap();
        assert_eq!(remotes["ok"], true);
        assert!(remotes["remotes"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn git_v2_does_not_expose_deferred_remote_mutations() {
        let temp = tempfile::tempdir().unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();
        dispatch_git_method(&workspace, "init", json!({}))
            .await
            .unwrap();

        for method in ["fetch", "pull", "push"] {
            let err = dispatch_git_method(
                &workspace,
                method,
                json!({"branch": "HEAD", "remote": "origin"}),
            )
            .await
            .unwrap_err();
            assert_eq!(err.kind(), "unknown_tool", "{method} should be deferred");
        }
    }

    #[tokio::test]
    async fn git_v2_rejects_refs_that_git_rejects_before_command() {
        let temp = tempfile::tempdir().unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();
        dispatch_git_method(&workspace, "init", json!({}))
            .await
            .unwrap();

        let err = dispatch_git_method(&workspace, "branch", json!({"name": "foo@{bar"}))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[tokio::test]
    async fn git_v2_can_operate_inside_cloned_child_repo() {
        let source = tempfile::tempdir().unwrap();
        run_git(source.path(), &["init".to_string()]).await.unwrap();
        std::fs::write(source.path().join("README.md"), "hi\n").unwrap();
        run_git(
            source.path(),
            &["add".to_string(), "--".to_string(), "README.md".to_string()],
        )
        .await
        .unwrap();
        run_git(
            source.path(),
            &[
                "-c".to_string(),
                "user.name=Lab".to_string(),
                "-c".to_string(),
                "user.email=lab@example.invalid".to_string(),
                "commit".to_string(),
                "--no-gpg-sign".to_string(),
                "-m".to_string(),
                "init".to_string(),
            ],
        )
        .await
        .unwrap();

        let temp = tempfile::tempdir().unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();
        run_git(
            workspace.root_path(),
            &[
                "clone".to_string(),
                "--".to_string(),
                source.path().to_string_lossy().to_string(),
                "repo".to_string(),
            ],
        )
        .await
        .unwrap();

        let status = dispatch_git_method(&workspace, "status", json!({"cwd": "repo"}))
            .await
            .unwrap();
        assert_eq!(status["stdout"], "");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn git_v2_rejects_symlink_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), temp.path().join("link")).unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();

        let err = dispatch_git_method(&workspace, "status", json!({"cwd": "link"}))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "symlink_rejected");
    }

    #[tokio::test]
    async fn git_status_hides_reserved_runtime_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();
        dispatch_git_method(&workspace, "init", json!({}))
            .await
            .unwrap();
        std::fs::create_dir_all(temp.path().join(".labby-state/plans")).unwrap();
        std::fs::write(temp.path().join(".labby-state/plans/abc.json"), "{}").unwrap();
        workspace
            .write_file(
                &crate::state::path::VirtualPath::parse("visible.txt").unwrap(),
                "visible\n",
            )
            .await
            .unwrap();

        let status = dispatch_git_method(&workspace, "status", json!({}))
            .await
            .unwrap();
        let stdout = status["stdout"].as_str().unwrap();

        assert!(stdout.contains("visible.txt"));
        assert!(!stdout.contains(".labby-state"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn git_v2_rejects_symlink_clone_destination_before_network_call() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), temp.path().join("link")).unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();

        let err = dispatch_git_method(
            &workspace,
            "clone",
            json!({
                "url": "https://github.com/jmagar/example.git",
                "directory": "link"
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.kind(), "symlink_rejected");
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn git_v2_clone_requires_absent_destination() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("repo")).unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();

        let err = dispatch_git_method(
            &workspace,
            "clone",
            json!({
                "url": "https://github.com/jmagar/example.git",
                "directory": "repo"
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.kind(), "invalid_param");
        assert!(
            std::fs::read_dir(temp.path().join("repo"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    fn git_output_caps_error_instead_of_silently_truncating() {
        let bytes = vec![b'x'; MAX_GIT_STDOUT_BYTES + 1];
        let err = bounded_git_output(&bytes, MAX_GIT_STDOUT_BYTES, "stdout").unwrap_err();
        assert_eq!(err.kind(), "response_too_large");
    }

    #[tokio::test]
    async fn git_v2_remote_list_preserves_plain_https_urls() {
        let temp = tempfile::tempdir().unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();
        dispatch_git_method(&workspace, "init", json!({}))
            .await
            .unwrap();
        dispatch_git_method(
            &workspace,
            "remoteAdd",
            json!({"name": "origin", "url": "https://github.com/jmagar/example.git"}),
        )
        .await
        .unwrap();

        let remotes = dispatch_git_method(&workspace, "remoteList", json!({}))
            .await
            .unwrap();
        assert!(
            remotes["stdout"]
                .as_str()
                .unwrap()
                .contains("https://github.com/")
        );
        let rows = remotes["remotes"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "origin");
        assert_eq!(rows[0]["url"], "https://github.com/jmagar/example.git");
        assert_eq!(rows[0]["kind"], "fetch");
        assert_eq!(rows[1]["name"], "origin");
        assert_eq!(rows[1]["url"], "https://github.com/jmagar/example.git");
        assert_eq!(rows[1]["kind"], "push");
    }
}
