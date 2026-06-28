use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::ToolError;

use super::path::VirtualPath;
use super::workspace::{FileEdit, StateWorkspace, default_search_limit, default_true};

#[derive(Deserialize)]
struct PathParams {
    path: String,
}

#[derive(Deserialize)]
struct WriteFileParams {
    path: String,
    content: String,
}

#[derive(Deserialize)]
struct OptionalRecursivePathParams {
    path: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(Deserialize)]
struct FromToParams {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct WalkTreeParams {
    path: String,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct WriteJsonParams {
    path: String,
    value: Value,
    #[serde(default)]
    pretty: bool,
}

#[derive(Deserialize)]
struct HashFileParams {
    path: String,
    algorithm: String,
}

#[derive(Deserialize)]
struct ArchiveCreateParams {
    source: String,
    destination: String,
}

#[derive(Deserialize)]
struct ArchiveListParams {
    path: String,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct GlobParams {
    pattern: String,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct SearchFilesParams {
    pattern: String,
    query: String,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct ReplaceInFilesParams {
    pattern: String,
    search: String,
    replace: String,
    #[serde(default = "default_true", rename = "dryRun")]
    dry_run: bool,
}

#[derive(Deserialize)]
struct PlanEditsParams {
    edits: Vec<FileEdit>,
}

#[derive(Deserialize)]
struct ApplyEditPlanParams {
    #[serde(rename = "planId", alias = "plan_id")]
    plan_id: String,
}

pub(crate) async fn dispatch_state_method(
    workspace: &StateWorkspace,
    method: &str,
    params: Value,
) -> Result<Value, ToolError> {
    match method {
        "readFile" => {
            let params: PathParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace
                .read_file(&VirtualPath::parse(&params.path)?)
                .await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "writeFile" => {
            let params: WriteFileParams = serde_json::from_value(params).map_err(invalid_params)?;
            workspace
                .write_file(&VirtualPath::parse(&params.path)?, &params.content)
                .await?;
            Ok(json!({ "ok": true, "path": params.path }))
        }
        "appendFile" => {
            let params: WriteFileParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace
                .append_file(&VirtualPath::parse(&params.path)?, &params.content)
                .await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "exists" => {
            let params: PathParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace.exists(&VirtualPath::parse(&params.path)?).await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "stat" | "lstat" => {
            let params: PathParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace.stat(&VirtualPath::parse(&params.path)?).await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "mkdir" => {
            let params: PathParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace.mkdir(&VirtualPath::parse(&params.path)?).await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "rm" => {
            let params: OptionalRecursivePathParams =
                serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace
                .remove(&VirtualPath::parse(&params.path)?, params.recursive)
                .await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "cp" => {
            let params: FromToParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace
                .copy(
                    &VirtualPath::parse(&params.from)?,
                    &VirtualPath::parse(&params.to)?,
                )
                .await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "mv" => {
            let params: FromToParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace
                .move_path(
                    &VirtualPath::parse(&params.from)?,
                    &VirtualPath::parse(&params.to)?,
                )
                .await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "walkTree" | "summarizeTree" => {
            let params: WalkTreeParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace
                .walk_tree(
                    &VirtualPath::parse(&params.path)?,
                    params.limit.unwrap_or(default_search_limit()),
                )
                .await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "readJson" => {
            let params: PathParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace.read_json(&VirtualPath::parse(&params.path)?).await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "writeJson" => {
            let params: WriteJsonParams = serde_json::from_value(params).map_err(invalid_params)?;
            workspace
                .write_json(
                    &VirtualPath::parse(&params.path)?,
                    &params.value,
                    params.pretty,
                )
                .await?;
            Ok(json!({ "ok": true, "path": params.path }))
        }
        "hashFile" => {
            let params: HashFileParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace
                .hash_file(&VirtualPath::parse(&params.path)?, &params.algorithm)
                .await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "detectFile" => {
            let params: PathParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace
                .detect_file(&VirtualPath::parse(&params.path)?)
                .await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "archiveCreate" => {
            let params: ArchiveCreateParams =
                serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace
                .archive_create(
                    &VirtualPath::parse(&params.source)?,
                    &VirtualPath::parse(&params.destination)?,
                )
                .await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "archiveList" => {
            let params: ArchiveListParams =
                serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace
                .archive_list(
                    &VirtualPath::parse(&params.path)?,
                    params.limit.unwrap_or(default_search_limit()),
                )
                .await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "list" | "readdir" => {
            let params: PathParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace.list(&VirtualPath::parse(&params.path)?).await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "glob" => {
            let params: GlobParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace
                .glob(
                    &params.pattern,
                    params.limit.unwrap_or(default_search_limit()),
                )
                .await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "searchFiles" => {
            let params: SearchFilesParams =
                serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace
                .search_files(
                    &params.pattern,
                    &params.query,
                    params.limit.unwrap_or(default_search_limit()),
                )
                .await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "replaceInFiles" => {
            let params: ReplaceInFilesParams =
                serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace
                .replace_in_files(
                    &params.pattern,
                    &params.search,
                    &params.replace,
                    params.dry_run,
                )
                .await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "planEdits" => {
            let params: PlanEditsParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace.plan_edits(params.edits).await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "applyEditPlan" => {
            let params: ApplyEditPlanParams =
                serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace.apply_edit_plan(&params.plan_id).await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        other => Err(ToolError::Sdk {
            sdk_kind: "unknown_tool".to_string(),
            message: format!("unknown state method `{other}`"),
        }),
    }
}

fn invalid_params(err: serde_json::Error) -> ToolError {
    ToolError::InvalidParam {
        message: format!("invalid state params: {err}"),
        param: "params".to_string(),
    }
}

fn serialize_error(err: serde_json::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to serialize state result: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::state::quota::StateWorkspaceLimits;

    #[tokio::test]
    async fn write_and_read_file_dispatch_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();
        dispatch_state_method(
            &workspace,
            "writeFile",
            json!({
                "path": "/src/app.rs",
                "content": "fn main() {}\n"
            }),
        )
        .await
        .unwrap();
        let result = dispatch_state_method(
            &workspace,
            "readFile",
            json!({
                "path": "src/app.rs"
            }),
        )
        .await
        .unwrap();
        assert_eq!(result["content"], "fn main() {}\n");
    }

    #[tokio::test]
    async fn search_replace_plan_and_apply_dispatch() {
        let temp = tempfile::tempdir().unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();
        dispatch_state_method(
            &workspace,
            "writeFile",
            json!({"path": "src/app.rs", "content": "println!(\"hi\");\n"}),
        )
        .await
        .unwrap();

        let matches = dispatch_state_method(
            &workspace,
            "searchFiles",
            json!({"pattern": "src/**/*.rs", "query": "println"}),
        )
        .await
        .unwrap();
        assert_eq!(matches["matches"].as_array().unwrap().len(), 1);

        let dry_run = dispatch_state_method(
            &workspace,
            "replaceInFiles",
            json!({
                "pattern": "src/**/*.rs",
                "search": "println",
                "replace": "eprintln",
                "dryRun": true
            }),
        )
        .await
        .unwrap();
        assert_eq!(dry_run["dry_run"], true);

        let plan = dispatch_state_method(
            &workspace,
            "planEdits",
            json!({"edits": [{"path": "src/app.rs", "search": "println", "replace": "eprintln"}]}),
        )
        .await
        .unwrap();
        let plan_id = plan["plan_id"].as_str().unwrap();
        dispatch_state_method(&workspace, "applyEditPlan", json!({"planId": plan_id}))
            .await
            .unwrap();
        let result = dispatch_state_method(&workspace, "readFile", json!({"path": "src/app.rs"}))
            .await
            .unwrap();
        assert!(result["content"].as_str().unwrap().contains("eprintln"));
    }

    #[tokio::test]
    async fn v2_state_filesystem_methods_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();

        dispatch_state_method(&workspace, "mkdir", json!({"path": "src"}))
            .await
            .unwrap();
        dispatch_state_method(
            &workspace,
            "writeFile",
            json!({"path": "src/app.rs", "content": "fn main() {}\n"}),
        )
        .await
        .unwrap();
        dispatch_state_method(
            &workspace,
            "appendFile",
            json!({"path": "src/app.rs", "content": "// tail\n"}),
        )
        .await
        .unwrap();

        let stat = dispatch_state_method(&workspace, "stat", json!({"path": "src/app.rs"}))
            .await
            .unwrap();
        assert_eq!(stat["kind"], "file");
        assert!(stat["bytes"].as_u64().unwrap() > 0);

        let exists = dispatch_state_method(&workspace, "exists", json!({"path": "src/app.rs"}))
            .await
            .unwrap();
        assert_eq!(exists["exists"], true);

        dispatch_state_method(
            &workspace,
            "cp",
            json!({"from": "src/app.rs", "to": "src/copy.rs"}),
        )
        .await
        .unwrap();
        dispatch_state_method(
            &workspace,
            "mv",
            json!({"from": "src/copy.rs", "to": "src/moved.rs"}),
        )
        .await
        .unwrap();
        let tree =
            dispatch_state_method(&workspace, "walkTree", json!({"path": "src", "limit": 10}))
                .await
                .unwrap();
        assert!(
            tree["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["path"] == "src/moved.rs")
        );

        dispatch_state_method(&workspace, "rm", json!({"path": "src/moved.rs"}))
            .await
            .unwrap();
        let gone = dispatch_state_method(&workspace, "exists", json!({"path": "src/moved.rs"}))
            .await
            .unwrap();
        assert_eq!(gone["exists"], false);
    }

    #[tokio::test]
    async fn v2_json_hash_and_detect_methods_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();

        dispatch_state_method(
            &workspace,
            "writeJson",
            json!({
                "path": "data/config.json",
                "value": {"enabled": true, "count": 2},
                "pretty": true
            }),
        )
        .await
        .unwrap();

        let json_value =
            dispatch_state_method(&workspace, "readJson", json!({"path": "data/config.json"}))
                .await
                .unwrap();
        assert_eq!(json_value["value"]["enabled"], true);

        let hash = dispatch_state_method(
            &workspace,
            "hashFile",
            json!({"path": "data/config.json", "algorithm": "sha256"}),
        )
        .await
        .unwrap();
        assert_eq!(hash["algorithm"], "sha256");
        assert_eq!(hash["hex"].as_str().unwrap().len(), 64);

        let detected =
            dispatch_state_method(&workspace, "detectFile", json!({"path": "data/config.json"}))
                .await
                .unwrap();
        assert_eq!(detected["extension"], "json");
        assert_eq!(detected["text"], true);
    }

    #[tokio::test]
    async fn v2_archive_create_and_list_stays_in_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();

        dispatch_state_method(
            &workspace,
            "writeFile",
            json!({"path": "src/a.txt", "content": "a"}),
        )
        .await
        .unwrap();
        dispatch_state_method(
            &workspace,
            "writeFile",
            json!({"path": "src/b.txt", "content": "b"}),
        )
        .await
        .unwrap();
        dispatch_state_method(
            &workspace,
            "archiveCreate",
            json!({"source": "src", "destination": "out/src.tar"}),
        )
        .await
        .unwrap();
        let listing = dispatch_state_method(
            &workspace,
            "archiveList",
            json!({"path": "out/src.tar", "limit": 10}),
        )
        .await
        .unwrap();
        assert!(
            listing["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry == "a.txt")
        );
        assert!(
            listing["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry == "b.txt")
        );
    }
}
