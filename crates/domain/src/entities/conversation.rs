use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::message::Message;
use crate::entities::model::ModelId;
use crate::entities::tool::{ToolCall, ToolDefinition};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationRequest {
    pub id: Uuid,
    pub model: ModelId,
    pub messages: Vec<Message>,
    pub stream: bool,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// Tools the model may call. Empty means no tool use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_definitions: Vec<ToolDefinition>,
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
            tool_definitions: Vec::new(),
        }
    }

    pub fn has_tools(&self) -> bool {
        !self.tool_definitions.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationResponse {
    pub id: Uuid,
    pub model: ModelId,
    pub content: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// Set when the model requests tool execution instead of producing a final answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl ConversationResponse {
    pub fn is_tool_call(&self) -> bool {
        self.tool_calls
            .as_ref()
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::message::Message;
    use crate::entities::model::ModelId;
    use crate::entities::tool::{ToolCall, ToolDefinition};
    use serde_json::json;

    #[test]
    fn conversation_request_no_tools_by_default() {
        let req = ConversationRequest::new(ModelId::new("m"), vec![Message::user("hi")]);
        assert!(!req.has_tools());
        assert!(req.tool_definitions.is_empty());
    }

    #[test]
    fn conversation_request_with_tools() {
        let mut req = ConversationRequest::new(ModelId::new("m"), vec![Message::user("hi")]);
        req.tool_definitions = vec![ToolDefinition::function(
            "file_read",
            "Read a file",
            json!({}),
        )];
        assert!(req.has_tools());
    }

    #[test]
    fn tool_definitions_omitted_from_json_when_empty() {
        let req = ConversationRequest::new(ModelId::new("m"), vec![Message::user("hi")]);
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert!(v.get("tool_definitions").is_none());
    }

    #[test]
    fn conversation_request_serde_round_trip_with_tools() {
        let mut req = ConversationRequest::new(ModelId::new("qwen"), vec![Message::user("hi")]);
        req.tool_definitions = vec![ToolDefinition::function(
            "search",
            "Search code",
            json!({ "type": "object" }),
        )];
        let json = serde_json::to_string(&req).unwrap();
        let back: ConversationRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool_definitions.len(), 1);
        assert_eq!(back.tool_definitions[0].function.name, "search");
    }

    #[test]
    fn conversation_response_is_tool_call_true() {
        let resp = ConversationResponse {
            id: Uuid::new_v4(),
            model: ModelId::new("m"),
            content: String::new(),
            prompt_tokens: 0,
            completion_tokens: 0,
            tool_calls: Some(vec![ToolCall::new("c1", "file_read", r#"{"path":"a.rs"}"#)]),
        };
        assert!(resp.is_tool_call());
    }

    #[test]
    fn conversation_response_is_tool_call_false_when_none() {
        let resp = ConversationResponse {
            id: Uuid::new_v4(),
            model: ModelId::new("m"),
            content: "hello".into(),
            prompt_tokens: 5,
            completion_tokens: 3,
            tool_calls: None,
        };
        assert!(!resp.is_tool_call());
    }

    #[test]
    fn conversation_response_is_tool_call_false_when_empty_vec() {
        let resp = ConversationResponse {
            id: Uuid::new_v4(),
            model: ModelId::new("m"),
            content: String::new(),
            prompt_tokens: 0,
            completion_tokens: 0,
            tool_calls: Some(vec![]),
        };
        assert!(!resp.is_tool_call());
    }

    #[test]
    fn tool_calls_omitted_from_json_when_none() {
        let resp = ConversationResponse {
            id: Uuid::new_v4(),
            model: ModelId::new("m"),
            content: "done".into(),
            prompt_tokens: 0,
            completion_tokens: 0,
            tool_calls: None,
        };
        let v: serde_json::Value = serde_json::to_value(&resp).unwrap();
        assert!(v.get("tool_calls").is_none());
    }
}
