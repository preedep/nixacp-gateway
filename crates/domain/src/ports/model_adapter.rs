/// Per-model plugin: content cleaning, tool-call instruction style, and token floor.
///
/// Implement this trait in `infrastructure/adapters/<model>.rs` and register it in
/// `AppState::new()` to add support for a new LLM — no other files need to change.
pub trait ModelAdapter: Send + Sync {
    /// True if this adapter should handle the given model identifier string.
    fn matches(&self, model: &str) -> bool;

    /// Strip model-specific injection tokens from raw message content.
    ///
    /// Called on every message part before the payload is sent to the backend.
    /// Default: identity — no cleaning.
    fn clean_content(&self, text: String) -> String {
        text
    }

    /// Tool-call instructions appended to the system prompt for this model.
    ///
    /// `workspace_root` is the configured workspace root path.
    /// `tools` is the sorted list of registered tool names the model may call.
    fn tool_instructions(&self, workspace_root: &str, tools: &[&str]) -> String;

    /// Minimum `max_tokens` value for tool-loop requests.
    ///
    /// Some small quantised models need a lower ceiling; most need at least 4096
    /// to produce meaningful file content or search results.
    fn tool_max_tokens(&self) -> u32 {
        4096
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoAdapter;

    impl ModelAdapter for EchoAdapter {
        fn matches(&self, model: &str) -> bool {
            model == "echo"
        }

        fn tool_instructions(&self, root: &str, tools: &[&str]) -> String {
            format!("root={root} tools={}", tools.join(","))
        }
    }

    #[test]
    fn default_clean_content_is_identity() {
        let a = EchoAdapter;
        assert_eq!(a.clean_content("hello".to_owned()), "hello");
    }

    #[test]
    fn default_tool_max_tokens_is_4096() {
        let a = EchoAdapter;
        assert_eq!(a.tool_max_tokens(), 4096);
    }

    #[test]
    fn tool_instructions_receives_args() {
        let a = EchoAdapter;
        let instr = a.tool_instructions("/workspace", &["read_file", "search_files"]);
        assert!(instr.contains("/workspace"));
        assert!(instr.contains("read_file"));
    }
}
