use std::sync::Arc;

use domain::entities::message::{Message, Role};

use super::ModelAdapterRegistry;

/// Ensures a system message is present and contains model-appropriate tool instructions.
///
/// If the conversation has no system message, one is prepended.
/// If one already exists (e.g. from Zed), tool instructions are appended to it.
pub struct SystemPromptBuilder {
    registry: Arc<ModelAdapterRegistry>,
    workspace_root: String,
    tool_names: Vec<String>,
}

impl SystemPromptBuilder {
    pub fn new(
        registry: Arc<ModelAdapterRegistry>,
        workspace_root: impl Into<String>,
        tool_names: Vec<String>,
    ) -> Self {
        Self {
            registry,
            workspace_root: workspace_root.into(),
            tool_names,
        }
    }

    pub fn apply(&self, model: &str, mut messages: Vec<Message>) -> Vec<Message> {
        let tools: Vec<&str> = self.tool_names.iter().map(String::as_str).collect();
        let instructions = self
            .registry
            .for_model(model)
            .tool_instructions(&self.workspace_root, &tools);

        if let Some(first) = messages.first_mut() {
            if first.role == Role::System {
                use domain::entities::message::ContentPart;
                if let Some(ContentPart::Text(ref mut text)) = first.content.first_mut() {
                    text.push('\n');
                    text.push_str(&instructions);
                }
                return messages;
            }
        }

        messages.insert(0, Message::system(&instructions));
        messages
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use domain::entities::message::Role;
    use domain::ports::model_adapter::ModelAdapter;

    struct TaggedAdapter(&'static str);
    impl ModelAdapter for TaggedAdapter {
        fn matches(&self, model: &str) -> bool {
            model.starts_with(self.0)
        }
        fn tool_instructions(&self, root: &str, tools: &[&str]) -> String {
            format!("[{}] root={} tools={}", self.0, root, tools.join(","))
        }
    }

    struct CatchAllAdapter;
    impl ModelAdapter for CatchAllAdapter {
        fn matches(&self, _: &str) -> bool {
            true
        }
        fn tool_instructions(&self, root: &str, tools: &[&str]) -> String {
            format!("[default] root={} tools={}", root, tools.join(","))
        }
    }

    fn builder() -> SystemPromptBuilder {
        let registry = Arc::new(ModelAdapterRegistry::new(vec![
            Arc::new(TaggedAdapter("qwen")),
            Arc::new(TaggedAdapter("deepseek")),
            Arc::new(CatchAllAdapter),
        ]));
        SystemPromptBuilder::new(
            registry,
            "/workspace",
            vec!["read_file".into(), "search_files".into()],
        )
    }

    #[test]
    fn prepends_when_no_system_message() {
        let b = builder();
        let msgs = vec![Message::user("hello")];
        let out = b.apply("llama3:8b", msgs);
        assert_eq!(out[0].role, Role::System);
        assert_eq!(out[1].role, Role::User);
    }

    #[test]
    fn appends_to_existing_system_message() {
        let b = builder();
        let msgs = vec![Message::system("You are helpful."), Message::user("hi")];
        let out = b.apply("llama3:8b", msgs);
        assert_eq!(out.iter().filter(|m| m.role == Role::System).count(), 1);
        let sys = out[0].text_content().unwrap();
        assert!(sys.contains("You are helpful."));
        assert!(sys.contains("read_file"), "tool names must appear: {sys}");
    }

    #[test]
    fn model_specific_adapter_selected() {
        let b = builder();
        let msgs = vec![Message::user("hi")];
        let out = b.apply("qwen2.5-coder:14b", msgs);
        let sys = out[0].text_content().unwrap();
        assert!(sys.starts_with("[qwen]"), "got: {sys}");
        assert!(sys.contains("/workspace"));
        assert!(sys.contains("read_file"));
    }

    #[test]
    fn fallback_adapter_used_for_unknown_model() {
        let b = builder();
        let msgs = vec![Message::user("hi")];
        let out = b.apply("mistral:7b", msgs);
        let sys = out[0].text_content().unwrap();
        assert!(sys.starts_with("[default]"), "got: {sys}");
    }

    #[test]
    fn tool_names_appear_in_system_prompt() {
        let b = builder();
        let msgs = vec![Message::user("hi")];
        let out = b.apply("deepseek-coder:7b", msgs);
        let sys = out[0].text_content().unwrap();
        assert!(sys.contains("read_file"));
        assert!(sys.contains("search_files"));
    }
}
