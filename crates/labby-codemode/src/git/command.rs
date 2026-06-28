use serde::Deserialize;
use serde_json::Value;

use crate::error::ToolError;
use crate::state::path::VirtualPath;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitCommandSpec {
    pub(crate) args: Vec<String>,
}

impl GitCommandSpec {
    pub(crate) fn status() -> Self {
        Self {
            args: base_args(["status", "--short"]),
        }
    }

    pub(crate) fn for_method(method: &str, params: Value) -> Result<Self, ToolError> {
        match method {
            "init" => Ok(Self {
                args: base_args(["init"]),
            }),
            "status" => Ok(Self::status()),
            "add" => {
                let params: PathParams = parse_params(params)?;
                let path = VirtualPath::parse(&params.path)?;
                let mut args = base_args(["add", "--"]);
                args.push(path.as_str().to_string());
                Ok(Self { args })
            }
            "commit" => {
                let params: CommitParams = parse_params(params)?;
                if params.message.trim().is_empty() {
                    return Err(ToolError::InvalidParam {
                        message: "git commit message must not be empty".to_string(),
                        param: "message".to_string(),
                    });
                }
                let author = format!("{} <{}>", params.author_name, params.author_email);
                let mut args = base_args([
                    "-c",
                    "user.name=Lab",
                    "-c",
                    "user.email=lab@example.invalid",
                    "commit",
                    "--no-gpg-sign",
                    "--author",
                ]);
                args.push(author);
                args.push("-m".to_string());
                args.push(params.message);
                Ok(Self { args })
            }
            "log" => {
                let params: LimitParams = parse_params(params)?;
                let limit = params.limit.unwrap_or(20).clamp(1, 50).to_string();
                let mut args = base_args(["log", "--oneline", "-n"]);
                args.push(limit);
                Ok(Self { args })
            }
            "diff" => {
                let params: OptionalPathParams = parse_params(params)?;
                let mut args = base_args(["diff", "--"]);
                if let Some(path) = params.path {
                    args.push(VirtualPath::parse(&path)?.as_str().to_string());
                }
                Ok(Self { args })
            }
            "branch" => {
                let params: BranchParams = parse_params(params)?;
                validate_git_ref(&params.name, "name")?;
                let mut args = if params.delete {
                    base_args(["branch", "-D"])
                } else {
                    base_args(["branch"])
                };
                args.push(params.name);
                Ok(Self { args })
            }
            "checkout" => {
                let params: CheckoutParams = parse_params(params)?;
                validate_git_ref(&params.git_ref, "ref")?;
                let mut args = if params.create {
                    base_args(["checkout", "-b"])
                } else {
                    base_args(["checkout"])
                };
                args.push(params.git_ref);
                Ok(Self { args })
            }
            "remoteList" => Ok(Self {
                args: base_args(["remote", "-v"]),
            }),
            "remoteAdd" => {
                let params: RemoteAddParams = parse_params(params)?;
                validate_remote_name(&params.name, "name")?;
                validate_remote_url(&params.url, "url")?;
                let mut args = base_args(["remote", "add"]);
                args.push(params.name);
                args.push(params.url);
                Ok(Self { args })
            }
            "remoteRemove" => {
                let params: RemoteNameParams = parse_params(params)?;
                validate_remote_name(&params.name, "name")?;
                let mut args = base_args(["remote", "remove"]);
                args.push(params.name);
                Ok(Self { args })
            }
            "clone" => {
                let params: CloneParams = parse_params(params)?;
                validate_remote_url(&params.url, "url")?;
                let directory = VirtualPath::parse(&params.directory)?;
                validate_clone_directory(directory.as_str())?;
                let mut args = base_args(["clone", "--depth", "1", "--"]);
                args.push(params.url);
                args.push(directory.as_str().to_string());
                Ok(Self { args })
            }
            "fetch" => {
                let params: PullPushParams = parse_params(params)?;
                let remote = params.remote.unwrap_or_else(|| "origin".to_string());
                validate_remote_name(&remote, "remote")?;
                let mut args = base_args(["fetch"]);
                args.push(remote);
                Ok(Self { args })
            }
            "pull" => {
                let params: PullPushParams = parse_params(params)?;
                let remote = params.remote.unwrap_or_else(|| "origin".to_string());
                let branch = params.branch.unwrap_or_else(|| "HEAD".to_string());
                validate_remote_name(&remote, "remote")?;
                validate_git_ref(&branch, "branch")?;
                let mut args = base_args(["pull", "--ff-only"]);
                args.push(remote);
                args.push(branch);
                Ok(Self { args })
            }
            "push" => {
                let params: PullPushParams = parse_params(params)?;
                let remote = params.remote.unwrap_or_else(|| "origin".to_string());
                let branch = params.branch.unwrap_or_else(|| "HEAD".to_string());
                validate_remote_name(&remote, "remote")?;
                validate_git_ref(&branch, "branch")?;
                let mut args = base_args(["push"]);
                args.push(remote);
                args.push(branch);
                Ok(Self { args })
            }
            other => Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown git method `{other}`"),
            }),
        }
    }
}

