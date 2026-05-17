use domain::entities::tool::{FunctionCall, ToolCall};
use domain::ports::llm_backend::BackendError;
use uuid::Uuid;

use crate::ollama::types::OllamaMessageContent;

/// Converts raw Ollama message content into normalized `ToolCall` structs.
///
/// Three formats are tried in priority order:
///   1. Ollama structured `tool_calls` array (OpenAI-compatible JSON objects)
///   2. Qwen XML: `<tool_call>{...}</tool_call>` blocks in content
///   3. DeepSeek markdown: ` ```json\n{...}\n``` ` fences in content
///
/// Returns `None` when the message contains no tool calls in any format.
/// Returns `Err` only when a recognized format is present but malformed.
pub struct ToolCallNormalizer;

impl ToolCallNormalizer {
    /// Extract normalized tool calls from an Ollama message, or `None` if the
    /// message is a plain text response.
    pub(crate) fn normalize(msg: &OllamaMessageContent) -> Result<Option<Vec<ToolCall>>, BackendError> {
        // Priority 1: structured tool_calls array from Ollama native format.
        if let Some(ref raw_calls) = msg.tool_calls {
            if !raw_calls.is_empty() {
                let calls = raw_calls
                    .iter()
                    .map(|c| {
                        let args_str = serde_json::to_string(&c.function.arguments)
                            .map_err(|e| BackendError::StreamParse(e.to_string()))?;
                        Ok(ToolCall {
                            id: format!("call_{}", Uuid::new_v4().simple()),
                            kind: "function".to_owned(),
                            function: FunctionCall {
                                name: c.function.name.clone(),
                                arguments: args_str,
                            },
                        })
                    })
                    .collect::<Result<Vec<_>, BackendError>>()?;
                return Ok(Some(calls));
            }
        }

        // Priority 2: Qwen XML format — <tool_call>{...}</tool_call>
        if msg.content.contains("<tool_call>") {
            let calls = parse_qwen_xml(&msg.content)?;
            if !calls.is_empty() {
                return Ok(Some(calls));
            }
            // Tag present but no valid calls → malformed
            return Err(BackendError::StreamParse(
                "found <tool_call> tag but extracted no valid calls".to_owned(),
            ));
        }

        // Priority 3: DeepSeek markdown fence — ```json\n{...}\n```
        if looks_like_deepseek_tool_call(&msg.content) {
            let calls = parse_deepseek_markdown(&msg.content)?;
            if !calls.is_empty() {
                return Ok(Some(calls));
            }
        }

        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Qwen XML parser
// ---------------------------------------------------------------------------

fn parse_qwen_xml(content: &str) -> Result<Vec<ToolCall>, BackendError> {
    let mut calls = Vec::new();
    let mut rest = content;

    while let Some(start) = rest.find("<tool_call>") {
        let after_open = &rest[start + "<tool_call>".len()..];
        let end = after_open.find("</tool_call>").ok_or_else(|| {
            BackendError::StreamParse("unclosed <tool_call> tag".to_owned())
        })?;
        let json_str = after_open[..end].trim();
        let call = parse_tool_call_json(json_str)?;
        calls.push(call);
        rest = &after_open[end + "</tool_call>".len()..];
    }

    Ok(calls)
}

// ---------------------------------------------------------------------------
// DeepSeek markdown fence parser
// ---------------------------------------------------------------------------

fn looks_like_deepseek_tool_call(content: &str) -> bool {
    // Heuristic: a json fence containing "name" and "arguments" keys.
    content.contains("```json") && content.contains("\"name\"") && content.contains("\"arguments\"")
}

fn parse_deepseek_markdown(content: &str) -> Result<Vec<ToolCall>, BackendError> {
    let mut calls = Vec::new();
    let mut rest = content;

    while let Some(fence_start) = rest.find("```json") {
        let after_fence = &rest[fence_start + "```json".len()..];
        let fence_end = after_fence.find("```").ok_or_else(|| {
            BackendError::StreamParse("unclosed markdown fence".to_owned())
        })?;
        let json_str = after_fence[..fence_end].trim();
        // Only treat as a tool call if it has both "name" and "arguments".
        if json_str.contains("\"name\"") && json_str.contains("\"arguments\"") {
            let call = parse_tool_call_json(json_str)?;
            calls.push(call);
        }
        rest = &after_fence[fence_end + "```".len()..];
    }

    Ok(calls)
}

// ---------------------------------------------------------------------------
// Shared JSON parser for both fallback formats
// ---------------------------------------------------------------------------

/// Parse a JSON object with `name` and `arguments` into a `ToolCall`.
/// `arguments` may be either a JSON object or a JSON string.
fn parse_tool_call_json(json_str: &str) -> Result<ToolCall, BackendError> {
    let v: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| BackendError::StreamParse(format!("invalid tool call JSON: {e}")))?;

    let name = v["name"]
        .as_str()
        .ok_or_else(|| BackendError::StreamParse("tool call missing 'name' field".to_owned()))?
        .to_owned();

    // `arguments` can arrive as a JSON object or as a pre-serialized string.
    let arguments = match &v["arguments"] {
        serde_json::Value::String(s) => s.clone(),
        other => serde_json::to_string(other)
            .map_err(|e| BackendError::StreamParse(e.to_string()))?,
    };

    Ok(ToolCall {
        id: format!("call_{}", Uuid::new_v4().simple()),
        kind: "function".to_owned(),
        function: FunctionCall { name, arguments },
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ollama::types::{OllamaFunction, OllamaMessageContent, OllamaToolCall};
    use serde_json::json;

    fn plain_msg(content: &str) -> OllamaMessageContent {
        OllamaMessageContent { role: "assistant".to_owned(), content: content.to_owned(), tool_calls: None }
    }

    fn structured_msg(calls: Vec<OllamaToolCall>) -> OllamaMessageContent {
        OllamaMessageContent {
            role: "assistant".to_owned(),
            content: String::new(),
            tool_calls: Some(calls),
        }
    }

    // --- Priority 1: Ollama structured format ---

    #[test]
    fn structured_single_tool_call() {
        let msg = structured_msg(vec![OllamaToolCall {
            function: OllamaFunction {
                name: "file_read".to_owned(),
                arguments: json!({"path": "src/main.rs"}),
            },
        }]);
        let calls = ToolCallNormalizer::normalize(&msg).unwrap().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "file_read");
        let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["path"], "src/main.rs");
    }

    #[test]
    fn structured_multiple_tool_calls() {
        let msg = structured_msg(vec![
            OllamaToolCall {
                function: OllamaFunction {
                    name: "file_read".to_owned(),
                    arguments: json!({"path": "a.rs"}),
                },
            },
            OllamaToolCall {
                function: OllamaFunction {
                    name: "search".to_owned(),
                    arguments: json!({"query": "async fn"}),
                },
            },
        ]);
        let calls = ToolCallNormalizer::normalize(&msg).unwrap().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "file_read");
        assert_eq!(calls[1].function.name, "search");
    }

