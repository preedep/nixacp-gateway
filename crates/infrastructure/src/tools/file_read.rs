use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};
use tokio::fs;

use domain::entities::tool::{ToolCall, ToolDefinition, ToolError, ToolResult};
use domain::ports::tool_runtime::ToolRuntime;

/// Reads files from within a confined workspace root.
///
/// Both path traversal (`../`) and absolute paths outside the root are rejected
/// via canonicalization — the resolved path must be a descendant of the
/// canonicalized workspace root.
pub struct ReadFileTool {
    workspace_root: PathBuf,
}

impl ReadFileTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

#[async_trait]
impl ToolRuntime for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(
            "read_file",
            "Read the contents of a file inside the workspace. Use relative paths.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file from the workspace root, e.g. \"src/main.rs\""
                    }
                },
                "required": ["path"]
            }),
        )
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, ToolError> {
        let args = call
            .function
            .parse_arguments()
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("missing 'path' field".into()))?;

        let requested = Path::new(path_str);

        // Resolve workspace root; fail fast if root doesn't exist.
        let root = fs::canonicalize(&self.workspace_root)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("workspace root invalid: {e}")))?;

        // Build the candidate path: join root + requested (absolute paths are replaced
        // by the join, so we catch them in the canonicalize step below).
        let candidate = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            root.join(requested)
        };

        // Canonicalize resolves symlinks and `..` segments.
        let resolved = fs::canonicalize(&candidate).await.map_err(|_| {
            // File not found or invalid path — treat as not found.
            ToolError::NotFound(path_str.to_owned())
        })?;

        // Security gate: the resolved path must be inside the workspace root.
        if !resolved.starts_with(&root) {
            return Err(ToolError::Unauthorized(format!(
                "path '{path_str}' is outside workspace root"
            )));
        }

        let contents = fs::read_to_string(&resolved)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("read failed: {e}")))?;

        Ok(ToolResult::ok(call.id.clone(), contents))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_workspace() -> (TempDir, ReadFileTool) {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = ReadFileTool::new(dir.path());
        (dir, tool)
    }

    fn write_file(dir: &TempDir, name: &str, content: &str) {
        let path = dir.path().join(name);
        std::fs::write(path, content).unwrap();
    }

    fn call_with_path(path: &str) -> ToolCall {
        ToolCall::new("call_1", "read_file", format!(r#"{{"path":"{path}"}}"#))
    }

    #[tokio::test]
    async fn reads_file_within_workspace() {
        let (dir, tool) = temp_workspace();
        write_file(&dir, "hello.txt", "hello world");

        let call = call_with_path("hello.txt");
        let result = tool.execute(&call).await.unwrap();
        assert_eq!(result.content.as_str(), "hello world");
        assert_eq!(result.tool_call_id, "call_1");
    }

    #[tokio::test]
    async fn rejects_path_traversal() {
        let (dir, tool) = temp_workspace();
        // Ensure there's a file one level up to canonicalize against.
        let parent = dir.path().parent().unwrap();
        let victim = parent.join("secret.txt");
        std::fs::write(&victim, "secret").unwrap();

        let call = call_with_path("../secret.txt");
        let err = tool.execute(&call).await.unwrap_err();
        assert!(matches!(err, ToolError::Unauthorized(_)), "got: {err:?}");

        let _ = std::fs::remove_file(victim);
    }

    #[tokio::test]
    async fn rejects_absolute_path_outside_root() {
        let (dir, tool) = temp_workspace();
        // /tmp is almost always present and outside any per-test tempdir.
        // We use the parent of the tempdir — guaranteed to exist.
        let outside = dir.path().parent().unwrap().to_str().unwrap().to_owned();
        let call = ToolCall::new(
            "call_abs",
            "read_file",
            format!(r#"{{"path":"{outside}"}}"#),
        );
        let err = tool.execute(&call).await.unwrap_err();
        // Absolute paths that are outside the root hit Unauthorized or NotFound
        // depending on whether they canonicalize; both are acceptable security outcomes.
        assert!(
            matches!(err, ToolError::Unauthorized(_) | ToolError::NotFound(_)),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn returns_not_found_for_missing_file() {
        let (_dir, tool) = temp_workspace();
        let call = call_with_path("does_not_exist.rs");
        let err = tool.execute(&call).await.unwrap_err();
        assert!(matches!(err, ToolError::NotFound(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn returns_invalid_arguments_when_path_missing() {
        let (_dir, tool) = temp_workspace();
        let call = ToolCall::new("call_bad", "read_file", "{}");
        let err = tool.execute(&call).await.unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidArguments(_)),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn returns_invalid_arguments_on_bad_json() {
        let (_dir, tool) = temp_workspace();
        let call = ToolCall::new("call_bad2", "read_file", "not json");
        let err = tool.execute(&call).await.unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidArguments(_)),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn reads_file_in_subdirectory() {
        let (dir, tool) = temp_workspace();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        write_file(&dir, "sub/nested.txt", "nested content");

        let call = call_with_path("sub/nested.txt");
        let result = tool.execute(&call).await.unwrap();
        assert_eq!(result.content.as_str(), "nested content");
    }
}
