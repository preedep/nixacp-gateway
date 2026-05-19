use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};
use tokio::fs;

use domain::entities::tool::{ToolCall, ToolDefinition, ToolError, ToolResult};
use domain::ports::tool_runtime::ToolRuntime;

pub struct PatchFileTool {
    workspace_root: PathBuf,
}

impl PatchFileTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

#[async_trait]
impl ToolRuntime for PatchFileTool {
    fn name(&self) -> &str {
        "patch_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(
            "patch_file",
            "Replace an exact string in a file with new content. Fails if old_str is not found or appears more than once.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file from the workspace root, e.g. \"src/main.rs\""
                    },
                    "old_str": {
                        "type": "string",
                        "description": "The exact string to find and replace. Must appear exactly once in the file."
                    },
                    "new_str": {
                        "type": "string",
                        "description": "The string to substitute in place of old_str"
                    }
                },
                "required": ["path", "old_str", "new_str"]
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

        let old_str = args["old_str"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("missing 'old_str' field".into()))?;

        let new_str = args["new_str"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("missing 'new_str' field".into()))?;

        let root = fs::canonicalize(&self.workspace_root)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("workspace root invalid: {e}")))?;

        let candidate = if Path::new(path_str).is_absolute() {
            PathBuf::from(path_str)
        } else {
            root.join(path_str)
        };

        let resolved = fs::canonicalize(&candidate)
            .await
            .map_err(|_| ToolError::NotFound(path_str.to_owned()))?;

        if !resolved.starts_with(&root) {
            return Err(ToolError::Unauthorized(format!(
                "path '{path_str}' is outside workspace root"
            )));
        }

        let original = fs::read_to_string(&resolved)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("read failed: {e}")))?;

        let count = original.matches(old_str).count();
        if count == 0 {
            return Err(ToolError::ExecutionFailed(format!(
                "old_str not found in '{path_str}'"
            )));
        }
        if count > 1 {
            return Err(ToolError::ExecutionFailed(format!(
                "old_str appears {count} times in '{path_str}' — must be unique"
            )));
        }

        let patched = original.replacen(old_str, new_str, 1);

        fs::write(&resolved, &patched)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("write failed: {e}")))?;

        Ok(ToolResult::ok(
            call.id.clone(),
            format!("patched '{path_str}'"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup(content: &str) -> (TempDir, PatchFileTool, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("target.txt");
        std::fs::write(&path, content).unwrap();
        let tool = PatchFileTool::new(dir.path());
        (dir, tool, "target.txt".to_owned())
    }

    fn call(path: &str, old_str: &str, new_str: &str) -> ToolCall {
        ToolCall::new(
            "p1",
            "patch_file",
            serde_json::to_string(&json!({
                "path": path,
                "old_str": old_str,
                "new_str": new_str
            }))
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn replaces_unique_match() {
        let (dir, tool, filename) = setup("fn old_name() {}");
        tool.execute(&call(&filename, "old_name", "new_name"))
            .await
            .unwrap();
        let result = std::fs::read_to_string(dir.path().join(&filename)).unwrap();
        assert_eq!(result, "fn new_name() {}");
    }

    #[tokio::test]
    async fn replaces_multiline_match() {
        let (dir, tool, filename) = setup("line1\nold block\nline3");
        tool.execute(&call(&filename, "old block", "new block"))
            .await
            .unwrap();
        let result = std::fs::read_to_string(dir.path().join(&filename)).unwrap();
        assert_eq!(result, "line1\nnew block\nline3");
    }

    #[tokio::test]
    async fn errors_when_old_str_not_found() {
        let (_dir, tool, filename) = setup("some content");
        let err = tool
            .execute(&call(&filename, "nonexistent", "replacement"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ToolError::ExecutionFailed(_)),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn errors_when_old_str_ambiguous() {
        let (_dir, tool, filename) = setup("foo foo foo");
        let err = tool
            .execute(&call(&filename, "foo", "bar"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ToolError::ExecutionFailed(_)),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn rejects_path_traversal() {
        let (_dir, tool, _) = setup("x");
        let err = tool
            .execute(&call("../escape.txt", "x", "y"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ToolError::NotFound(_) | ToolError::Unauthorized(_)),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn returns_not_found_for_missing_file() {
        let (_dir, tool, _) = setup("x");
        let err = tool
            .execute(&call("does_not_exist.txt", "x", "y"))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::NotFound(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn returns_invalid_arguments_when_fields_missing() {
        let (_dir, tool, _) = setup("x");
        let c = ToolCall::new("p", "patch_file", r#"{"path":"f.txt"}"#);
        let err = tool.execute(&c).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "got: {err:?}");
    }
}
