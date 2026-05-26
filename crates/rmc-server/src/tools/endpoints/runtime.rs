//! Runtime lifecycle status and cleanup tools.

use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, Content},
    schemars,
};

use crate::mcp::{RuntimeClearRequest, RuntimeClearScope, RuntimeState};

#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct RuntimeStatusParams {}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ClearRuntimeParams {
    #[schemars(
        description = "Cleanup scope for in-memory runtime caches and sync tracking. Defaults to all. Values: all, workspace, semantic_only, search_cache_only, sync_tracking_only. This does not stop the background sync task; process shutdown cancels tasks through ServerRuntime."
    )]
    #[serde(default)]
    pub scope: Option<RuntimeClearScope>,
    #[schemars(
        description = "Optional workspace path. With scope=workspace this is required; with semantic_only/search_cache_only/sync_tracking_only it limits cleanup to one workspace."
    )]
    #[serde(default)]
    pub workspace: Option<String>,
}

pub(crate) async fn runtime_status(
    runtime: &RuntimeState,
    _params: RuntimeStatusParams,
) -> Result<CallToolResult, McpError> {
    let status = runtime.status().await;
    let body = serde_json::to_string_pretty(&status)
        .map_err(|e| McpError::internal_error(format!("Failed to serialize status: {}", e), None))?;
    Ok(CallToolResult::success(vec![Content::text(body)]))
}

pub(crate) async fn clear_runtime(
    runtime: &RuntimeState,
    params: ClearRuntimeParams,
) -> Result<CallToolResult, McpError> {
    let scope = params.scope.unwrap_or_default();
    let workspace = params.workspace.map(std::path::PathBuf::from);
    if scope == RuntimeClearScope::Workspace && workspace.is_none() {
        return Err(McpError::invalid_params(
            "workspace is required when scope is 'workspace'",
            None,
        ));
    }

    let report = runtime
        .clear(RuntimeClearRequest { scope, workspace })
        .await;
    let body = serde_json::to_string_pretty(&report)
        .map_err(|e| McpError::internal_error(format!("Failed to serialize clear report: {}", e), None))?;
    Ok(CallToolResult::success(vec![Content::text(body)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runtime_clear_workspace_requires_workspace_param() {
        let runtime = RuntimeState::standalone();

        let error = clear_runtime(
            &runtime,
            ClearRuntimeParams {
                scope: Some(RuntimeClearScope::Workspace),
                workspace: None,
            },
        )
        .await
        .expect_err("workspace scope should require workspace");

        assert!(error.to_string().contains("workspace is required"));
    }
}
