use std::sync::Arc;

use domain::entities::conversation::{ConversationRequest, ConversationResponse};
use domain::ports::llm_backend::{BackendError, BackendStream, LlmBackend};
use domain::ports::token_counter::TokenCounter;

use crate::compression::CompressionService;
use crate::prompt::PromptPipeline;

/// Default context budget applied when `ConversationRequest.max_tokens` is absent.
/// 90% of a 4K context avoids off-by-one token rejections.
const DEFAULT_CONTEXT_BUDGET: u32 = 3_600;

pub struct ChatService {
    pub backend: Arc<dyn LlmBackend>,
    pipeline: PromptPipeline,
    compression: CompressionService,
    counter: Arc<dyn TokenCounter>,
}

impl ChatService {
    pub fn new(
        backend: Arc<dyn LlmBackend>,
        pipeline: PromptPipeline,
        compression: CompressionService,
        counter: Arc<dyn TokenCounter>,
    ) -> Self {
        Self {
            backend,
            pipeline,
            compression,
            counter,
        }
    }

    /// Apply prompt pipeline transforms (system prompt injection, quirks) without compression.
    /// Used by callers that manage their own conversation loop (e.g. tool loop).
    pub fn apply_pipeline(
        &self,
        model: &str,
        messages: Vec<domain::entities::message::Message>,
    ) -> Vec<domain::entities::message::Message> {
        self.pipeline.transform(model, messages)
    }

    fn prepare(&self, mut req: ConversationRequest) -> ConversationRequest {
        req.messages = self.pipeline.transform(req.model.as_str(), req.messages);
        let budget = req.max_tokens.unwrap_or(DEFAULT_CONTEXT_BUDGET);
        req.messages = self.compression.maybe_compress(req.messages, budget);
        req
    }

    pub async fn stream(&self, req: ConversationRequest) -> Result<BackendStream, BackendError> {
        self.backend.stream(self.prepare(req)).await
    }

    pub async fn complete(
        &self,
        req: ConversationRequest,
    ) -> Result<ConversationResponse, BackendError> {
        let prompt_tokens = self.counter.count_messages(&req.messages);
        let req = self.prepare(req);
        let mut resp = self.backend.complete(req).await?;
        resp.prompt_tokens = prompt_tokens;
        resp.completion_tokens = self.counter.count_str(&resp.content);
        Ok(resp)
    }
}
