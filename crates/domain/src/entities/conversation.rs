use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::message::Message;
use crate::entities::model::ModelId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationRequest {
    pub id: Uuid,
    pub model: ModelId,
    pub messages: Vec<Message>,
    pub stream: bool,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

impl ConversationRequest {
    pub fn new(model: ModelId, messages: Vec<Message>) -> Self {
        Self {
            id: Uuid::new_v4(),
            model,
            messages,
            stream: true,
            temperature: None,
            max_tokens: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationResponse {
    pub id: Uuid,
    pub model: ModelId,
    pub content: String,
    /// Stubbed in Phase 1; filled in Phase 2 when TiktokenCounter is added.
    pub prompt_tokens: u32,
    /// Stubbed in Phase 1; filled in Phase 2.
    pub completion_tokens: u32,
}