    #[test]
    fn structured_empty_tool_calls_falls_through_to_none() {
        // empty tool_calls array → no tool calls
        let msg = OllamaMessageContent {
            role: "assistant".to_owned(),
            content: "Hello!".to_owned(),
            tool_calls: Some(vec![]),
        };
        let result = ToolCallNormalizer::normalize(&msg).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn structured_tool_call_ids_are_unique() {
        let msg = structured_msg(vec![
            OllamaToolCall {
                function: OllamaFunction { name: "f1".to_owned(), arguments: json!({}) },
            },
            OllamaToolCall {
                function: OllamaFunction { name: "f2".to_owned(), arguments: json!({}) },
            },
        ]);
        let calls = ToolCallNormalizer::normalize(&msg).unwrap().unwrap();
        assert_ne!(calls[0].id, calls[1].id, "each call must get a unique ID");
    }

    // --- Priority 2: Qwen XML format ---

    #[test]
    fn qwen_xml_single_call() {
        let content = r#"<tool_call>{"name":"file_read","arguments":{"path":"src/lib.rs"}}</tool_call>"#;
        let msg = plain_msg(content);
        let calls = ToolCallNormalizer::normalize(&msg).unwrap().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "file_read");
        let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["path"], "src/lib.rs");
    }

    #[test]
    fn qwen_xml_multiple_calls() {
        let content = concat!(
            r#"<tool_call>{"name":"file_read","arguments":{"path":"a.rs"}}</tool_call>"#,
            r#"<tool_call>{"name":"search","arguments":{"query":"todo"}}</tool_call>"#,
        );
        let msg = plain_msg(content);
        let calls = ToolCallNormalizer::normalize(&msg).unwrap().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "file_read");
        assert_eq!(calls[1].function.name, "search");
    }

    #[test]
    fn qwen_xml_arguments_as_string() {
        // Some Qwen variants emit arguments as a pre-serialized JSON string.
        let content = r#"<tool_call>{"name":"file_read","arguments":"{\"path\":\"a.rs\"}"}</tool_call>"#;
        let msg = plain_msg(content);
        let calls = ToolCallNormalizer::normalize(&msg).unwrap().unwrap();
        assert_eq!(calls.len(), 1);
        // Arguments string is the inner string value, not double-encoded.
        assert!(calls[0].function.arguments.contains("path"));
    }

    #[test]
    fn qwen_xml_unclosed_tag_returns_error() {
        let content = r#"<tool_call>{"name":"file_read","arguments":{}}"#; // no closing tag
        let msg = plain_msg(content);
        let err = ToolCallNormalizer::normalize(&msg).unwrap_err();
        assert!(matches!(err, BackendError::StreamParse(_)));
    }

    #[test]
    fn qwen_xml_invalid_json_inside_tag_returns_error() {
        let content = "<tool_call>not json at all</tool_call>";
        let msg = plain_msg(content);
        let err = ToolCallNormalizer::normalize(&msg).unwrap_err();
        assert!(matches!(err, BackendError::StreamParse(_)));
    }

    #[test]
    fn qwen_xml_missing_name_field_returns_error() {
        let content = r#"<tool_call>{"arguments":{"path":"a.rs"}}</tool_call>"#;
        let msg = plain_msg(content);
        let err = ToolCallNormalizer::normalize(&msg).unwrap_err();
        assert!(matches!(err, BackendError::StreamParse(_)));
    }

    // --- Priority 3: DeepSeek markdown fence ---

    #[test]
    fn deepseek_markdown_single_call() {
        let content = "```json\n{\"name\":\"search\",\"arguments\":{\"query\":\"async fn\"}}\n```";
        let msg = plain_msg(content);
        let calls = ToolCallNormalizer::normalize(&msg).unwrap().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "search");
        let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["query"], "async fn");
    }

    #[test]
    fn deepseek_markdown_multiple_calls() {
        let content = concat!(
            "```json\n{\"name\":\"file_read\",\"arguments\":{\"path\":\"a.rs\"}}\n```\n",
            "```json\n{\"name\":\"search\",\"arguments\":{\"query\":\"TODO\"}}\n```",
        );
        let msg = plain_msg(content);
        let calls = ToolCallNormalizer::normalize(&msg).unwrap().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "file_read");
        assert_eq!(calls[1].function.name, "search");
    }

    #[test]
    fn deepseek_markdown_fence_without_tool_fields_is_ignored() {
        // A plain code fence with no "name"/"arguments" keys → not a tool call
        let content = "```json\n{\"foo\":\"bar\"}\n```";
        let msg = plain_msg(content);
        let result = ToolCallNormalizer::normalize(&msg).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn deepseek_markdown_unclosed_fence_returns_error() {
        let content = "```json\n{\"name\":\"search\",\"arguments\":{\"query\":\"x\"}}";
        let msg = plain_msg(content);
        let err = ToolCallNormalizer::normalize(&msg).unwrap_err();
        assert!(matches!(err, BackendError::StreamParse(_)));
    }

    // --- Plain text (no tool calls) ---

    #[test]
    fn plain_text_returns_none() {
        let msg = plain_msg("Hello! How can I help you today?");
        let result = ToolCallNormalizer::normalize(&msg).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn empty_content_no_structured_calls_returns_none() {
        let msg = plain_msg("");
        let result = ToolCallNormalizer::normalize(&msg).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn structured_takes_priority_over_xml_in_content() {
        // If both structured tool_calls and XML content are present,
        // structured format wins (priority 1).
        let msg = OllamaMessageContent {
            role: "assistant".to_owned(),
            content: "<tool_call>{\"name\":\"xml_fn\",\"arguments\":{}}</tool_call>".to_owned(),
            tool_calls: Some(vec![OllamaToolCall {
                function: OllamaFunction {
                    name: "structured_fn".to_owned(),
                    arguments: json!({}),
                },
            }]),
        };
        let calls = ToolCallNormalizer::normalize(&msg).unwrap().unwrap();
        assert_eq!(calls[0].function.name, "structured_fn");
    }
}
