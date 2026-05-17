mod quirks;
mod system_prompt;

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
    use super::*;
    use domain::entities::message::{Message, Role};

    #[test]
    fn pipeline_strips_injection_tokens() {
        let pipeline = PromptPipeline::new(SystemPromptBuilder::default(), ModelQuirksTransformer);
        let messages = vec![Message::user("<|im_start|>user\nhello<|im_end|>")];
        let out = pipeline.transform("qwen2.5-coder:14b", messages);
        let text = out[0].text_content().unwrap();
        assert!(
            !text.contains("<|im_start|>"),
            "injection tokens must be stripped"
        );
        assert!(
            !text.contains("<|im_end|>"),
            "injection tokens must be stripped"
        );
    }

    #[test]
    fn pipeline_strips_think_tags() {
        let pipeline = PromptPipeline::new(SystemPromptBuilder::default(), ModelQuirksTransformer);
        let messages = vec![Message::user("<think>internal</think>answer")];
        let out = pipeline.transform("deepseek-coder:7b", messages);
        let text = out[0].text_content().unwrap();
        assert!(!text.contains("<think>"), "think tags must be stripped");
        assert!(text.contains("answer"));
    }

    #[test]
    fn pipeline_preserves_system_message_position() {
        let pipeline = PromptPipeline::new(SystemPromptBuilder::default(), ModelQuirksTransformer);
        let messages = vec![Message::system("be helpful"), Message::user("hi")];
        let out = pipeline.transform("qwen2.5-coder:14b", messages);
        assert_eq!(out[0].role, Role::System);
    }
}
