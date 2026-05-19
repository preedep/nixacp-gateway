use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub(crate) struct OllamaGenerateRequest {
    pub model: String,
    pub messages: Vec<OllamaChatMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<OllamaOptions>,
    /// Tool definitions forwarded to Ollama when the request includes tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OllamaTool>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OllamaChatMessage {
    pub role: String,
    pub content: String,
    /// Tool calls emitted by the assistant; present only on assistant messages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OllamaToolCall>>,
    /// Links a tool-result message to the call that produced it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct OllamaToolCall {
    pub function: OllamaFunction,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct OllamaFunction {
    pub name: String,
    /// Ollama emits arguments as a JSON object (not a string like OpenAI).
    pub arguments: serde_json::Value,
}

/// Tool definition sent to Ollama in the request body.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct OllamaTool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: OllamaToolFunction,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct OllamaToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
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
    /// Present when Ollama returns a structured tool call response.
    pub tool_calls: Option<Vec<OllamaToolCall>>,
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
    use serde_json::json;

    #[test]
    fn serialize_generate_request() {
        let req = OllamaGenerateRequest {
            model: "qwen2.5-coder:14b".to_string(),
            messages: vec![OllamaChatMessage {
                role: "user".to_string(),
                content: "hello".to_string(),
                tool_calls: None,
                tool_call_id: None,
            }],
            stream: true,
            options: None,
            tools: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"model\":\"qwen2.5-coder:14b\""));
        assert!(json.contains("\"stream\":true"));
        assert!(!json.contains("options"));
        assert!(!json.contains("tools"));
    }

    #[test]
    fn serialize_generate_request_with_tools() {
        let req = OllamaGenerateRequest {
            model: "qwen2.5-coder:14b".to_string(),
            messages: vec![],
            stream: false,
            options: None,
            tools: Some(vec![OllamaTool {
                kind: "function".to_owned(),
                function: OllamaToolFunction {
                    name: "read_file".to_owned(),
                    description: "Read a file".to_owned(),
                    parameters: json!({ "type": "object" }),
                },
            }]),
        };
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["tools"][0]["type"], "function");
        assert_eq!(v["tools"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn deserialize_chat_chunk_delta() {
        let json = r#"{"model":"qwen2.5-coder:14b","message":{"role":"assistant","content":"hi"},"done":false}"#;
        let chunk: OllamaChatChunk = serde_json::from_str(json).unwrap();
        assert!(!chunk.done);
        assert_eq!(chunk.message.content, "hi");
        assert!(chunk.message.tool_calls.is_none());
    }

    #[test]
    fn deserialize_chat_chunk_done() {
        let json = r#"{"model":"qwen2.5-coder:14b","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop"}"#;
        let chunk: OllamaChatChunk = serde_json::from_str(json).unwrap();
        assert!(chunk.done);
        assert_eq!(chunk.done_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn deserialize_chat_chunk_with_tool_call() {
        let json = r#"{
            "model":"qwen2.5-coder:14b",
            "message":{
                "role":"assistant",
                "content":"",
                "tool_calls":[{"function":{"name":"read_file","arguments":{"path":"src/main.rs"}}}]
            },
            "done":true,
            "done_reason":"stop"
        }"#;
        let chunk: OllamaChatChunk = serde_json::from_str(json).unwrap();
        let tool_calls = chunk.message.tool_calls.unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "read_file");
        assert_eq!(tool_calls[0].function.arguments["path"], "src/main.rs");
    }

    #[test]
    fn ollama_chat_message_omits_optional_fields_when_none() {
        let msg = OllamaChatMessage {
            role: "user".to_owned(),
            content: "hello".to_owned(),
            tool_calls: None,
            tool_call_id: None,
        };
        let v: serde_json::Value = serde_json::to_value(&msg).unwrap();
        assert!(v.get("tool_calls").is_none());
        assert!(v.get("tool_call_id").is_none());
    }
}
