use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};
use tokio::fs;

use domain::entities::tool::{ToolCall, ToolDefinition, ToolError, ToolResult};
use domain::ports::tool_runtime::ToolRuntime;

// ── FindTool ─────────────────────────────────────────────────────────────────

/// Recursively searches for files/directories by name glob within the workspace.
///
/// Arguments: `path` (optional sub-path, defaults to root), `pattern` (glob, e.g. `*.rs`),
/// `max_depth` (optional, default 10).
pub struct FindTool {
    workspace_root: PathBuf,
}

impl FindTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

#[async_trait]
impl ToolRuntime for FindTool {
    fn name(&self) -> &str {
        "find"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(
            "find",
            "Recursively find files or directories matching a glob pattern inside the workspace.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match filenames, e.g. \"*.rs\" or \"Cargo.toml\""
                    },
                    "path": {
                        "type": "string",
                        "description": "Optional sub-directory to search within, e.g. \"src\""
                    },
                    "max_depth": {
                        "type": "integer",
                        "description": "Maximum recursion depth (default 10)"
                    }
                },
                "required": ["pattern"]
            }),
        )
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, ToolError> {
        let args = call
            .function
            .parse_arguments()
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("missing 'pattern' field".into()))?
            .to_owned();

        let sub_path = args["path"].as_str().unwrap_or(".");
        let max_depth = args["max_depth"].as_u64().unwrap_or(10) as usize;

        let root = fs::canonicalize(&self.workspace_root)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("workspace root invalid: {e}")))?;

        let search_dir = if Path::new(sub_path).is_absolute() {
            PathBuf::from(sub_path)
        } else {
            root.join(sub_path)
        };

        let resolved_dir = fs::canonicalize(&search_dir)
            .await
            .map_err(|_| ToolError::NotFound(format!("path '{sub_path}' does not exist")))?;

        if !resolved_dir.starts_with(&root) {
            return Err(ToolError::Unauthorized(format!(
                "path '{sub_path}' is outside workspace root"
            )));
        }

        // Build glob matcher from the pattern.
        let glob = glob_match_fn(&pattern);

        let matches = walk_dir(&resolved_dir, &root, max_depth, 0, &glob).await?;

        if matches.is_empty() {
            Ok(ToolResult::ok(call.id.clone(), "(no matches)".to_owned()))
        } else {
            Ok(ToolResult::ok(call.id.clone(), matches.join("\n")))
        }
    }
}

// ── ListTool ──────────────────────────────────────────────────────────────────

/// Lists the immediate contents of a directory within the workspace.
///
/// Arguments: `path` (optional, defaults to workspace root).
/// Returns one entry per line: `[dir]  name` or `[file] name`.
pub struct ListTool {
    workspace_root: PathBuf,
}

impl ListTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

#[async_trait]
impl ToolRuntime for ListTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(
            "list_dir",
            "List the immediate contents of a directory in the workspace. Shows files and sub-directories.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the directory, e.g. \"src\" or \".\" for root"
                    }
                },
                "required": []
            }),
        )
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, ToolError> {
        let args = call
            .function
            .parse_arguments()
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let sub_path = args["path"].as_str().unwrap_or(".");

        let root = fs::canonicalize(&self.workspace_root)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("workspace root invalid: {e}")))?;

        let target = if Path::new(sub_path).is_absolute() {
            PathBuf::from(sub_path)
        } else {
            root.join(sub_path)
        };

        let resolved = fs::canonicalize(&target)
            .await
            .map_err(|_| ToolError::NotFound(format!("path '{sub_path}' does not exist")))?;

        if !resolved.starts_with(&root) {
            return Err(ToolError::Unauthorized(format!(
                "path '{sub_path}' is outside workspace root"
            )));
        }

        let meta = fs::metadata(&resolved)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("stat failed: {e}")))?;

        if !meta.is_dir() {
            return Err(ToolError::InvalidArguments(format!(
                "'{sub_path}' is a file, not a directory — use file_read to read it"
            )));
        }

        let mut entries = Vec::new();
        let mut read_dir = fs::read_dir(&resolved)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("read_dir failed: {e}")))?;

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("read_dir entry failed: {e}")))?
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            let kind = if is_dir { "[dir] " } else { "[file]" };
            entries.push(format!("{kind} {name}"));
        }

        entries.sort();

        if entries.is_empty() {
            Ok(ToolResult::ok(
                call.id.clone(),
                "(empty directory)".to_owned(),
            ))
        } else {
            Ok(ToolResult::ok(call.id.clone(), entries.join("\n")))
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns a closure that tests a filename against a simple glob pattern.
/// Supports `*` (any chars) and `?` (single char). Case-sensitive.
fn glob_match_fn(pattern: &str) -> impl Fn(&str) -> bool + '_ {
    move |name: &str| glob_matches(pattern, name)
}

