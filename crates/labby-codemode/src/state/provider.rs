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
}
