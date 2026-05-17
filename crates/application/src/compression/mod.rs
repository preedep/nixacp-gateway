mod sliding_window;

pub use sliding_window::SlidingWindowCompressor;

use std::sync::Arc;

use domain::entities::message::Message;
use domain::ports::context_compressor::ContextCompressor;
use domain::ports::token_counter::TokenCounter;

/// Applies context compression when the message list exceeds the model's token budget.
pub struct CompressionService {
    counter: Arc<dyn TokenCounter>,
    compressor: Arc<dyn ContextCompressor>,
}

impl CompressionService {
    pub fn new(counter: Arc<dyn TokenCounter>, compressor: Arc<dyn ContextCompressor>) -> Self {
        Self {
            counter,
            compressor,
        }
    }

    /// Returns messages trimmed to fit within `max_tokens`, or the original list
    /// unchanged if it already fits.
    pub fn maybe_compress(&self, messages: Vec<Message>, max_tokens: u32) -> Vec<Message> {
        let used = self.counter.count_messages(&messages);
        if used <= max_tokens {
            return messages;
        }
        tracing::debug!(
            used_tokens = used,
            max_tokens,
            "context exceeds budget, compressing"
        );
        self.compressor.compress(messages, max_tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::entities::message::Message;
    use domain::ports::context_compressor::ContextCompressor;
    use domain::ports::token_counter::TokenCounter;

    struct FakeCounter(u32);
    impl TokenCounter for FakeCounter {
        fn count_messages(&self, _: &[Message]) -> u32 {
            self.0
        }
        fn count_str(&self, text: &str) -> u32 {
            text.len() as u32
        }
    }

    struct FakeCompressor;
    impl ContextCompressor for FakeCompressor {
        fn compress(&self, mut messages: Vec<Message>, _max: u32) -> Vec<Message> {
            messages.truncate(1);
            messages
        }
    }

    #[test]
    fn no_compression_when_under_budget() {
        let svc = CompressionService::new(Arc::new(FakeCounter(10)), Arc::new(FakeCompressor));
        let msgs = vec![Message::user("a"), Message::user("b")];
        let out = svc.maybe_compress(msgs, 100);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn compresses_when_over_budget() {
        let svc = CompressionService::new(Arc::new(FakeCounter(200)), Arc::new(FakeCompressor));
        let msgs = vec![Message::user("a"), Message::user("b")];
        let out = svc.maybe_compress(msgs, 100);
        assert_eq!(out.len(), 1);
    }
}
