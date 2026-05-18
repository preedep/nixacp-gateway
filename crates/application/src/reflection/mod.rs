use std::sync::Arc;
use std::time::Duration;

use domain::entities::conversation::{ConversationRequest, ConversationResponse};
use domain::entities::message::Message;
use domain::ports::llm_backend::{BackendError, LlmBackend};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

/// Maximum reflection passes (initial attempt + retries).
const MAX_ATTEMPTS: usize = 3;

/// Timeout for a single backend call inside the reflection loop.
const BACKEND_TIMEOUT: Duration = Duration::from_secs(120);

/// Maximum concurrent reflection sessions. Sessions that cannot acquire the permit
/// immediately skip reflection and return the direct backend response.
const MAX_CONCURRENT_SESSIONS: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum ReflectionError {
    #[error("backend: {0}")]
    Backend(#[from] BackendError),
    #[error("cancelled")]
    Cancelled,
}

/// Checks whether a response needs to be reflected on (i.e., is incomplete/malformed).
///
/// Returns `Some(reason)` when the response should be retried; `None` when it is acceptable.
fn quality_issue(response: &ConversationResponse) -> Option<&'static str> {
    let text = response.content.trim();

    // Empty non-tool response is suspicious.
    if text.is_empty() && !response.is_tool_call() {
        return Some("empty response");
    }

    // Truncated code block: opened a fenced block but never closed it.
    let fence_opens = text.matches("```").count();
    if !fence_opens.is_multiple_of(2) {
        return Some("truncated code block");
    }

    // Bare malformed JSON tool call that slipped past the normalizer.
    if text.starts_with('{') && text.ends_with("...") {
        return Some("truncated json tool call");
    }

    // Response ran out of tokens mid-heading (e.g. ends with "###").
    if text.ends_with("###") || text.ends_with("##") || text.ends_with('#') {
        return Some("truncated heading");
    }

    // Prose cut off mid-sentence — no terminal punctuation or code boundary.
    // Only applies outside code blocks (an even fence count means we're in prose).
    let fence_opens = text.matches("```").count();
    if fence_opens.is_multiple_of(2) {
        let last_char = text.chars().last().unwrap_or(' ');
        if !matches!(
            last_char,
            '.' | '!' | '?' | '`' | '}' | ')' | ';' | '"' | ':'
        ) {
            return Some("truncated sentence");
        }
    }

    None
}

/// Builds the reflection prompt injected between the bad response and the retry.
fn reflection_prompt(reason: &str) -> String {
    format!(
        "Your previous response was incomplete or malformed ({reason}). \
         Please provide a complete, correct response. Do not repeat the earlier \
         incomplete content — start fresh."
    )
}

/// Orchestrates reflection: calls the backend, checks quality, and retries with a
/// reflection prompt up to MAX_ATTEMPTS times. Sequential passes only — one
/// ConversationRequest lives in memory at a time.
///
/// Concurrent sessions are limited by an internal semaphore. Sessions that cannot
/// acquire a permit immediately skip reflection and return the direct response.
pub struct ReflectionOrchestrator {
    backend: Arc<dyn LlmBackend>,
    semaphore: Arc<Semaphore>,
}

