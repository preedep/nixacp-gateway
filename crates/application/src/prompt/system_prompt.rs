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
        Self {
            default_system_prompt: Some(prompt.into()),
        }
    }

    pub fn apply(&self, _model: &str, mut messages: Vec<Message>) -> Vec<Message> {
        let Some(ref default_prompt) = self.default_system_prompt else {
            return messages;
        };

        // If a system message already exists (e.g. from Zed), append our
        // tool-use rules to it so both survive. This is critical: without our
        // injected rules the model ignores the tool definitions entirely and
        // falls back to prose shell-command suggestions.
        if let Some(first) = messages.first_mut() {
            if first.role == Role::System {
                use domain::entities::message::ContentPart;
                if let Some(ContentPart::Text(ref mut text)) = first.content.first_mut() {
                    text.push('\n');
                    text.push_str(default_prompt);
                }
                return messages;
            }
        }

        messages.insert(0, Message::system(default_prompt.as_str()));
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::entities::message::Role;

    #[test]
    fn prepends_when_no_system_message() {
        let builder = SystemPromptBuilder::with_default("USE TOOLS");
        let msgs = vec![Message::user("hello")];
        let out = builder.apply("model", msgs);
        assert_eq!(out[0].role, Role::System);
        assert_eq!(out[0].text_content(), Some("USE TOOLS"));
        assert_eq!(out[1].role, Role::User);
    }

    #[test]
    fn appends_to_existing_system_message() {
        let builder = SystemPromptBuilder::with_default("USE TOOLS");
        let msgs = vec![Message::system("You are helpful."), Message::user("hi")];
        let out = builder.apply("model", msgs);
        // Still only one system message
        assert_eq!(out.iter().filter(|m| m.role == Role::System).count(), 1);
        let sys_text = out[0].text_content().unwrap();
        assert!(
            sys_text.contains("You are helpful."),
            "original text must be preserved"
        );
        assert!(sys_text.contains("USE TOOLS"), "our rules must be appended");
    }

    #[test]
    fn no_default_leaves_messages_unchanged() {
        let builder = SystemPromptBuilder::default();
        let msgs = vec![Message::user("hello")];
        let out = builder.apply("model", msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, Role::User);
    }
}
