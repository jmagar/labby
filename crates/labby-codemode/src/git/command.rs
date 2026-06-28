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
        assert!(GitCommandSpec::for_method("push", serde_json::json!({})).is_err());
    }

    #[test]
    fn git_add_validates_virtual_path() {
        let err = GitCommandSpec::for_method("add", serde_json::json!({"path": "../outside"}))
            .unwrap_err();
        assert_eq!(err.kind(), "path_traversal");
    }
}
