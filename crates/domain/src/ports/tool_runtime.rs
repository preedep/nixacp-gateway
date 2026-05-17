use async_trait::async_trait;

use crate::entities::tool::{ToolCall, ToolError, ToolResult};

/// Executes a normalized tool call and returns its result.
///
/// Each registered tool implements this trait. The executor picks the right
/// implementation by matching `ToolCall.function.name` against the registry.
#[async_trait]
pub trait ToolRuntime: Send + Sync {
    /// The tool name this runtime handles (e.g. `"file_read"`, `"search"`).
    fn name(&self) -> &str;

    /// Execute the call. Must not block the async executor — use
    /// `tokio::task::spawn_blocking` for any I/O or CPU-intensive work.
    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, ToolError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::tool::{ToolCall, ToolResult};

    struct EchoTool;

    #[async_trait]
    impl ToolRuntime for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        async fn execute(&self, call: &ToolCall) -> Result<ToolResult, ToolError> {
            let args = call.function.parse_arguments()
                .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
            let msg = args["message"].as_str().unwrap_or("").to_owned();
            Ok(ToolResult::ok(call.id.clone(), msg))
        }
    }

    #[tokio::test]
    async fn echo_tool_returns_message() {
        let tool = EchoTool;
        assert_eq!(tool.name(), "echo");
        let call = ToolCall::new("call_1", "echo", r#"{"message":"hello"}"#);
        let result = tool.execute(&call).await.unwrap();
        assert_eq!(result.tool_call_id, "call_1");
        assert_eq!(result.content.as_str(), "hello");
    }

    #[tokio::test]
    async fn echo_tool_invalid_arguments() {
        let tool = EchoTool;
        let call = ToolCall::new("call_2", "echo", "not json");
        let err = tool.execute(&call).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }
}
