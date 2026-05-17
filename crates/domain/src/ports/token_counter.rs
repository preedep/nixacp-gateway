use crate::entities::message::Message;

/// Counts tokens in a message list or a plain string.
///
/// Implementations use a best-effort approximation (cl100k_base) — the encoding
/// may not be an exact match for Qwen/DeepSeek vocabularies, but it is
/// conservative enough to avoid exceeding context windows in practice.
pub trait TokenCounter: Send + Sync {
    fn count_messages(&self, messages: &[Message]) -> u32;
    fn count_str(&self, text: &str) -> u32;
}
