use std::sync::Arc;

use domain::entities::conversation::{ConversationRequest, ConversationResponse};
use domain::entities::message::Message;
use domain::entities::tool::{ToolError, ToolResult};
use domain::ports::llm_backend::{BackendError, LlmBackend};
use domain::ports::tool_runtime::ToolRuntime;
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;

/// Hard limit on LLM→tool→LLM round trips per request.
/// On the final pass, a tool-call response is returned as-is (graceful degradation).
const MAX_PASSES: usize = 5;

#[derive(Debug, thiserror::Error)]
pub enum ToolLoopError {
    #[error("backend: {0}")]
    Backend(#[from] BackendError),
}

pub struct ToolLoopOrchestrator {
    backend: Arc<dyn LlmBackend>,
    tools: Vec<Arc<dyn ToolRuntime>>,
}

impl ToolLoopOrchestrator {
    pub fn new(backend: Arc<dyn LlmBackend>, tools: Vec<Arc<dyn ToolRuntime>>) -> Self {
        Self { backend, tools }
    }

    pub async fn run(&self, mut request: ConversationRequest) -> Result<ConversationResponse, ToolLoopError> {
        let mut last_response = None;

        for _ in 0..MAX_PASSES {
            let response = self.backend.complete(request.clone()).await?;

            if !response.is_tool_call() {
                return Ok(response);
            }

            let tool_calls = response.tool_calls.clone().unwrap_or_default();

            // Dispatch all tool calls concurrently; unrecognized tools produce error results
            // rather than aborting — the model sees every result and can recover.
            let mut dispatches = FuturesUnordered::new();
            for call in &tool_calls {
                let runtime = self.tools.iter().find(|t| t.name() == call.function.name).cloned();
                let call = call.clone();
                dispatches.push(async move {
                    match runtime {
                        Some(rt) => match rt.execute(&call).await {
                            Ok(result) => result,
                            Err(e) => ToolResult::err(call.id.clone(), e),
                        },
                        None => ToolResult::err(
                            call.id.clone(),
                            ToolError::NotFound(call.function.name.clone()),
                        ),
                    }
                });
            }

            // Collect results in completion order, then sort by tool_call order to
            // guarantee deterministic message ordering in tests.
            let mut results: Vec<ToolResult> = Vec::with_capacity(tool_calls.len());
            while let Some(result) = dispatches.next().await {
                results.push(result);
            }
            results.sort_by_key(|r| {
                tool_calls.iter().position(|c| c.id == r.tool_call_id).unwrap_or(usize::MAX)
            });

            request.messages.push(Message::assistant_with_tool_calls(tool_calls));
            for result in results {
                request.messages.push(Message::tool_result(
                    result.tool_call_id,
                    result.content.as_str(),
                ));
            }

            last_response = Some(response);
        }

        // MAX_PASSES exhausted with the model still requesting tools — return the last
        // response rather than erroring; callers can inspect tool_calls to detect this.
        Ok(last_response.expect("loop ran at least once"))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use domain::entities::conversation::{ConversationRequest, ConversationResponse};
    use domain::entities::message::{Message, Role};
    use domain::entities::model::{ModelDescriptor, ModelId};
    use domain::entities::stream_chunk::StreamChunk;
    use domain::entities::tool::{ToolCall, ToolError, ToolResult};
    use domain::ports::llm_backend::{BackendError, BackendStream, LlmBackend};
    use domain::ports::tool_runtime::ToolRuntime;
    use uuid::Uuid;

    use super::ToolLoopOrchestrator;

    // --- Fakes ---

    struct FakeBackend {
        /// Pre-programmed responses returned in order; last entry is repeated when exhausted.
        responses: Mutex<Vec<ConversationResponse>>,
    }

    impl FakeBackend {
        fn new(responses: Vec<ConversationResponse>) -> Self {
            Self { responses: Mutex::new(responses) }
        }
    }

    #[async_trait]
    impl LlmBackend for FakeBackend {
        async fn complete(&self, _req: ConversationRequest) -> Result<ConversationResponse, BackendError> {
            let mut q = self.responses.lock().unwrap();
            if q.len() > 1 {
                Ok(q.remove(0))
            } else {
                Ok(q[0].clone())
            }
        }

        async fn stream(&self, _req: ConversationRequest) -> Result<BackendStream, BackendError> {
            Ok(Box::pin(futures_util::stream::empty::<Result<StreamChunk, BackendError>>()))
        }

        async fn list_models(&self) -> Result<Vec<ModelDescriptor>, BackendError> {
            Ok(vec![])
        }

        async fn health_check(&self) -> Result<(), BackendError> {
            Ok(())
        }
    }

    struct FakeTool {
        tool_name: &'static str,
        output: &'static str,
    }

    #[async_trait]
    impl ToolRuntime for FakeTool {
        fn name(&self) -> &str {
            self.tool_name
        }

        async fn execute(&self, call: &ToolCall) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::ok(call.id.clone(), self.output))
        }
    }

    fn plain_response(content: &str) -> ConversationResponse {
        ConversationResponse {
            id: Uuid::new_v4(),
            model: ModelId::new("test"),
            content: content.to_owned(),
            prompt_tokens: 0,
            completion_tokens: 0,
            tool_calls: None,
        }
    }

    fn tool_call_response(calls: Vec<ToolCall>) -> ConversationResponse {
        ConversationResponse {
            id: Uuid::new_v4(),
            model: ModelId::new("test"),
            content: String::new(),
            prompt_tokens: 0,
            completion_tokens: 0,
            tool_calls: Some(calls),
        }
    }

