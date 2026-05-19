mod adapter_registry;
mod quirks;
mod system_prompt;

pub use adapter_registry::ModelAdapterRegistry;
pub use quirks::ModelQuirksTransformer;
pub use system_prompt::SystemPromptBuilder;

use domain::entities::message::Message;

/// Ordered pipeline of message transformations applied before every backend call.
pub struct PromptPipeline {
    system_prompt: SystemPromptBuilder,
    quirks: ModelQuirksTransformer,
}

impl PromptPipeline {
    pub fn new(system_prompt: SystemPromptBuilder, quirks: ModelQuirksTransformer) -> Self {
        Self {
            system_prompt,
            quirks,
        }
    }

    pub fn transform(&self, model: &str, messages: Vec<Message>) -> Vec<Message> {
        let messages = self.system_prompt.apply(model, messages);
        self.quirks.apply(model, messages)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use domain::entities::message::{Message, Role};
    use domain::ports::model_adapter::ModelAdapter;

    struct StripMarkerAdapter;
    impl ModelAdapter for StripMarkerAdapter {
        fn matches(&self, _: &str) -> bool {
            true
        }
        fn clean_content(&self, text: String) -> String {
            text.replace("<MARK>", "").trim().to_owned()
        }
        fn tool_instructions(&self, _: &str, _: &[&str]) -> String {
            String::new()
        }
    }

    fn test_pipeline() -> PromptPipeline {
        let registry = Arc::new(ModelAdapterRegistry::new(vec![Arc::new(StripMarkerAdapter)]));
        PromptPipeline::new(
            SystemPromptBuilder::new(registry.clone(), "/ws", vec![]),
            ModelQuirksTransformer::new(registry),
        )
    }

    #[test]
    fn pipeline_cleans_content_via_adapter() {
        let pipeline = test_pipeline();
        let messages = vec![Message::user("<MARK>hello<MARK>")];
        let out = pipeline.transform("any-model", messages);
        assert_eq!(out[1].text_content().unwrap(), "hello");
    }

    #[test]
    fn pipeline_preserves_system_message_position() {
        let pipeline = test_pipeline();
        let messages = vec![Message::system("be helpful"), Message::user("hi")];
        let out = pipeline.transform("any-model", messages);
        assert_eq!(out[0].role, Role::System);
    }
}
