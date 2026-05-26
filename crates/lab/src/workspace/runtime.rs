use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use axum::Router;
use serde_json::Value;

use crate::api::state::AppState;
use crate::config::LabConfig;
use crate::dispatch::error::ToolError;
use crate::registry::{DispatchFn, RegisteredService};

fn workspace_dispatch(
    action: String,
    params: Value,
) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>> {
    Box::pin(async move { crate::mcp::services::fs::dispatch(&action, params).await })
}

#[derive(Debug, Clone)]
pub struct WorkspaceRuntime {
    workspace_root: Option<PathBuf>,
    workspace_root_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceRuntimeBuilder {
    config: LabConfig,
}

impl WorkspaceRuntimeBuilder {
    #[must_use]
    pub fn new(config: LabConfig) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn build(self) -> WorkspaceRuntime {
        match crate::dispatch::fs::resolve_workspace_root(&self.config) {
            Ok(workspace_root) => WorkspaceRuntime {
                workspace_root: Some(workspace_root),
                workspace_root_error: None,
            },
            Err(error) => WorkspaceRuntime {
                workspace_root: None,
                workspace_root_error: Some(error.to_string()),
            },
        }
    }
}

impl WorkspaceRuntime {
    #[must_use]
    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }

    #[must_use]
    pub fn workspace_root_error(&self) -> Option<&str> {
        self.workspace_root_error.as_deref()
    }

    #[must_use]
    pub fn registered_service() -> RegisteredService {
        let dispatch: DispatchFn = workspace_dispatch;
        RegisteredService::bootstrap(
            "fs",
            "Workspace filesystem browser (read-only, deny-listed)",
            "bootstrap",
            crate::mcp::services::fs::ACTIONS,
            dispatch,
        )
    }

    #[must_use]
    pub fn http_routes(state: AppState, api_auth_configured: bool) -> Option<Router<AppState>> {
        if state.web_ui_auth_disabled && !api_auth_configured {
            return None;
        }

        Some(crate::api::services::fs::routes(state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_resolves_configured_workspace_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = LabConfig::default();
        config.workspace.root = Some(temp.path().to_path_buf());

        let runtime = WorkspaceRuntimeBuilder::new(config).build();

        assert_eq!(
            runtime.workspace_root().expect("workspace root"),
            std::fs::canonicalize(temp.path()).expect("canonical")
        );
        assert!(runtime.workspace_root_error().is_none());
    }

    #[test]
    fn builder_keeps_invalid_workspace_root_unset() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = temp.path().join("not-a-dir");
        std::fs::write(&file, b"not a directory").expect("write");
        let mut config = LabConfig::default();
        config.workspace.root = Some(file);

        let runtime = WorkspaceRuntimeBuilder::new(config).build();

        assert!(runtime.workspace_root().is_none());
        assert!(runtime.workspace_root_error().is_some());
    }

    #[test]
    fn registered_service_uses_mcp_filtered_actions() {
        let service = WorkspaceRuntime::registered_service();
        let names: Vec<&str> = service.actions.iter().map(|action| action.name).collect();

        assert_eq!(service.name, "fs");
        assert!(names.contains(&"fs.list"));
        assert!(!names.contains(&"fs.preview"));
    }

    #[tokio::test]
    async fn http_routes_refuse_disabled_auth_without_api_auth() {
        let state = AppState::new().with_web_ui_auth_disabled(true);

        assert!(WorkspaceRuntime::http_routes(state, false).is_none());
    }

    #[tokio::test]
    async fn http_routes_mount_when_api_auth_is_configured() {
        let state = AppState::new().with_web_ui_auth_disabled(true);

        assert!(WorkspaceRuntime::http_routes(state, true).is_some());
    }
}
