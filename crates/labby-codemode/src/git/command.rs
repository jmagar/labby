use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::error::ToolError;
use crate::state::path::VirtualPath;

const ALLOWED_REMOTE_HOSTS: &[&str] = &["github.com"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitCommandSpec {
    pub(crate) args: Vec<String>,
    pub(crate) cwd: Option<VirtualPath>,
    pub(crate) remote_preflight: Option<String>,
    pub(crate) push_remote_preflight: Option<String>,
    pub(crate) branch_preflight: Option<String>,
    pub(crate) clone_destination: Option<VirtualPath>,
}

impl GitCommandSpec {
    pub(crate) fn status() -> Self {
        Self {
            args: git_base_args(["status", "--short"]),
            cwd: None,
            remote_preflight: None,
            push_remote_preflight: None,
            branch_preflight: None,
            clone_destination: None,
        }
    }

    pub(crate) fn for_method(method: &str, params: Value) -> Result<Self, ToolError> {
        match method {
            "init" => Ok(Self {
                args: git_base_args(["init"]),
                cwd: parse_cwd(params)?,
                remote_preflight: None,
                push_remote_preflight: None,
                branch_preflight: None,
                clone_destination: None,
            }),
            "status" => {
                let cwd = parse_cwd(params)?;
                Ok(Self {
                    cwd,
                    ..Self::status()
                })
            }
            "add" => {
                let params: PathParams = parse_params(params)?;
                let path = VirtualPath::parse(&params.path)?;
                let mut args = git_base_args(["add", "--"]);
                args.push(path.as_str().to_string());
                Ok(Self {
                    args,
                    cwd: parse_optional_cwd(params.cwd)?,
                    remote_preflight: None,
                    push_remote_preflight: None,
                    branch_preflight: None,
                    clone_destination: None,
                })
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
                let mut args = git_base_args([
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
                Ok(Self {
                    args,
                    cwd: parse_optional_cwd(params.cwd)?,
                    remote_preflight: None,
                    push_remote_preflight: None,
                    branch_preflight: None,
                    clone_destination: None,
                })
            }
            "log" => {
                let params: LimitParams = parse_params(params)?;
                let limit = params.limit.unwrap_or(20).clamp(1, 50).to_string();
                let mut args = git_base_args(["log", "--oneline", "-n"]);
                args.push(limit);
                Ok(Self {
                    args,
                    cwd: parse_optional_cwd(params.cwd)?,
                    remote_preflight: None,
                    push_remote_preflight: None,
                    branch_preflight: None,
                    clone_destination: None,
                })
            }
            "diff" => {
                let params: OptionalPathParams = parse_params(params)?;
                let mut args = git_base_args(["diff", "--"]);
                if let Some(path) = params.path {
                    args.push(VirtualPath::parse(&path)?.as_str().to_string());
                }
                Ok(Self {
                    args,
                    cwd: parse_optional_cwd(params.cwd)?,
                    remote_preflight: None,
                    push_remote_preflight: None,
                    branch_preflight: None,
                    clone_destination: None,
                })
            }
            "branch" => {
                let params: BranchParams = parse_params(params)?;
                if params.delete && params.name.is_none() {
                    return Err(invalid_param(
                        "name",
                        "git branch delete requires a branch name",
                    ));
                }
                if params.list || params.name.is_none() {
                    return Ok(Self {
                        args: git_base_args(["branch", "--list"]),
                        cwd: parse_optional_cwd(params.cwd)?,
                        remote_preflight: None,
                        push_remote_preflight: None,
                        branch_preflight: None,
                        clone_destination: None,
                    });
                }
                let name = params.name.unwrap();
                validate_git_ref(&name, "name")?;
                let mut args = if params.delete {
                    git_base_args(["branch", "-D"])
                } else {
                    git_base_args(["branch"])
                };
                args.push(name.clone());
                Ok(Self {
                    args,
                    cwd: parse_optional_cwd(params.cwd)?,
                    remote_preflight: None,
                    push_remote_preflight: None,
                    branch_preflight: Some(name),
                    clone_destination: None,
                })
            }
            "checkout" => {
                let params: CheckoutParams = parse_params(params)?;
                validate_git_ref(&params.git_ref, "ref")?;
                let mut args = if params.create {
                    git_base_args(["checkout", "-b"])
                } else {
                    git_base_args(["checkout"])
                };
                args.push(params.git_ref.clone());
                Ok(Self {
                    args,
                    cwd: parse_optional_cwd(params.cwd)?,
                    remote_preflight: None,
                    push_remote_preflight: None,
                    branch_preflight: Some(params.git_ref),
                    clone_destination: None,
                })
            }
            "remoteList" => Ok(Self {
                args: git_base_args(["remote", "-v"]),
                cwd: parse_cwd(params)?,
                remote_preflight: None,
                push_remote_preflight: None,
                branch_preflight: None,
                clone_destination: None,
            }),
            "remoteAdd" => {
                let params: RemoteAddParams = parse_params(params)?;
                validate_remote_name(&params.name, "name")?;
                validate_remote_url(&params.url, "url")?;
                let mut args = git_base_args(["remote", "add"]);
                args.push(params.name);
                args.push(params.url);
                Ok(Self {
                    args,
                    cwd: parse_optional_cwd(params.cwd)?,
                    remote_preflight: None,
                    push_remote_preflight: None,
                    branch_preflight: None,
                    clone_destination: None,
                })
            }
            "remoteRemove" => {
                let params: RemoteNameParams = parse_params(params)?;
                validate_remote_name(&params.name, "name")?;
                let mut args = git_base_args(["remote", "remove"]);
                args.push(params.name);
                Ok(Self {
                    args,
                    cwd: parse_optional_cwd(params.cwd)?,
                    remote_preflight: None,
                    push_remote_preflight: None,
                    branch_preflight: None,
                    clone_destination: None,
                })
            }
            "clone" => {
                let params: CloneParams = parse_params(params)?;
                validate_remote_url(&params.url, "url")?;
                let directory = VirtualPath::parse(&params.directory)?;
                validate_clone_directory(directory.as_str())?;
                let mut args = git_base_args(["clone", "--depth", "1", "--"]);
                args.push(params.url);
                args.push(directory.as_str().to_string());
                Ok(Self {
                    args,
                    cwd: parse_optional_cwd(params.cwd)?,
                    remote_preflight: None,
                    push_remote_preflight: None,
                    branch_preflight: None,
                    clone_destination: Some(directory),
                })
            }
            other => Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown git method `{other}`"),
            }),
        }
    }
}

pub(crate) fn git_base_args<const N: usize>(tail: [&str; N]) -> Vec<String> {
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

fn parse_cwd(params: Value) -> Result<Option<VirtualPath>, ToolError> {
    parse_optional_cwd(parse_params::<CwdParams>(params)?.cwd)
}

fn parse_optional_cwd(cwd: Option<String>) -> Result<Option<VirtualPath>, ToolError> {
    cwd.map(|value| VirtualPath::parse(&value)).transpose()
}

#[derive(Deserialize)]
struct CwdParams {
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct PathParams {
    path: String,
    cwd: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitParams {
    message: String,
    author_name: String,
    author_email: String,
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct LimitParams {
    limit: Option<usize>,
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct OptionalPathParams {
    path: Option<String>,
    cwd: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BranchParams {
    name: Option<String>,
    #[serde(default)]
    list: bool,
    #[serde(default)]
    delete: bool,
    cwd: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckoutParams {
    #[serde(rename = "ref")]
    git_ref: String,
    #[serde(default)]
    create: bool,
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct RemoteNameParams {
    name: String,
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct RemoteAddParams {
    name: String,
    url: String,
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct CloneParams {
    url: String,
    directory: String,
    cwd: Option<String>,
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

pub(crate) fn validate_remote_url(value: &str, param: &str) -> Result<(), ToolError> {
    let parsed = Url::parse(value).map_err(|_| {
        invalid_param(
            param,
            "git remote URL must be a valid https URL on an allowed host",
        )
    })?;
    if parsed.scheme() != "https" {
        return Err(invalid_param(param, "git remote URL must use https"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(invalid_param(
            param,
            "git remote URL must not include embedded credentials",
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(invalid_param(
            param,
            "git remote URL must not include query or fragment",
        ));
    }
    let Some(host) = parsed.host_str() else {
        return Err(invalid_param(param, "git remote URL must include a host"));
    };
    if !ALLOWED_REMOTE_HOSTS.contains(&host) {
        return Err(invalid_param(
            param,
            "git remote host is not allowed for Code Mode",
        ));
    }
    if parsed.path() == "/" || parsed.path().trim_matches('/').is_empty() {
        return Err(invalid_param(param, "git remote URL must include a path"));
    }
    Ok(())
}

fn validate_clone_directory(value: &str) -> Result<(), ToolError> {
    if value
        .split('/')
        .any(|part| part.eq_ignore_ascii_case(".git") || part.eq_ignore_ascii_case(".labby-state"))
    {
        return Err(invalid_param(
            "directory",
            "git clone directory must not include reserved metadata paths",
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
        assert_eq!(cmd.cwd, None);
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
            "https://github.com/owner/repo.git?token=x",
            "https://example.com/owner/repo.git",
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
        assert!(
            branch
                .args
                .ends_with(&["branch".to_string(), "feature/demo".to_string()])
        );

        let checkout =
            GitCommandSpec::for_method("checkout", serde_json::json!({"ref": "feature/demo"}))
                .unwrap();
        assert!(
            checkout
                .args
                .ends_with(&["checkout".to_string(), "feature/demo".to_string()])
        );

        let remote = GitCommandSpec::for_method(
            "remoteAdd",
            serde_json::json!({"name": "origin", "url": "https://github.com/jmagar/example.git"}),
        )
        .unwrap();
        assert!(remote.args.iter().any(|arg| arg == "remote"));
    }

    #[test]
    fn git_v2_builds_workspace_relative_cwd() {
        let cmd = GitCommandSpec::for_method("status", serde_json::json!({"cwd": "repo"})).unwrap();
        assert_eq!(cmd.cwd.unwrap().as_str(), "repo");

        let err = GitCommandSpec::for_method("status", serde_json::json!({"cwd": "../repo"}))
            .unwrap_err();
        assert_eq!(err.kind(), "path_traversal");
    }
}
