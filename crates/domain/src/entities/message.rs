use serde::{Deserialize, Serialize};

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
        Self { role: Role::User, content: vec![ContentPart::Text(text.into())] }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self { role: Role::Assistant, content: vec![ContentPart::Text(text.into())] }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self { role: Role::System, content: vec![ContentPart::Text(text.into())] }
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
        if let ContentPart::Text(s) = back {
            assert_eq!(s, "hi");
        } else {
            panic!("wrong variant");
        }
    }
}
