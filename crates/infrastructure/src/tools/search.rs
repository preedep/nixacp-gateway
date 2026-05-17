use async_trait::async_trait;
use std::path::PathBuf;

use domain::entities::tool::{ToolCall, ToolError, ToolResult};
use domain::ports::tool_runtime::ToolRuntime;

/// Searches for text matches using the `ripgrep` binary (`rg`).
///
/// Runs under a 10-second timeout enforced by `ToolExecutor`; the process
/// itself is spawned via `tokio::process::Command` so it is cancellation-safe.
pub struct SearchTool {
    workspace_root: PathBuf,
    ripgrep_bin: String,
}

impl SearchTool {
    pub fn new(workspace_root: impl Into<PathBuf>, ripgrep_bin: impl Into<String>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            ripgrep_bin: ripgrep_bin.into(),
        }
    }
}

#[async_trait]
impl ToolRuntime for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, ToolError> {
        let args = call.function.parse_arguments()
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let query = args["query"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("missing 'query' field".into()))?;

        // Optional sub-path within workspace root.
        let search_path = match args["path"].as_str() {
            Some(p) => self.workspace_root.join(p),
            None => self.workspace_root.clone(),
        };

        let output = tokio::process::Command::new(&self.ripgrep_bin)
            .args([
                "--line-number",
                "--no-heading",
                "--color=never",
                "--max-count=50",
                query,
                search_path.to_str().unwrap_or("."),
            ])
            .output()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("failed to run rg: {e}")))?;

        // ripgrep exits with 1 when no matches found — treat as empty result, not error.
        // Exit code 2 means a real error (bad arguments, I/O failure, etc.).
        if output.status.code() == Some(2) {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            return Err(ToolError::ExecutionFailed(format!("rg error: {stderr}")));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        Ok(ToolResult::ok(call.id.clone(), stdout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup(content: &str) -> (TempDir, SearchTool) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), content).unwrap();
        let tool = SearchTool::new(dir.path(), "rg");
        (dir, tool)
    }

    fn call(query: &str) -> ToolCall {
        ToolCall::new("call_s", "search", format!(r#"{{"query":"{query}"}}"#))
    }

    fn call_with_path(query: &str, path: &str) -> ToolCall {
        ToolCall::new(
            "call_sp",
            "search",
            format!(r#"{{"query":"{query}","path":"{path}"}}"#),
        )
    }

    #[tokio::test]
    async fn returns_matches_for_existing_content() {
        let (_dir, tool) = setup("hello world\nfoo bar\nhello again");
        let result = tool.execute(&call("hello")).await.unwrap();
        let out = result.content.as_str();
        assert!(out.contains("hello world"), "output: {out}");
        assert!(out.contains("hello again"), "output: {out}");
    }

    #[tokio::test]
    async fn returns_empty_string_for_no_matches() {
        let (_dir, tool) = setup("hello world");
        // ripgrep exits 1 with no matches — we return empty string.
        let result = tool.execute(&call("zzz_no_match_zzz")).await.unwrap();
        assert_eq!(result.content.as_str(), "");
    }

    #[tokio::test]
    async fn returns_invalid_arguments_when_query_missing() {
        let (_dir, tool) = setup("content");
        let bad_call = ToolCall::new("cb", "search", r#"{}"#);
        let err = tool.execute(&bad_call).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn returns_invalid_arguments_on_bad_json() {
        let (_dir, tool) = setup("content");
        let bad_call = ToolCall::new("cb2", "search", "not json");
        let err = tool.execute(&bad_call).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn optional_path_argument_narrows_search() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/match.txt"), "target line").unwrap();
        std::fs::write(dir.path().join("other.txt"), "other content").unwrap();

        let tool = SearchTool::new(dir.path(), "rg");
        let result = tool.execute(&call_with_path("target", "sub")).await.unwrap();
        let out = result.content.as_str();
        assert!(out.contains("target line"), "output: {out}");
    }

    #[tokio::test]
    async fn missing_rg_binary_returns_execution_failed() {
        let dir = tempfile::tempdir().unwrap();
        let tool = SearchTool::new(dir.path(), "/nonexistent/rg_binary_xyz");
        let c = call("anything");
        let err = tool.execute(&c).await.unwrap_err();
        assert!(matches!(err, ToolError::ExecutionFailed(_)), "got: {err:?}");
    }
}
