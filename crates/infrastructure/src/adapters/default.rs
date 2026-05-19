use domain::ports::model_adapter::ModelAdapter;

/// Fallback adapter for any model not matched by a more specific adapter.
///
/// No content cleaning; generic tool-call instructions compatible with
/// models that follow standard OpenAI function-calling conventions.
pub struct DefaultAdapter;

impl ModelAdapter for DefaultAdapter {
    fn matches(&self, _model: &str) -> bool {
        true
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
    fn matches_any_model() {
        let a = DefaultAdapter;
        assert!(a.matches("llama3:8b"));
        assert!(a.matches("mistral:7b"));
        assert!(a.matches("phi3:mini"));
        assert!(a.matches(""));
    }

    #[test]
    fn clean_content_is_identity() {
        let a = DefaultAdapter;
        assert_eq!(a.clean_content("hello".to_owned()), "hello");
        assert_eq!(a.clean_content("<|im_start|>".to_owned()), "<|im_start|>");
    }

    #[test]
    fn tool_instructions_contains_workspace_and_tools() {
        let a = DefaultAdapter;
        let instr = a.tool_instructions("/home/user/proj", &["read_file", "list_directory"]);
        assert!(instr.contains("/home/user/proj"));
        assert!(instr.contains("read_file"));
        assert!(instr.contains("list_directory"));
    }
}