fn glob_matches(pattern: &str, name: &str) -> bool {
    // Simple recursive glob: * matches any sequence, ? matches one char.
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    glob_recurse(&p, &n, 0, 0)
}

fn glob_recurse(p: &[char], n: &[char], pi: usize, ni: usize) -> bool {
    if pi == p.len() {
        return ni == n.len();
    }
    if p[pi] == '*' {
        // Try matching zero or more characters.
        for skip in ni..=n.len() {
            if glob_recurse(p, n, pi + 1, skip) {
                return true;
            }
        }
        return false;
    }
    if ni == n.len() {
        return false;
    }
    if p[pi] == '?' || p[pi] == n[ni] {
        return glob_recurse(p, n, pi + 1, ni + 1);
    }
    false
}

/// Walks `dir` recursively, collecting relative paths (from `root`) whose
/// file names match `glob`. Stops at `max_depth`.
async fn walk_dir(
    dir: &Path,
    root: &Path,
    max_depth: usize,
    depth: usize,
    glob: &impl Fn(&str) -> bool,
) -> Result<Vec<String>, ToolError> {
    let mut results = Vec::new();

    if depth > max_depth {
        return Ok(results);
    }

    let mut read_dir = fs::read_dir(dir)
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("read_dir failed: {e}")))?;

    while let Some(entry) = read_dir
        .next_entry()
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("read_dir entry: {e}")))?
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let entry_path = entry.path();

        let file_type = entry
            .file_type()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("file_type: {e}")))?;

        if glob(&name) {
            let rel = entry_path
                .strip_prefix(root)
                .unwrap_or(&entry_path)
                .to_string_lossy()
                .into_owned();
            results.push(rel);
        }

        if file_type.is_dir() {
            let mut sub = Box::pin(walk_dir(&entry_path, root, max_depth, depth + 1, glob)).await?;
            results.append(&mut sub);
        }
    }

    Ok(results)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("lib.rs"), "pub fn lib() {}").unwrap();
        std::fs::write(root.join("README.md"), "# readme").unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src").join("helper.rs"), "// helper").unwrap();
        std::fs::create_dir(root.join("tests")).unwrap();
        std::fs::write(root.join("tests").join("integration.rs"), "// test").unwrap();
        dir
    }

    // ── FindTool tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn find_matches_glob_pattern() {
        let dir = setup();
        let tool = FindTool::new(dir.path());
        let call = ToolCall::new("c1", "find", r#"{"pattern":"*.rs"}"#);
        let result = tool.execute(&call).await.unwrap();
        let output = result.content.as_str();
        assert!(output.contains("main.rs"), "got: {output}");
        assert!(output.contains("lib.rs"), "got: {output}");
        assert!(output.contains("helper.rs"), "got: {output}");
        assert!(!output.contains("README.md"), "got: {output}");
    }

    #[tokio::test]
    async fn find_scoped_to_sub_path() {
        let dir = setup();
        let tool = FindTool::new(dir.path());
        let call = ToolCall::new("c2", "find", r#"{"path":"src","pattern":"*.rs"}"#);
        let result = tool.execute(&call).await.unwrap();
        let output = result.content.as_str();
        assert!(output.contains("helper.rs"), "got: {output}");
        assert!(!output.contains("main.rs"), "got: {output}");
    }

    #[tokio::test]
    async fn find_no_matches_returns_no_matches() {
        let dir = setup();
        let tool = FindTool::new(dir.path());
        let call = ToolCall::new("c3", "find", r#"{"pattern":"*.py"}"#);
        let result = tool.execute(&call).await.unwrap();
        assert_eq!(result.content.as_str(), "(no matches)");
    }

    #[tokio::test]
    async fn find_rejects_path_traversal() {
        let dir = setup();
        let tool = FindTool::new(dir.path());
        let call = ToolCall::new("c4", "find", r#"{"path":"../","pattern":"*"}"#);
        let err = tool.execute(&call).await.unwrap_err();
        assert!(matches!(err, ToolError::Unauthorized(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn find_missing_pattern_returns_invalid_arguments() {
        let dir = setup();
        let tool = FindTool::new(dir.path());
        let call = ToolCall::new("c5", "find", r#"{"path":"."}"#);
        let err = tool.execute(&call).await.unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidArguments(_)),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn find_respects_max_depth() {
        let dir = setup();
        let tool = FindTool::new(dir.path());
        // max_depth=0 — only root-level files matched, no recursion into subdirs.
        let call = ToolCall::new("c6", "find", r#"{"pattern":"*.rs","max_depth":0}"#);
        let result = tool.execute(&call).await.unwrap();
        let output = result.content.as_str();
        assert!(output.contains("main.rs"), "got: {output}");
        assert!(!output.contains("helper.rs"), "got: {output}");
    }

    // ── ListTool tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_root_shows_entries() {
        let dir = setup();
        let tool = ListTool::new(dir.path());
        let call = ToolCall::new("l1", "list_dir", r#"{}"#);
        let result = tool.execute(&call).await.unwrap();
        let output = result.content.as_str();
        assert!(output.contains("[file] main.rs"), "got: {output}");
        assert!(output.contains("[file] README.md"), "got: {output}");
        assert!(output.contains("[dir]  src"), "got: {output}");
        assert!(output.contains("[dir]  tests"), "got: {output}");
    }

    #[tokio::test]
    async fn list_sub_directory() {
        let dir = setup();
        let tool = ListTool::new(dir.path());
        let call = ToolCall::new("l2", "list_dir", r#"{"path":"src"}"#);
        let result = tool.execute(&call).await.unwrap();
        let output = result.content.as_str();
        assert!(output.contains("[file] helper.rs"), "got: {output}");
        assert!(!output.contains("main.rs"), "got: {output}");
    }

    #[tokio::test]
    async fn list_rejects_path_traversal() {
        let dir = setup();
        let tool = ListTool::new(dir.path());
        let call = ToolCall::new("l3", "list_dir", r#"{"path":".."}"#);
        let err = tool.execute(&call).await.unwrap_err();
        assert!(matches!(err, ToolError::Unauthorized(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn list_file_returns_invalid_arguments() {
        let dir = setup();
        let tool = ListTool::new(dir.path());
        let call = ToolCall::new("l4", "list_dir", r#"{"path":"main.rs"}"#);
        let err = tool.execute(&call).await.unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidArguments(_)),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn list_missing_path_returns_not_found() {
        let dir = setup();
        let tool = ListTool::new(dir.path());
        let call = ToolCall::new("l5", "list_dir", r#"{"path":"nonexistent"}"#);
        let err = tool.execute(&call).await.unwrap_err();
        assert!(matches!(err, ToolError::NotFound(_)), "got: {err:?}");
    }

    // ── glob_matches unit tests ───────────────────────────────────────────────

    #[test]
    fn glob_star_matches_any() {
        assert!(glob_matches("*.rs", "main.rs"));
        assert!(glob_matches("*.rs", "lib.rs"));
        assert!(!glob_matches("*.rs", "main.py"));
    }

    #[test]
    fn glob_question_matches_one_char() {
        assert!(glob_matches("?.rs", "a.rs"));
        assert!(!glob_matches("?.rs", "ab.rs"));
    }

    #[test]
    fn glob_exact_match() {
        assert!(glob_matches("main.rs", "main.rs"));
        assert!(!glob_matches("main.rs", "main.py"));
    }

    #[test]
    fn glob_star_only_matches_everything() {
        assert!(glob_matches("*", "anything"));
        assert!(glob_matches("*", ""));
    }
}