impl ReflectionOrchestrator {
    pub fn new(backend: Arc<dyn LlmBackend>) -> Self {
        Self {
            backend,
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_SESSIONS)),
        }
    }

    /// Run the reflection loop under `cancel`.
    ///
    /// If the concurrent-session semaphore is exhausted, falls back to a single
    /// direct backend call (no retry). Returns `ReflectionError::Cancelled` if
    /// `cancel` fires before a response is obtained.
    pub async fn run(
        &self,
        request: ConversationRequest,
        cancel: CancellationToken,
    ) -> Result<ConversationResponse, ReflectionError> {
        // try_acquire_owned: non-blocking. If the semaphore is full we skip
        // reflection entirely and fall through to a single direct call.
        let permit: Option<OwnedSemaphorePermit> = self.semaphore.clone().try_acquire_owned().ok();

        if permit.is_some() {
            self.run_with_reflection(request, cancel).await
        } else {
            self.run_direct(request, cancel).await
        }
        // permit drops here, releasing the semaphore slot.
    }

    async fn run_direct(
        &self,
        request: ConversationRequest,
        cancel: CancellationToken,
    ) -> Result<ConversationResponse, ReflectionError> {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ReflectionError::Cancelled),
            result = tokio::time::timeout(BACKEND_TIMEOUT, self.backend.complete(request)) => {
                match result {
                    Ok(Ok(r)) => Ok(r),
                    Ok(Err(e)) => Err(ReflectionError::Backend(e)),
                    Err(_) => Err(ReflectionError::Backend(BackendError::Timeout)),
                }
            }
        }
    }

    async fn run_with_reflection(
        &self,
        mut request: ConversationRequest,
        cancel: CancellationToken,
    ) -> Result<ConversationResponse, ReflectionError> {
        let mut last_response: Option<ConversationResponse> = None;

        for attempt in 0..MAX_ATTEMPTS {
            let response = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ReflectionError::Cancelled),
                result = tokio::time::timeout(
                    BACKEND_TIMEOUT,
                    self.backend.complete(request.clone()),
                ) => {
                    match result {
                        Ok(Ok(r)) => r,
                        Ok(Err(e)) => return Err(ReflectionError::Backend(e)),
                        Err(_elapsed) => return Err(ReflectionError::Backend(BackendError::Timeout)),
                    }
                }
            };

            // On the last attempt take whatever we got.
            if attempt == MAX_ATTEMPTS - 1 {
                return Ok(response);
            }

            match quality_issue(&response) {
                None => return Ok(response),
                Some(reason) => {
                    // Inject the bad response plus a reflection prompt, then retry.
                    let trimmed = response.content.trim().to_owned();
                    if !trimmed.is_empty() {
                        request.messages.push(Message::assistant(trimmed));
                    }
                    request
                        .messages
                        .push(Message::user(reflection_prompt(reason)));
                    last_response = Some(response);
                }
            }
        }

        // Unreachable in practice: the loop always returns inside the body.
        Ok(last_response.expect("loop ran at least once"))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use domain::entities::model::{ModelDescriptor, ModelId};
    use domain::entities::stream_chunk::StreamChunk;
    use domain::ports::llm_backend::BackendStream;
    use uuid::Uuid;

    use super::*;

    struct FakeBackend {
        responses: Mutex<Vec<ConversationResponse>>,
    }

    impl FakeBackend {
        fn new(responses: Vec<ConversationResponse>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl LlmBackend for FakeBackend {
        async fn complete(
            &self,
            _req: ConversationRequest,
        ) -> Result<ConversationResponse, BackendError> {
            let mut q = self.responses.lock().unwrap();
            if q.len() > 1 {
                Ok(q.remove(0))
            } else {
                Ok(q[0].clone())
            }
        }

        async fn stream(&self, _req: ConversationRequest) -> Result<BackendStream, BackendError> {
            Ok(Box::pin(futures_util::stream::empty::<
                Result<StreamChunk, BackendError>,
            >()))
        }

        async fn list_models(&self) -> Result<Vec<ModelDescriptor>, BackendError> {
            Ok(vec![])
        }

        async fn health_check(&self) -> Result<(), BackendError> {
            Ok(())
        }
    }

    fn good_response(content: &str) -> ConversationResponse {
        ConversationResponse {
            id: Uuid::new_v4(),
            model: ModelId::new("test"),
            content: content.to_owned(),
            prompt_tokens: 0,
            completion_tokens: 0,
            tool_calls: None,
        }
    }

    fn base_request() -> ConversationRequest {
        ConversationRequest::new(ModelId::new("test"), vec![Message::user("write code")])
    }

    #[tokio::test]
    async fn good_response_returned_immediately() {
        let backend = Arc::new(FakeBackend::new(vec![good_response("fn main() {}")]));
        let orch = ReflectionOrchestrator::new(backend);
        let resp = orch
            .run(base_request(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(resp.content, "fn main() {}");
    }

    #[tokio::test]
    async fn empty_response_triggers_retry() {
        let backend = Arc::new(FakeBackend::new(vec![
            good_response(""),
            good_response("fn main() {}"),
        ]));
        let orch = ReflectionOrchestrator::new(backend);
        let resp = orch
            .run(base_request(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(resp.content, "fn main() {}");
    }

    #[tokio::test]
    async fn truncated_code_block_triggers_retry() {
        let backend = Arc::new(FakeBackend::new(vec![
            good_response("Here is the code:\n```rust\nfn main("),
            good_response("fn main() {}"),
        ]));
        let orch = ReflectionOrchestrator::new(backend);
        let resp = orch
            .run(base_request(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(resp.content, "fn main() {}");
    }

    #[tokio::test]
    async fn max_attempts_returns_last_response() {
        let backend = Arc::new(FakeBackend::new(vec![good_response("")]));
        let orch = ReflectionOrchestrator::new(backend);
        let resp = orch
            .run(base_request(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(resp.content, "");
    }

    #[tokio::test]
    async fn cancellation_returns_cancelled_error() {
        let backend = Arc::new(FakeBackend::new(vec![good_response("ok")]));
        let orch = ReflectionOrchestrator::new(backend);
        let token = CancellationToken::new();
        token.cancel();
        let err = orch.run(base_request(), token).await.unwrap_err();
        assert!(matches!(err, ReflectionError::Cancelled));
    }

    #[tokio::test]
    async fn full_semaphore_falls_back_to_direct_call() {
        let backend = Arc::new(FakeBackend::new(vec![good_response("direct")]));
        let orch = ReflectionOrchestrator::new(backend);

        // Exhaust the semaphore by acquiring all permits.
        let _permits: Vec<_> = (0..MAX_CONCURRENT_SESSIONS)
            .map(|_| orch.semaphore.clone().try_acquire_owned().unwrap())
            .collect();

        // With no permits available, run() falls through to run_direct().
        let resp = orch
            .run(base_request(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(resp.content, "direct");
    }

    // --- quality_issue unit tests ---

    #[test]
    fn quality_issue_good_response() {
        assert!(quality_issue(&good_response("fn main() {}")).is_none());
    }

    #[test]
    fn quality_issue_empty_content() {
        assert!(quality_issue(&good_response("")).is_some());
    }

    #[test]
    fn quality_issue_truncated_fence() {
        assert!(quality_issue(&good_response("```rust\nfn main(")).is_some());
    }

    #[test]
    fn quality_issue_closed_fence_is_fine() {
        assert!(quality_issue(&good_response("```rust\nfn main() {}\n```")).is_none());
    }

    #[test]
    fn quality_issue_truncated_heading() {
        assert!(quality_issue(&good_response("Some intro\n\n###")).is_some());
        assert!(quality_issue(&good_response("Some intro\n\n##")).is_some());
        assert!(quality_issue(&good_response("Some intro\n\n#")).is_some());
    }

    #[test]
    fn quality_issue_complete_heading_is_fine() {
        assert!(quality_issue(&good_response("### Summary\nAll done.")).is_none());
    }

    #[test]
    fn quality_issue_truncated_sentence() {
        assert!(quality_issue(&good_response(
            "This function sorts a vector of integers using Rust"
        ))
        .is_some());
    }

    #[test]
    fn quality_issue_sentence_ends_with_punctuation() {
        assert!(quality_issue(&good_response("This function sorts a vector.")).is_none());
        assert!(quality_issue(&good_response("Use vec.sort();")).is_none());
        assert!(quality_issue(&good_response("Here is the result:")).is_none());
    }

    #[test]
    fn quality_issue_truncated_sentence_not_fired_inside_open_fence() {
        // Open fence — truncated-sentence check must not fire; fence check handles it.
        let r = good_response("```rust\nfn sort(v: &mut Vec<i32>)");
        let issue = quality_issue(&r);
        assert!(issue.is_some());
        assert_eq!(issue, Some("truncated code block"));
    }

    #[tokio::test]
    async fn truncated_heading_triggers_retry() {
        let backend = Arc::new(FakeBackend::new(vec![
            good_response("### Summary\nThis function sorts a vector\n\n###"),
            good_response("```rust\nfn sort(v: &mut Vec<i32>) { v.sort(); }\n```"),
        ]));
        let orch = ReflectionOrchestrator::new(backend);
        let resp = orch
            .run(base_request(), CancellationToken::new())
            .await
            .unwrap();
        assert!(resp.content.contains("fn sort"));
    }
}
