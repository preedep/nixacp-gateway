use domain::entities::message::{Message, Role};

/// Ensures a system message is present as the first message.
///
/// If the conversation has no system message, a coding-focused default is prepended.
/// If one already exists it is left in place.
#[derive(Default)]
pub struct SystemPromptBuilder {
    default_system_prompt: Option<String>,
}

impl SystemPromptBuilder {
    pub fn with_default(prompt: impl Into<String>) -> Self {
        Self { default_system_prompt: Some(prompt.into()) }
    }

    pub fn apply(&self, _model: &str, mut messages: Vec<Message>) -> Vec<Message> {
        let has_system = messages.first().map(|m| m.role == Role::System).unwrap_or(false);
        if !has_system {
            if let Some(ref prompt) = self.default_system_prompt {
                messages.insert(0, Message::system(prompt.as_str()));
            }
        }
        messages
    }
}
