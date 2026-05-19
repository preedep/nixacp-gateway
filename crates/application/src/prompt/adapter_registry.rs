use std::sync::Arc;

use domain::ports::model_adapter::ModelAdapter;

/// Selects the first registered adapter whose `matches()` returns true.
///
/// Registration order matters — more specific adapters (Qwen, DeepSeek) must
/// be added before `DefaultAdapter`, which matches every model.
pub struct ModelAdapterRegistry {
    adapters: Vec<Arc<dyn ModelAdapter>>,
}

impl ModelAdapterRegistry {
    pub fn new(adapters: Vec<Arc<dyn ModelAdapter>>) -> Self {
        Self { adapters }
    }

    /// Returns the first adapter that matches `model`, or panics if none does.
    ///
    /// In practice this never panics because `DefaultAdapter` is always last.
    pub fn for_model(&self, model: &str) -> &dyn ModelAdapter {
        self.adapters
            .iter()
            .find(|a| a.matches(model))
            .map(|a| a.as_ref())
            .expect("no adapter matched — DefaultAdapter must always be registered last")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::ports::model_adapter::ModelAdapter;

    struct PrefixAdapter(&'static str);

    impl ModelAdapter for PrefixAdapter {
        fn matches(&self, model: &str) -> bool {
            model.starts_with(self.0)
        }

        fn tool_instructions(&self, _root: &str, _tools: &[&str]) -> String {
            format!("adapter={}", self.0)
        }
    }

    fn registry() -> ModelAdapterRegistry {
        ModelAdapterRegistry::new(vec![
            Arc::new(PrefixAdapter("qwen")),
            Arc::new(PrefixAdapter("deepseek")),
            Arc::new(PrefixAdapter("")), // catches everything — acts as default
        ])
    }

    #[test]
    fn selects_first_match() {
        let r = registry();
        let instr = r.for_model("qwen2.5-coder:14b").tool_instructions("", &[]);
        assert_eq!(instr, "adapter=qwen");
    }

    #[test]
    fn falls_through_to_default() {
        let r = registry();
        let instr = r.for_model("llama3:8b").tool_instructions("", &[]);
        assert_eq!(instr, "adapter=");
    }

    #[test]
    fn deepseek_matched_before_default() {
        let r = registry();
        let instr = r.for_model("deepseek-coder:7b").tool_instructions("", &[]);
        assert_eq!(instr, "adapter=deepseek");
    }
}
