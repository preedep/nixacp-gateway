use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub stream: Option<bool>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ApiToolDefinition>,
}

/// A single content block inside a multi-part message (OpenAI content array format).
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ContentBlock {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub text: String,
}

/// Content field: Zed sends an array of content blocks; plain clients send a string.
/// We deserialize either form and flatten to a plain string immediately.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    pub fn into_text(self) -> String {
        match self {
            MessageContent::Text(s) => s,
            MessageContent::Blocks(blocks) => blocks
                .into_iter()
                .filter(|b| b.kind == "text")
                .map(|b| b.text)
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

impl Default for MessageContent {
    fn default() -> Self {
        MessageContent::Text(String::new())
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ApiToolCall>>,
}

impl ChatMessage {
    pub fn content_text(&self) -> String {
        self.content.clone().into_text()
    }
}

impl Serialize for ChatMessage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let content_str = self.content.clone().into_text();
        let field_count = 2
            + self.tool_call_id.is_some() as usize
            + self.tool_calls.is_some() as usize;
        let mut s = serializer.serialize_struct("ChatMessage", field_count)?;
        s.serialize_field("role", &self.role)?;
        s.serialize_field("content", &content_str)?;
        if let Some(ref id) = self.tool_call_id {
            s.serialize_field("tool_call_id", id)?;
        }
        if let Some(ref calls) = self.tool_calls {
            s.serialize_field("tool_calls", calls)?;
        }
        s.end()
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ApiToolDefinition {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ApiFunctionDefinition,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ApiFunctionDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ApiToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ApiFunctionCall,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ApiFunctionCall {
    pub name: String,
    pub arguments: String,
}

// --- Streaming (SSE) response ---

#[derive(Debug, Serialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: &'static str,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
}

#[derive(Debug, Serialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: ChunkDelta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

// --- Non-streaming response ---

#[derive(Debug, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: &'static str,
    pub model: String,
    pub choices: Vec<CompletionChoice>,
    pub usage: Usage,
}

#[derive(Debug, Serialize)]
pub struct CompletionChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: String,
}

#[derive(Debug, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

// --- Models endpoint ---

#[derive(Debug, Serialize)]
pub struct ModelListResponse {
    pub object: &'static str,
    pub data: Vec<ModelObject>,
}

#[derive(Debug, Serialize)]
pub struct ModelObject {
    pub id: String,
    pub object: &'static str,
    pub owned_by: String,
}
