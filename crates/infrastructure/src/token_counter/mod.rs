use std::sync::{Arc, OnceLock};

use domain::entities::message::{ContentPart, Message};
use domain::ports::token_counter::TokenCounter;
use tiktoken_rs::{cl100k_base, CoreBPE};

// BPE tables are large (~several MB) and expensive to initialise.
// OnceLock ensures the tables are loaded exactly once at first use, never per-request.
static BPE: OnceLock<Arc<CoreBPE>> = OnceLock::new();

fn bpe() -> &'static Arc<CoreBPE> {
    BPE.get_or_init(|| Arc::new(cl100k_base().expect("cl100k_base BPE must load")))
}

fn encode_len(text: &str) -> u32 {
    bpe().encode_ordinary(text).len() as u32
}

fn message_token_count(msg: &Message) -> u32 {
    msg.content
        .iter()
        .map(|p| {
            let ContentPart::Text(s) = p;
            encode_len(s)
        })
        .sum()
}

pub struct TiktokenCounter;

impl TiktokenCounter {
    pub fn new() -> Self {
        // Eagerly initialise the BPE tables on construction so the first
        // real request does not pay the ~50 ms startup cost.
        let _ = bpe();
        Self
    }
}

impl Default for TiktokenCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenCounter for TiktokenCounter {
    fn count_messages(&self, messages: &[Message]) -> u32 {
        // 4 overhead tokens per message (role + content wrapper + separators) and
        // 2 reply-priming tokens — matches OpenAI's token counting reference.
        messages
            .iter()
            .map(|m| 4 + message_token_count(m))
            .sum::<u32>()
            + 2
    }

    fn count_str(&self, text: &str) -> u32 {
        encode_len(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::entities::message::Message;

    #[test]
    fn count_str_nonempty() {
        let counter = TiktokenCounter::new();
        let n = counter.count_str("Hello, world!");
        assert!(n > 0, "expected nonzero token count");
    }

    #[test]
    fn count_messages_includes_overhead() {
        let counter = TiktokenCounter::new();
        let msgs = vec![Message::user("hi")];
        // 4 overhead + tokens("hi") + 2 reply priming
        let n = counter.count_messages(&msgs);
        assert!(n >= 6);
    }

    #[test]
    fn longer_text_more_tokens() {
        let counter = TiktokenCounter::new();
        let short = counter.count_str("hi");
        let long = counter.count_str("Hello, how are you doing today? I have a long message.");
        assert!(long > short);
    }
}
