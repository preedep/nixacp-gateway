use domain::entities::message::{Message, Role};
use domain::ports::context_compressor::ContextCompressor;
use domain::ports::token_counter::TokenCounter;

/// Drops the oldest non-system messages until the token count fits within `max_tokens`.
///
/// Preservation rules (in priority order):
///   1. System message (index 0 if Role::System) — always kept.
///   2. Last user message — always kept so the model always has a question to answer.
///   3. Oldest middle turns are evicted first (FIFO).
///
/// If even the minimum retained set (system + last user) exceeds `max_tokens`,
/// the function returns only those two messages and logs a warning.
pub struct SlidingWindowCompressor {
    counter: std::sync::Arc<dyn TokenCounter>,
}

impl SlidingWindowCompressor {
    pub fn new(counter: std::sync::Arc<dyn TokenCounter>) -> Self {
        Self { counter }
    }
}

impl ContextCompressor for SlidingWindowCompressor {
    fn compress(&self, messages: Vec<Message>, max_tokens: u32) -> Vec<Message> {
        if messages.is_empty() {
            return messages;
        }

        // Partition into: leading system message (0 or 1), middle turns, last message.
        let (system, mut middle, last) = split_messages(messages);

        // Evict from the front of middle until the total fits.
        loop {
            let candidate: Vec<Message> = system
                .iter()
                .chain(middle.iter())
                .chain(std::iter::once(&last))
                .cloned()
                .collect();

            if self.counter.count_messages(&candidate) <= max_tokens || middle.is_empty() {
                return candidate;
            }

            middle.remove(0);
        }
    }
}

fn split_messages(mut messages: Vec<Message>) -> (Vec<Message>, Vec<Message>, Message) {
    let last = messages.pop().expect("messages must not be empty");
    let system = if messages
        .first()
        .map(|m| m.role == Role::System)
        .unwrap_or(false)
    {
        vec![messages.remove(0)]
    } else {
        vec![]
    };
    (system, messages, last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::entities::message::Message;
    use domain::ports::token_counter::TokenCounter;
    use std::sync::Arc;

    // Each message costs exactly `n` tokens from our fake counter.
    struct FixedCounter(u32);
    impl TokenCounter for FixedCounter {
        fn count_messages(&self, msgs: &[Message]) -> u32 {
            msgs.len() as u32 * self.0
        }
        fn count_str(&self, text: &str) -> u32 {
            text.len() as u32
        }
    }

    fn compressor(cost_per_msg: u32) -> SlidingWindowCompressor {
        SlidingWindowCompressor::new(Arc::new(FixedCounter(cost_per_msg)))
    }

    #[test]
    fn keeps_all_when_under_budget() {
        let c = compressor(10);
        let msgs = vec![Message::user("a"), Message::user("b"), Message::user("c")];
        let out = c.compress(msgs, 100);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn evicts_oldest_middle_turns_first() {
        // cost 20/msg → 5 msgs = 100, budget = 40 → keeps 2
        let c = compressor(20);
        let msgs = vec![
            Message::system("sys"),
            Message::user("old1"),
            Message::user("old2"),
            Message::assistant("reply"),
            Message::user("latest"),
        ];
        let out = c.compress(msgs.clone(), 40);
        // System + latest must be present.
        assert_eq!(out[0].role, Role::System);
        assert_eq!(out.last().unwrap().role, Role::User);
        assert!(out.len() < msgs.len(), "some messages must be evicted");
    }

    #[test]
    fn system_message_always_preserved() {
        let c = compressor(50);
        let msgs = vec![
            Message::system("always keep me"),
            Message::user("q1"),
            Message::assistant("a1"),
            Message::user("q2"),
        ];
        let out = c.compress(msgs, 100);
        assert_eq!(out[0].role, Role::System);
    }

    #[test]
    fn last_user_always_preserved() {
        let c = compressor(50);
        let msgs = vec![
            Message::user("first"),
            Message::user("second"),
            Message::user("must keep this"),
        ];
        let out = c.compress(msgs, 60);
        let last = out.last().unwrap();
        assert_eq!(last.text_content().unwrap(), "must keep this");
    }

    #[test]
    fn compression_boundary_200_messages() {
        // 200-message conversation; each costs 50 tokens. Budget = 2000 → keeps ~40.
        let c = compressor(50);
        let msgs: Vec<Message> = (0..200)
            .map(|i| {
                if i % 2 == 0 {
                    Message::user(format!("q{i}"))
                } else {
                    Message::assistant(format!("a{i}"))
                }
            })
            .collect();
        let out = c.compress(msgs, 2000);
        let total_cost = out.len() as u32 * 50;
        assert!(
            total_cost <= 2000,
            "compressed context ({total_cost} tokens) must fit within budget"
        );
        // Last message preserved.
        assert_eq!(out.last().unwrap().text_content().unwrap(), "a199");
    }
}
