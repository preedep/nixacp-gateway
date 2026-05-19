use domain::ports::model_adapter::ModelAdapter;

/// Adapter for DeepSeek / DeepSeek-Coder / DeepSeek-R1 models.
///
/// Strips `<think>…</think>` reasoning traces that DeepSeek-R1 leaks into
/// the assistant turn, which corrupt conversation history if forwarded verbatim.
pub struct DeepSeekAdapter;

impl ModelAdapter for DeepSeekAdapter {
    fn matches(&self, model: &str) -> bool {
        model.contains("deepseek") || model.contains("DeepSeek")
    }

    fn clean_content(&self, text: String) -> String {
        strip_think_tags(text).trim().to_owned()
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

fn strip_think_tags(text: String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_deepseek_variants() {
        let a = DeepSeekAdapter;
        assert!(a.matches("deepseek-coder:7b"));
        assert!(a.matches("DeepSeek-R1:14b"));
        assert!(a.matches("deepseek-r1-distill-qwen"));
        assert!(!a.matches("qwen2.5-coder:14b"));
        assert!(!a.matches("llama3:8b"));
    }

    #[test]
    fn strips_think_tags() {
        let a = DeepSeekAdapter;
        let input = "<think>internal reasoning</think>final answer".to_owned();
        let out = a.clean_content(input);
        assert!(!out.contains("<think>"));
        assert!(!out.contains("internal reasoning"));
        assert_eq!(out, "final answer");
    }

    #[test]
    fn unclosed_think_tag_drops_tail() {
        let a = DeepSeekAdapter;
        let out = a.clean_content("before<think>unclosed".to_owned());
        assert_eq!(out, "before");
    }

    #[test]
    fn multiple_think_blocks_stripped() {
        let a = DeepSeekAdapter;
        let input = "<think>a</think>mid<think>b</think>end".to_owned();
        let out = a.clean_content(input);
        assert_eq!(out, "midend");
    }

    #[test]
    fn tool_instructions_contains_tool_names() {
        let a = DeepSeekAdapter;
        let instr = a.tool_instructions("/ws", &["read_file", "search_files"]);
        assert!(instr.contains("/ws"));
        assert!(instr.contains("read_file"));
    }
}
