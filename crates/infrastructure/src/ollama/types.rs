use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub(crate) struct OllamaGenerateRequest {
    pub model: String,
    pub messages: Vec<OllamaChatMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<OllamaOptions>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OllamaChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OllamaChatChunk {
    #[allow(dead_code)]
    pub model: String,
    pub message: OllamaMessageContent,
    pub done: bool,
    #[allow(dead_code)]
    pub done_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OllamaMessageContent {
    #[allow(dead_code)]
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OllamaTagsResponse {
    pub models: Vec<OllamaModelEntry>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OllamaModelEntry {
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_generate_request() {
        let req = OllamaGenerateRequest {
            model: "qwen2.5-coder:14b".to_string(),
            messages: vec![OllamaChatMessage {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            stream: true,
            options: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"model\":\"qwen2.5-coder:14b\""));
        assert!(json.contains("\"stream\":true"));
        assert!(!json.contains("options"));
    }

    #[test]
    fn deserialize_chat_chunk_delta() {
        let json = r#"{"model":"qwen2.5-coder:14b","message":{"role":"assistant","content":"hi"},"done":false}"#;
        let chunk: OllamaChatChunk = serde_json::from_str(json).unwrap();
        assert!(!chunk.done);
        assert_eq!(chunk.message.content, "hi");
    }

    #[test]
    fn deserialize_chat_chunk_done() {
        let json = r#"{"model":"qwen2.5-coder:14b","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop"}"#;
        let chunk: OllamaChatChunk = serde_json::from_str(json).unwrap();
        assert!(chunk.done);
        assert_eq!(chunk.done_reason.as_deref(), Some("stop"));
    }
}
