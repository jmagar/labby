use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::error::ToolError;
use crate::state::workspace::StateWorkspace;

use super::command::GitCommandSpec;

const MAX_GIT_STDOUT_BYTES: usize = 64 * 1024;
const MAX_GIT_STDERR_BYTES: usize = 16 * 1024;

pub(crate) async fn dispatch_git_method(
    workspace: &StateWorkspace,
    method: &str,
    params: Value,
) -> Result<Value, ToolError> {
    let spec = GitCommandSpec::for_method(method, params)?;
    let stdout = run_git(workspace.root_path(), &spec.args).await?;
    Ok(json!({ "ok": true, "stdout": stdout }))
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

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(ToolError::Sdk {
            sdk_kind: "git_failed".to_string(),
            message: format!("git failed: {}", redact_git_output(&stderr)),
        });
    }
    Ok(redact_git_output(&stdout))
}

struct CappedGitOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
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
        stdout: stdout_result.expect("stdout result set").bytes,
        stderr: stderr_result.expect("stderr result set").bytes,
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
        dispatch_git_method(&workspace, "checkout", json!({"ref": "feature/demo"}))
            .await
            .unwrap();
        let remotes = dispatch_git_method(&workspace, "remoteList", json!({}))
            .await
            .unwrap();
        assert_eq!(remotes["ok"], true);
    }
}
