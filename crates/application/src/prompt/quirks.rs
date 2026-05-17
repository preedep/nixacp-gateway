use domain::entities::message::{ContentPart, Message};

/// Strips model-specific injection tokens that leak into user messages.
///
/// Qwen models sometimes surface `<|im_start|>` / `<|im_end|>` chat-template
/// tokens in the raw user input. DeepSeek-R1 leaks `<think>…</think>` reasoning
/// blocks. Both corrupt the conversation history if forwarded verbatim.
#[derive(Default)]
pub struct ModelQuirksTransformer;

impl ModelQuirksTransformer {
    pub fn apply(&self, model: &str, messages: Vec<Message>) -> Vec<Message> {
        messages
            .into_iter()
            .map(|mut msg| {
                msg.content = msg
                    .content
                    .into_iter()
                    .map(|part| {
                        let ContentPart::Text(text) = part;
                        ContentPart::Text(clean(model, text))
                    })
                    .collect();
                msg
            })
            .collect()
    }
}

fn clean(model: &str, mut text: String) -> String {
    // Qwen chat-template tokens that must never appear in the wire payload.
    if model.contains("qwen") || model.contains("Qwen") {
        text = text.replace("<|im_start|>", "");
        text = text.replace("<|im_end|>", "");
    }

    // DeepSeek-R1 reasoning traces — strip entire <think>…</think> blocks.
    if model.contains("deepseek") || model.contains("DeepSeek") {
        text = strip_think_tags(text);
    }

    text.trim().to_owned()
}

fn strip_think_tags(text: String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => {
                // Unclosed tag — drop everything after <think>.
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::entities::message::Message;

    #[test]
    fn strips_qwen_tokens() {
        let t = ModelQuirksTransformer::default();
        let msgs = vec![Message::user("<|im_start|>user\nhello<|im_end|>")];
        let out = t.apply("qwen2.5-coder:14b", msgs);
        let text = out[0].text_content().unwrap();
        assert!(!text.contains("<|im_start|>"));
        assert!(!text.contains("<|im_end|>"));
        assert!(text.contains("hello"));
    }

    #[test]
    fn strips_deepseek_think_tags() {
        let t = ModelQuirksTransformer::default();
        let msgs = vec![Message::user("<think>reasoning</think>final answer")];
        let out = t.apply("deepseek-coder:7b", msgs);
        let text = out[0].text_content().unwrap();
        assert!(!text.contains("<think>"));
        assert!(!text.contains("reasoning"));
        assert_eq!(text, "final answer");
    }

    #[test]
    fn no_change_for_unknown_model() {
        let t = ModelQuirksTransformer::default();
        let msgs = vec![Message::user("hello world")];
        let out = t.apply("llama3:8b", msgs);
        assert_eq!(out[0].text_content().unwrap(), "hello world");
    }

    #[test]
    fn unclosed_think_tag_drops_tail() {
        let t = ModelQuirksTransformer::default();
        let msgs = vec![Message::user("before<think>unclosed")];
        let out = t.apply("deepseek-r1:7b", msgs);
        let text = out[0].text_content().unwrap();
        assert_eq!(text, "before");
    }
}