fn base_args<const N: usize>(tail: [&str; N]) -> Vec<String> {
    let mut args = vec![
        "-c".to_string(),
        "core.hooksPath=/dev/null".to_string(),
        "-c".to_string(),
        "protocol.file.allow=never".to_string(),
        "-c".to_string(),
        "protocol.ext.allow=never".to_string(),
    ];
    args.extend(tail.into_iter().map(str::to_string));
    args
}

fn parse_params<T: for<'de> Deserialize<'de>>(params: Value) -> Result<T, ToolError> {
    serde_json::from_value(params).map_err(|err| ToolError::InvalidParam {
        message: format!("invalid git params: {err}"),
        param: "params".to_string(),
    })
}

#[derive(Deserialize)]
struct PathParams {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitParams {
    message: String,
    author_name: String,
    author_email: String,
}

#[derive(Deserialize)]
struct LimitParams {
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct OptionalPathParams {
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BranchParams {
    name: String,
    #[serde(default)]
    delete: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckoutParams {
    #[serde(rename = "ref")]
    git_ref: String,
    #[serde(default)]
    create: bool,
}

#[derive(Deserialize)]
struct RemoteNameParams {
    name: String,
}

#[derive(Deserialize)]
struct RemoteAddParams {
    name: String,
    url: String,
}

#[derive(Deserialize)]
struct CloneParams {
    url: String,
    directory: String,
}

#[derive(Deserialize)]
struct PullPushParams {
    remote: Option<String>,
    branch: Option<String>,
}

fn validate_remote_name(value: &str, param: &str) -> Result<(), ToolError> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'));
    if valid {
        Ok(())
    } else {
        Err(invalid_param(
            param,
            "git remote name must be 1-64 ASCII alnum, dash, underscore, or dot chars",
        ))
    }
}

fn validate_git_ref(value: &str, param: &str) -> Result<(), ToolError> {
    let invalid = value.trim().is_empty()
        || value != value.trim()
        || value.starts_with('-')
        || value.ends_with('/')
        || value.ends_with(".lock")
        || value.contains("..")
        || value
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '~' | '^' | ':' | '?' | '*' | '[' | '\\'));
    if invalid {
        Err(invalid_param(param, "git ref is not allowed"))
    } else {
        Ok(())
    }
}

fn validate_remote_url(value: &str, param: &str) -> Result<(), ToolError> {
    if !value.starts_with("https://") || value.contains('?') || value.contains('#') {
        return Err(invalid_param(
            param,
            "git remote URL must be an explicit https URL without query or fragment",
        ));
    }
    let remainder = &value["https://".len()..];
    let Some((authority, path)) = remainder.split_once('/') else {
        return Err(invalid_param(param, "git remote URL must include a path"));
    };
    if authority.is_empty() || path.is_empty() || authority.contains('@') {
        return Err(invalid_param(
            param,
            "git remote URL must not include embedded credentials",
        ));
    }
    Ok(())
}

fn validate_clone_directory(value: &str) -> Result<(), ToolError> {
    if value.split('/').any(|part| part == ".git") {
        return Err(invalid_param(
            "directory",
            "git clone directory must not include .git",
        ));
    }
    Ok(())
}

fn invalid_param(param: &str, message: &str) -> ToolError {
    ToolError::InvalidParam {
        message: message.to_string(),
        param: param.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_status_builds_fixed_argv() {
        let cmd = GitCommandSpec::status();
        assert_eq!(
            cmd.args,
            vec![
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "protocol.file.allow=never",
                "-c",
                "protocol.ext.allow=never",
                "status",
                "--short"
            ]
        );
    }

    #[test]
    fn git_rejects_unsupported_method() {
        assert!(GitCommandSpec::for_method("rebase", serde_json::json!({})).is_err());
    }

    #[test]
    fn git_add_validates_virtual_path() {
        let err = GitCommandSpec::for_method("add", serde_json::json!({"path": "../outside"}))
            .unwrap_err();
        assert_eq!(err.kind(), "path_traversal");
    }

    #[test]
    fn git_v2_rejects_unsafe_remote_urls() {
        for url in [
            "file:///tmp/repo",
            "ssh://host/repo",
            "git@github.com:owner/repo.git",
            "https://user:token@example.com/repo.git",
        ] {
            let err = GitCommandSpec::for_method(
                "remoteAdd",
                serde_json::json!({"name": "origin", "url": url}),
            )
            .unwrap_err();
            assert_eq!(err.kind(), "invalid_param");
        }
    }

    #[test]
    fn git_v2_builds_branch_checkout_and_remote_args() {
        let branch =
            GitCommandSpec::for_method("branch", serde_json::json!({"name": "feature/demo"}))
                .unwrap();
        assert!(branch
            .args
            .ends_with(&["branch".to_string(), "feature/demo".to_string()]));

        let checkout =
            GitCommandSpec::for_method("checkout", serde_json::json!({"ref": "feature/demo"}))
                .unwrap();
        assert!(checkout.args.ends_with(&[
            "checkout".to_string(),
            "feature/demo".to_string()
        ]));

        let remote = GitCommandSpec::for_method(
            "remoteAdd",
            serde_json::json!({"name": "origin", "url": "https://github.com/jmagar/example.git"}),
        )
        .unwrap();
        assert!(remote.args.iter().any(|arg| arg == "remote"));
    }
}
