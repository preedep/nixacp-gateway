use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};
use tokio::fs;

use domain::entities::tool::{ToolCall, ToolDefinition, ToolError, ToolResult};
use domain::ports::tool_runtime::ToolRuntime;

pub struct WriteFileTool {
    workspace_root: PathBuf,
}

impl WriteFileTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

#[async_trait]
impl ToolRuntime for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(
            "write_file",
            "Create or overwrite a file inside the workspace with the given content.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file from the workspace root, e.g. \"src/main.rs\""
                    },
                    "content": {
                        "type": "string",
                        "description": "The full content to write to the file"
                    }
                },
                "required": ["path", "content"]
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

        let content = args["content"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("missing 'content' field".into()))?;

        let root = fs::canonicalize(&self.workspace_root)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("workspace root invalid: {e}")))?;

        let candidate = if Path::new(path_str).is_absolute() {
            PathBuf::from(path_str)
        } else {
            root.join(path_str)
        };

        // Resolve parent to check confinement — the file itself may not exist yet.
        let parent = candidate
            .parent()
            .ok_or_else(|| ToolError::InvalidArguments("path has no parent directory".into()))?;

        // Create parent dirs if they don't exist, then check confinement.
        fs::create_dir_all(parent)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("create_dir_all failed: {e}")))?;

        let resolved_parent = fs::canonicalize(parent)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("cannot resolve parent: {e}")))?;

        if !resolved_parent.starts_with(&root) {
            return Err(ToolError::Unauthorized(format!(
                "path '{path_str}' is outside workspace root"
            )));
        }

        let target = resolved_parent.join(
            candidate
                .file_name()
                .ok_or_else(|| ToolError::InvalidArguments("path has no filename".into()))?,
        );

        fs::write(&target, content)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("write failed: {e}")))?;

        Ok(ToolResult::ok(
            call.id.clone(),
            format!("wrote {} bytes to {path_str}", content.len()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, WriteFileTool) {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = WriteFileTool::new(dir.path());
        (dir, tool)
    }

    fn call(path: &str, content: &str) -> ToolCall {
        ToolCall::new(
            "w1",
            "write_file",
            serde_json::to_string(&json!({"path": path, "content": content})).unwrap(),
        )
    }

    #[tokio::test]
    async fn creates_new_file() {
        let (dir, tool) = setup();
        let result = tool.execute(&call("hello.txt", "hello world")).await.unwrap();
        assert!(result.content.as_str().contains("wrote"));
        let written = std::fs::read_to_string(dir.path().join("hello.txt")).unwrap();
        assert_eq!(written, "hello world");
    }

    #[tokio::test]
    async fn overwrites_existing_file() {
        let (dir, tool) = setup();
        std::fs::write(dir.path().join("a.txt"), "old").unwrap();
        tool.execute(&call("a.txt", "new content")).await.unwrap();
        let written = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();
        assert_eq!(written, "new content");
    }

    #[tokio::test]
    async fn creates_parent_directories() {
        let (dir, tool) = setup();
        tool.execute(&call("sub/dir/file.txt", "nested")).await.unwrap();
        let written = std::fs::read_to_string(dir.path().join("sub/dir/file.txt")).unwrap();
        assert_eq!(written, "nested");
    }

    #[tokio::test]
    async fn rejects_path_traversal() {
        let (dir, tool) = setup();
        // Ensure parent exists so canonicalize can resolve it.
        let parent = dir.path().parent().unwrap().to_str().unwrap().to_owned();
        let traversal_path = "../escape.txt";
        let err = tool.execute(&call(traversal_path, "evil")).await.unwrap_err();
        assert!(
            matches!(err, ToolError::Unauthorized(_)),
            "got: {err:?} (parent={parent})"
        );
    }

    #[tokio::test]
    async fn returns_invalid_arguments_when_path_missing() {
        let (_dir, tool) = setup();
        let c = ToolCall::new("w", "write_file", r#"{"content":"x"}"#);
        let err = tool.execute(&c).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn returns_invalid_arguments_when_content_missing() {
        let (_dir, tool) = setup();
        let c = ToolCall::new("w", "write_file", r#"{"path":"a.txt"}"#);
        let err = tool.execute(&c).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "got: {err:?}");
    }
}
