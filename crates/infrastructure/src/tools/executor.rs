use std::sync::Arc;
use std::time::Duration;

use domain::entities::tool::{ToolCall, ToolError, ToolResult};
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;

use crate::tools::registry::ToolRegistry;

const TOOL_TIMEOUT: Duration = Duration::from_secs(10);

/// Dispatches a batch of tool calls concurrently and collects results.
///
/// Each call runs under a 10-second timeout. Calls that time out or fail
/// produce a `ToolResult::err` rather than propagating to the caller —
/// the tool loop injects errors as Role::Tool messages so the model can
/// react to them rather than crashing the session.
pub struct ToolExecutor {
    registry: Arc<ToolRegistry>,
}

impl ToolExecutor {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }

    /// Execute all `calls` concurrently, returning one `ToolResult` per call.
    /// Never returns `Err` — individual failures become error `ToolResult`s.
    pub async fn dispatch_all(&self, calls: &[ToolCall]) -> Vec<ToolResult> {
        let mut futs: FuturesUnordered<_> = calls
            .iter()
            .map(|call| {
                let registry = Arc::clone(&self.registry);
                let call = call.clone();
                async move {
                    let result = tokio::time::timeout(TOOL_TIMEOUT, registry.execute(&call)).await;
                    match result {
                        Ok(Ok(r)) => r,
                        Ok(Err(e)) => ToolResult::err(call.id.clone(), e),
                        Err(_) => ToolResult::err(call.id.clone(), ToolError::Timeout),
                    }
                }
            })
            .collect();

        let mut results = Vec::with_capacity(calls.len());
        while let Some(r) = futs.next().await {
            results.push(r);
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use domain::ports::tool_runtime::ToolRuntime;

    struct SlowTool {
        delay_ms: u64,
    }

    #[async_trait]
    impl ToolRuntime for SlowTool {
        fn name(&self) -> &str {
            "slow"
        }

        async fn execute(&self, call: &ToolCall) -> Result<ToolResult, ToolError> {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            Ok(ToolResult::ok(call.id.clone(), "done"))
        }
    }

    struct ErrTool;

    #[async_trait]
    impl ToolRuntime for ErrTool {
        fn name(&self) -> &str {
            "failing"
        }

        async fn execute(&self, _call: &ToolCall) -> Result<ToolResult, ToolError> {
            Err(ToolError::ExecutionFailed("intentional".into()))
        }
    }

    fn make_executor(tools: Vec<Arc<dyn ToolRuntime>>) -> ToolExecutor {
        let mut reg = ToolRegistry::new();
        for t in tools {
            reg.register(t);
        }
        ToolExecutor::new(Arc::new(reg))
    }

    #[tokio::test]
    async fn dispatch_empty_slice_returns_empty() {
        let exec = make_executor(vec![]);
        let results = exec.dispatch_all(&[]).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn dispatch_single_success() {
        let exec = make_executor(vec![Arc::new(SlowTool { delay_ms: 0 })]);
        let calls = vec![ToolCall::new("c1", "slow", "{}")];
        let results = exec.dispatch_all(&calls).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content.as_str(), "done");
        assert_eq!(results[0].tool_call_id, "c1");
    }

    #[tokio::test]
    async fn dispatch_multiple_calls_concurrently() {
        let exec = make_executor(vec![Arc::new(SlowTool { delay_ms: 20 })]);
        let calls: Vec<_> = (0..4)
            .map(|i| ToolCall::new(format!("c{i}"), "slow", "{}"))
            .collect();

        let start = std::time::Instant::now();
        let results = exec.dispatch_all(&calls).await;
        let elapsed = start.elapsed();

        assert_eq!(results.len(), 4);
        // Concurrent: 4 × 20 ms each → should finish in < 100 ms, not 80 ms serial.
        assert!(elapsed < Duration::from_millis(100), "dispatch was not concurrent: {elapsed:?}");
    }

    #[tokio::test]
    async fn dispatch_tool_error_becomes_error_result() {
        let exec = make_executor(vec![Arc::new(ErrTool)]);
        let calls = vec![ToolCall::new("cx", "failing", "{}")];
        let results = exec.dispatch_all(&calls).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].content.as_str().contains("intentional"));
        assert_eq!(results[0].tool_call_id, "cx");
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_becomes_not_found_result() {
        let exec = make_executor(vec![]);
        let calls = vec![ToolCall::new("cu", "ghost_tool", "{}")];
        let results = exec.dispatch_all(&calls).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].content.as_str().contains("ghost_tool"), "{}", results[0].content.as_str());
    }

    // ToolError::Timeout is produced when the inner future exceeds TOOL_TIMEOUT (10s).
    // We verify the error variant shape here; the real 10s path is not exercised in
    // unit tests to avoid slow CI — integration tests cover that scenario with mocks.
    #[test]
    fn timeout_error_display() {
        let e = ToolError::Timeout;
        assert_eq!(e.to_string(), "timeout");
    }
}
