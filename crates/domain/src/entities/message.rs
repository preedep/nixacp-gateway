use serde::{Deserialize, Serialize};

use crate::entities::tool::ToolCall;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContentPart {
    Text(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentPart>,
    /// Present on Role::Tool messages — links this result to the ToolCall that
    /// produced it. Required by the OpenAI spec and Ollama structured tool format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Present on Role::Assistant messages when the model requests tool execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System => write!(f, "system"),
            Self::User => write!(f, "user"),
            Self::Assistant => write!(f, "assistant"),
            Self::Tool => write!(f, "tool"),
        }
    }
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentPart::Text(text.into())],
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentPart::Text(text.into())],
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn assistant_with_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentPart::Text(String::new())],
            tool_call_id: None,
            tool_calls: Some(tool_calls),
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![ContentPart::Text(text.into())],
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Construct a tool-result message that feeds back into the conversation.
    /// `tool_call_id` must match the `ToolCall.id` this result responds to.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: vec![ContentPart::Text(content.into())],
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: None,
        }
    }

    /// Returns the flat text if there is exactly one Text part.
    pub fn text_content(&self) -> Option<&str> {
        if let [ContentPart::Text(s)] = self.content.as_slice() {
            Some(s.as_str())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::tool::ToolCall;

    #[test]
    fn text_content_single_part() {
        let m = Message::user("hello");
        assert_eq!(m.text_content(), Some("hello"));
    }

    #[test]
    fn role_serde_round_trip() {
        let json = serde_json::to_string(&Role::Assistant).unwrap();
        assert_eq!(json, "\"assistant\"");
        let role: Role = serde_json::from_str("\"user\"").unwrap();
        assert_eq!(role, Role::User);
    }

    #[test]
    fn content_part_serde_round_trip() {
        let part = ContentPart::Text("hi".to_string());
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        let ContentPart::Text(s) = back;
        assert_eq!(s, "hi");
    }

    #[test]
    fn tool_result_message_has_tool_call_id() {
        let m = Message::tool_result("call_001", "file contents");
        assert_eq!(m.role, Role::Tool);
        assert_eq!(m.tool_call_id.as_deref(), Some("call_001"));
        assert_eq!(m.text_content(), Some("file contents"));
        assert!(m.tool_calls.is_none());
    }

    #[test]
    fn assistant_with_tool_calls_has_no_tool_call_id() {
        let calls = vec![ToolCall::new("call_1", "read_file", r#"{"path":"a.rs"}"#)];
        let m = Message::assistant_with_tool_calls(calls.clone());
        assert_eq!(m.role, Role::Assistant);
        assert!(m.tool_call_id.is_none());
        assert_eq!(m.tool_calls.as_ref().unwrap().len(), 1);
        assert_eq!(m.tool_calls.as_ref().unwrap()[0].id, "call_1");
    }

    #[test]
    fn tool_call_id_omitted_from_user_message_json() {
        let m = Message::user("hello");
        let v: serde_json::Value = serde_json::to_value(&m).unwrap();
        assert!(v.get("tool_call_id").is_none(), "tool_call_id must be absent from user message JSON");
        assert!(v.get("tool_calls").is_none(), "tool_calls must be absent from user message JSON");
    }

    #[test]
    fn tool_result_message_serde_round_trip() {
        let m = Message::tool_result("call_xyz", "result text");
        let json = serde_json::to_string(&m).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, Role::Tool);
        assert_eq!(back.tool_call_id.as_deref(), Some("call_xyz"));
        assert_eq!(back.text_content(), Some("result text"));
    }

    #[test]
    fn assistant_with_tool_calls_serde_round_trip() {
        let calls = vec![
            ToolCall::new("c1", "file_read", r#"{"path":"src/lib.rs"}"#),
            ToolCall::new("c2", "search", r#"{"query":"async fn"}"#),
        ];
        let m = Message::assistant_with_tool_calls(calls);
        let json = serde_json::to_string(&m).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        let tool_calls = back.tool_calls.unwrap();
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].id, "c1");
        assert_eq!(tool_calls[1].function.name, "search");
    }
}
