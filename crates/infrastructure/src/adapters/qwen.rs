use domain::ports::model_adapter::ModelAdapter;

/// Adapter for Qwen / Qwen2.5-coder models served via Ollama.
///
/// Strips `<|im_start|>` / `<|im_end|>` chat-template tokens that leak into
/// user messages when Ollama exposes Qwen's raw chat template output.
pub struct QwenAdapter;

impl ModelAdapter for QwenAdapter {
    fn matches(&self, model: &str) -> bool {
        model.contains("qwen") || model.contains("Qwen")
    }

    fn clean_content(&self, text: String) -> String {
        let text = text.replace("<|im_start|>", "");
        let text = text.replace("<|im_end|>", "");
        text.trim().to_owned()
    }

    fn tool_instructions(&self, workspace_root: &str, tools: &[&str]) -> String {
        let tool_list = tools.join(", ");
        format!(
            "You are a helpful coding assistant with access to the user's workspace.\n\
            Workspace root: {workspace_root}\n\
            Available tools: {tool_list}\n\n\
            When you need to read, search, or modify files, call the appropriate tool.\n\
            After receiving tool results, answer the user's question naturally in plain text.\n\
            Do NOT wrap your answers in JSON. Do NOT reproduce file contents verbatim unless explicitly asked.\n\
            File paths must be relative to the workspace root."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_qwen_variants() {
        let a = QwenAdapter;
        assert!(a.matches("qwen2.5-coder:14b"));
        assert!(a.matches("Qwen2-7B"));
        assert!(a.matches("qwen25-chat-local:latest"));
        assert!(!a.matches("deepseek-coder:7b"));
        assert!(!a.matches("llama3:8b"));
    }

    #[test]
    fn strips_im_tokens() {
        let a = QwenAdapter;
        let input = "<|im_start|>user\nhello world<|im_end|>".to_owned();
        let out = a.clean_content(input);
        assert!(!out.contains("<|im_start|>"));
        assert!(!out.contains("<|im_end|>"));
        assert!(out.contains("hello world"));
    }

    #[test]
    fn clean_content_trims_whitespace() {
        let a = QwenAdapter;
        let out = a.clean_content("  hello  ".to_owned());
        assert_eq!(out, "hello");
    }

    #[test]
    fn tool_instructions_contains_tool_names() {
        let a = QwenAdapter;
        let instr = a.tool_instructions("/proj", &["read_file", "write_file"]);
        assert!(instr.contains("/proj"));
        assert!(instr.contains("read_file"));
        assert!(instr.contains("write_file"));
    }
}
