use crate::entities::message::Message;

/// Trims a message list to fit within a token budget.
///
/// Implementations must always preserve the system message (index 0 if Role::System)
/// and the last user message; only middle turns are eligible for eviction.
pub trait ContextCompressor: Send + Sync {
    /// Returns a new message list that fits within `max_tokens`.
    /// Panics only if the system + last-user messages alone exceed `max_tokens`.
    fn compress(&self, messages: Vec<Message>, max_tokens: u32) -> Vec<Message>;
}
