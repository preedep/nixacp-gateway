use std::collections::HashMap;
use std::sync::Arc;

use domain::entities::tool::{ToolCall, ToolError, ToolResult};
use domain::ports::tool_runtime::ToolRuntime;

/// Maps tool names to their runtime implementations.
///
/// Uses `Arc` around the map so readers can clone the pointer without locking.
/// Swap via `arc_swap::ArcSwap` when hot-reloading is added (Phase 7+).
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ToolRuntime>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: HashMap::new() }
    }

    pub fn register(&mut self, tool: Arc<dyn ToolRuntime>) {
        self.tools.insert(tool.name().to_owned(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn ToolRuntime>> {
        self.tools.get(name).cloned()
    }

    pub async fn execute(&self, call: &ToolCall) -> Result<ToolResult, ToolError> {
        let tool = self
            .get(&call.function.name)
            .ok_or_else(|| ToolError::NotFound(call.function.name.clone()))?;
        tool.execute(call).await
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct ConstTool {
        name: &'static str,
        output: &'static str,
    }

    #[async_trait]
    impl ToolRuntime for ConstTool {
        fn name(&self) -> &str {
            self.name
        }

        async fn execute(&self, call: &ToolCall) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::ok(call.id.clone(), self.output))
        }
    }

    fn registry_with_echo() -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(ConstTool { name: "echo", output: "pong" }));
        reg
    }

    #[tokio::test]
    async fn registered_tool_is_dispatched() {
        let reg = registry_with_echo();
        let call = ToolCall::new("call_1", "echo", "{}");
        let result = reg.execute(&call).await.unwrap();
        assert_eq!(result.content.as_str(), "pong");
        assert_eq!(result.tool_call_id, "call_1");
    }

    #[tokio::test]
    async fn unknown_tool_returns_not_found() {
        let reg = ToolRegistry::new();
        let call = ToolCall::new("call_x", "missing_tool", "{}");
        let err = reg.execute(&call).await.unwrap_err();
        assert!(matches!(err, ToolError::NotFound(n) if n == "missing_tool"));
    }

    #[test]
    fn get_returns_none_for_unknown_name() {
        let reg = ToolRegistry::new();
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn get_returns_some_for_registered_name() {
        let reg = registry_with_echo();
        assert!(reg.get("echo").is_some());
    }

    #[tokio::test]
    async fn later_registration_overwrites_earlier() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(ConstTool { name: "tool", output: "first" }));
        reg.register(Arc::new(ConstTool { name: "tool", output: "second" }));
        let call = ToolCall::new("id", "tool", "{}");
        let result = reg.execute(&call).await.unwrap();
        assert_eq!(result.content.as_str(), "second");
    }
}
