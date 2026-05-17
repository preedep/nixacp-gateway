use std::sync::Arc;

use domain::entities::conversation::{ConversationRequest, ConversationResponse};
use domain::ports::llm_backend::{BackendError, BackendStream, LlmBackend};

pub struct ChatService {
    pub backend: Arc<dyn LlmBackend>,
}

impl ChatService {
    pub fn new(backend: Arc<dyn LlmBackend>) -> Self {
        Self { backend }
    }

    pub async fn stream(&self, req: ConversationRequest) -> Result<BackendStream, BackendError> {
        self.backend.stream(req).await
    }

    pub async fn complete(&self, req: ConversationRequest) -> Result<ConversationResponse, BackendError> {
        self.backend.complete(req).await
    }
}