    fn base_request() -> ConversationRequest {
        ConversationRequest::new(ModelId::new("test"), vec![Message::user("hello")])
    }

    // --- Tests ---

    #[tokio::test]
    async fn no_tool_calls_returns_immediately() {
        let backend = Arc::new(FakeBackend::new(vec![plain_response("done")]));
        let orch = ToolLoopOrchestrator::new(backend, vec![]);
        let resp = orch.run(base_request()).await.unwrap();
        assert_eq!(resp.content, "done");
        assert!(!resp.is_tool_call());
    }

    #[tokio::test]
    async fn single_tool_call_one_pass() {
        let call = ToolCall::new("c1", "read_file", r#"{"path":"a.rs"}"#);
        let backend = Arc::new(FakeBackend::new(vec![
            tool_call_response(vec![call.clone()]),
            plain_response("file was read"),
        ]));
        let tool: Arc<dyn ToolRuntime> =
            Arc::new(FakeTool { tool_name: "read_file", output: "contents of a.rs" });
        let orch = ToolLoopOrchestrator::new(backend, vec![tool]);

        let resp = orch.run(base_request()).await.unwrap();
        assert_eq!(resp.content, "file was read");
    }

    #[tokio::test]
    async fn max_passes_stops_loop() {
        let call = ToolCall::new("c1", "read_file", "{}");
        let backend = Arc::new(FakeBackend::new(vec![
            // Only one entry — FakeBackend repeats it forever
            tool_call_response(vec![call]),
        ]));
        let tool: Arc<dyn ToolRuntime> =
            Arc::new(FakeTool { tool_name: "read_file", output: "ok" });
        let orch = ToolLoopOrchestrator::new(backend, vec![tool]);

        // Must not error or hang — returns last tool-call response after MAX_PASSES
        let resp = orch.run(base_request()).await.unwrap();
        assert!(resp.is_tool_call());
    }

    #[tokio::test]
    async fn multiple_tool_calls_dispatched_concurrently() {
        let calls = vec![
            ToolCall::new("c1", "read_file", r#"{"path":"a.rs"}"#),
            ToolCall::new("c2", "search", r#"{"query":"fn main"}"#),
        ];
        let backend = Arc::new(FakeBackend::new(vec![
            tool_call_response(calls),
            plain_response("both done"),
        ]));
        let tools: Vec<Arc<dyn ToolRuntime>> = vec![
            Arc::new(FakeTool { tool_name: "read_file", output: "file contents" }),
            Arc::new(FakeTool { tool_name: "search", output: "search results" }),
        ];
        let orch = ToolLoopOrchestrator::new(backend, tools);

        let resp = orch.run(base_request()).await.unwrap();
        assert_eq!(resp.content, "both done");
    }

    #[tokio::test]
    async fn unknown_tool_becomes_error_result() {
        let call = ToolCall::new("c1", "nonexistent_tool", "{}");
        let backend = Arc::new(FakeBackend::new(vec![
            tool_call_response(vec![call]),
            plain_response("recovered"),
        ]));
        // No tools registered — every call is unknown
        let orch = ToolLoopOrchestrator::new(backend, vec![]);

        let resp = orch.run(base_request()).await.unwrap();
        // The loop injected an error ToolResult and resubmitted; backend returned "recovered"
        assert_eq!(resp.content, "recovered");
    }

    #[tokio::test]
    async fn tool_result_messages_appended_in_order() {

        // Capture the second request's messages to inspect ordering.
        use std::sync::atomic::{AtomicBool, Ordering};
        struct CapturingBackend {
            first_call: AtomicBool,
            captured: Mutex<Vec<Message>>,
        }

        #[async_trait]
        impl LlmBackend for CapturingBackend {
            async fn complete(&self, req: ConversationRequest) -> Result<ConversationResponse, BackendError> {
                if self.first_call.swap(false, Ordering::SeqCst) {
                    Ok(tool_call_response(vec![
                        ToolCall::new("c1", "read_file", "{}"),
                        ToolCall::new("c2", "search", "{}"),
                    ]))
                } else {
                    *self.captured.lock().unwrap() = req.messages.clone();
                    Ok(plain_response("done"))
                }
            }

            async fn stream(&self, _req: ConversationRequest) -> Result<BackendStream, BackendError> {
                Ok(Box::pin(futures_util::stream::empty::<Result<StreamChunk, BackendError>>()))
            }

            async fn list_models(&self) -> Result<Vec<ModelDescriptor>, BackendError> {
                Ok(vec![])
            }

            async fn health_check(&self) -> Result<(), BackendError> {
                Ok(())
            }
        }

        let capturing = Arc::new(CapturingBackend {
            first_call: AtomicBool::new(true),
            captured: Mutex::new(vec![]),
        });

        let tools: Vec<Arc<dyn ToolRuntime>> = vec![
            Arc::new(FakeTool { tool_name: "read_file", output: "file" }),
            Arc::new(FakeTool { tool_name: "search", output: "results" }),
        ];
        let orch = ToolLoopOrchestrator::new(capturing.clone(), tools);
        let _ = orch.run(base_request()).await.unwrap();

        let msgs = capturing.captured.lock().unwrap().clone();
        // Expected layout: [user] [assistant w/ tool_calls] [tool c1] [tool c2]
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, Role::User);
        assert_eq!(msgs[1].role, Role::Assistant);
        assert!(msgs[1].tool_calls.is_some());
        assert_eq!(msgs[2].role, Role::Tool);
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(msgs[3].role, Role::Tool);
        assert_eq!(msgs[3].tool_call_id.as_deref(), Some("c2"));
    }
}
