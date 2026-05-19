use std::sync::Arc;

use domain::entities::message::{ContentPart, Message};

use super::ModelAdapterRegistry;

/// Strips model-specific injection tokens by delegating to the registered `ModelAdapter`.
///
/// Previously contained hardcoded `if model.contains("qwen")` branches. Now any model
/// quirk is handled by its adapter — add a new adapter, get cleaning for free.
pub struct ModelQuirksTransformer {
    registry: Arc<ModelAdapterRegistry>,
}

impl ModelQuirksTransformer {
    pub fn new(registry: Arc<ModelAdapterRegistry>) -> Self {
        Self { registry }
    }

    pub fn apply(&self, model: &str, messages: Vec<Message>) -> Vec<Message> {
        let adapter = self.registry.for_model(model);
        messages
            .into_iter()
            .map(|mut msg| {
                msg.content = msg
                    .content
                    .into_iter()
                    .map(|part| {
                        let ContentPart::Text(text) = part;
                        ContentPart::Text(adapter.clean_content(text))
                    })
                    .collect();
                msg
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use domain::entities::message::Message;
    use domain::ports::model_adapter::ModelAdapter;

    struct StripBracketsAdapter;
    impl ModelAdapter for StripBracketsAdapter {
        fn matches(&self, model: &str) -> bool {
            model == "brackets"
        }
        fn clean_content(&self, text: String) -> String {
            text.replace(['[', ']'], "").trim().to_owned()
        }
        fn tool_instructions(&self, _: &str, _: &[&str]) -> String {
            String::new()
        }
    }

    struct NoopAdapter;
    impl ModelAdapter for NoopAdapter {
        fn matches(&self, _: &str) -> bool {
            true
        }
        fn tool_instructions(&self, _: &str, _: &[&str]) -> String {
            String::new()
        }
    }

    fn registry_with_strip() -> Arc<ModelAdapterRegistry> {
        Arc::new(ModelAdapterRegistry::new(vec![
            Arc::new(StripBracketsAdapter),
            Arc::new(NoopAdapter),
        ]))
    }

    #[test]
    fn delegates_cleaning_to_adapter() {
        let t = ModelQuirksTransformer::new(registry_with_strip());
        let msgs = vec![Message::user("[hello] world")];
        let out = t.apply("brackets", msgs);
        assert_eq!(out[0].text_content().unwrap(), "hello world");
    }

    #[test]
    fn no_change_for_noop_adapter() {
        let t = ModelQuirksTransformer::new(registry_with_strip());
        let msgs = vec![Message::user("[hello] world")];
        let out = t.apply("other-model", msgs);
        assert_eq!(out[0].text_content().unwrap(), "[hello] world");
    }

    #[test]
    fn applies_to_all_messages() {
        let t = ModelQuirksTransformer::new(registry_with_strip());
        let msgs = vec![
            Message::user("[first]"),
            Message::user("[second]"),
        ];
        let out = t.apply("brackets", msgs);
        assert_eq!(out[0].text_content().unwrap(), "first");
        assert_eq!(out[1].text_content().unwrap(), "second");
    }
}
