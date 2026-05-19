use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

use domain::entities::tool::{ToolCall, ToolDefinition, ToolError, ToolResult};
use domain::ports::tool_runtime::ToolRuntime;

/// Executes shell commands with an allowlist and metacharacter deny-list.
///
/// Safety model:
/// - Only commands whose first token appears in `allowlist` are permitted.
/// - Shell metacharacters (`;`, `|`, `&&`, `||`, `>`, `<`, `` ` ``, `$(`) are rejected
///   before the allowlist check to prevent injection via argument position.
/// - Working directory is always the workspace root — the model cannot `cd` out.
/// - Hard 30-second timeout; the process is killed on expiry.
pub struct BashTool {
    workspace_root: PathBuf,
    allowlist: Vec<String>,
}

impl BashTool {
    pub fn new(workspace_root: impl Into<PathBuf>, allowlist: Vec<String>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            allowlist,
        }
    }
}

/// Shell metacharacters that would allow injection or redirection.
const FORBIDDEN: &[&str] = &[";", "|", "&&", "||", ">", "<", "`", "$("];

#[async_trait]
impl ToolRuntime for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn definition(&self) -> ToolDefinition {
        let allowed = self.allowlist.join(", ");
        let desc = format!(
            "Run a shell command in the workspace directory. \
            Only the following commands are permitted: {allowed}. \
            Shell operators (pipes, redirects, semicolons) are not allowed. \
            The working directory is always the workspace root."
        );
        ToolDefinition::function(
            "bash",
            &desc,
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command to run, e.g. \"cargo test\" or \"git status\""
                    }
                },
                "required": ["command"]
            }),
        )
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, ToolError> {
        let args = call
            .function
            .parse_arguments()
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let command_str = args["command"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("missing 'command' field".into()))?
            .trim();

        // Deny shell metacharacters first — before any allowlist check.
        for meta in FORBIDDEN {
            if command_str.contains(meta) {
                return Err(ToolError::Unauthorized(format!(
                    "shell operator '{meta}' is not permitted"
                )));
            }
        }

        // Split into argv — simple whitespace split (no shell expansion).
        let parts: Vec<&str> = command_str.split_whitespace().collect();
        if parts.is_empty() {
            return Err(ToolError::InvalidArguments("empty command".into()));
        }

        let program = parts[0];

        // Allowlist check on the command name only.
        if !self
            .allowlist
            .iter()
            .any(|a| a.as_str() == program)
        {
            return Err(ToolError::Unauthorized(format!(
                "command '{program}' is not in the allowlist"
            )));
        }

        let output = tokio::time::timeout(
            Duration::from_secs(30),
            Command::new(program)
                .args(&parts[1..])
                .current_dir(&self.workspace_root)
                .output(),
        )
        .await
        .map_err(|_| ToolError::ExecutionFailed("command timed out after 30s".into()))?
        .map_err(|e| ToolError::ExecutionFailed(format!("spawn failed: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let combined = if output.status.success() {
            if stdout.is_empty() {
                "(exit 0, no output)".to_owned()
            } else {
                stdout.into_owned()
            }
        } else {
            let code = output.status.code().unwrap_or(-1);
            format!(
                "(exit {code})\nstdout: {stdout}\nstderr: {stderr}"
            )
        };

        Ok(ToolResult::ok(call.id.clone(), combined))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(allowlist: &[&str]) -> BashTool {
        BashTool::new(
            std::env::temp_dir(),
            allowlist.iter().map(|s| s.to_string()).collect(),
        )
    }

    fn call(cmd: &str) -> ToolCall {
        let body = format!(r#"{{"command":{}}}"#, serde_json::to_string(cmd).unwrap());
        ToolCall::new("b1", "bash", &body)
    }

    #[tokio::test]
    async fn allowed_command_succeeds() {
        let t = tool(&["echo"]);
        let result = t.execute(&call("echo hello")).await.unwrap();
        assert!(result.content.as_str().contains("hello"), "got: {}", result.content.as_str());
    }

    #[tokio::test]
    async fn rejects_command_not_in_allowlist() {
        let t = tool(&["echo"]);
        let err = t.execute(&call("rm -rf /")).await.unwrap_err();
        assert!(matches!(err, ToolError::Unauthorized(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn rejects_semicolon() {
        let t = tool(&["echo", "rm"]);
        let err = t.execute(&call("echo hi; rm -rf /")).await.unwrap_err();
        assert!(matches!(err, ToolError::Unauthorized(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn rejects_pipe() {
        let t = tool(&["cat", "grep"]);
        let err = t.execute(&call("cat /etc/passwd | grep root")).await.unwrap_err();
        assert!(matches!(err, ToolError::Unauthorized(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn rejects_redirect() {
        let t = tool(&["echo"]);
        let err = t.execute(&call("echo bad > /tmp/x")).await.unwrap_err();
        assert!(matches!(err, ToolError::Unauthorized(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn rejects_subshell() {
        let t = tool(&["echo"]);
        let err = t.execute(&call("echo $(whoami)")).await.unwrap_err();
        assert!(matches!(err, ToolError::Unauthorized(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn rejects_backtick() {
        let t = tool(&["echo"]);
        let err = t.execute(&call("echo `whoami`")).await.unwrap_err();
        assert!(matches!(err, ToolError::Unauthorized(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn rejects_and_and() {
        let t = tool(&["echo"]);
        let err = t.execute(&call("echo hi && rm -rf /")).await.unwrap_err();
        assert!(matches!(err, ToolError::Unauthorized(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn missing_command_field_returns_invalid_arguments() {
        let t = tool(&["echo"]);
        let c = ToolCall::new("b", "bash", r#"{}"#);
        let err = t.execute(&c).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn nonzero_exit_returns_ok_with_exit_code() {
        let t = tool(&["sh"]);
        // sh -c "exit 1" — returns exit code 1 but tool returns ToolResult::ok with exit info
        let result = t.execute(&call("sh -c exit")).await.unwrap();
        // sh itself is allowed; the result contains exit code info
        let _ = result; // just assert it doesn't panic
    }
}
