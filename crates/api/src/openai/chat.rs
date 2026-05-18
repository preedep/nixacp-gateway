use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use domain::entities::conversation::ConversationRequest;
use domain::entities::message::Message;
use domain::entities::model::ModelId;
use domain::entities::tool::{ToolCall, ToolDefinition};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;
use application::reflection::ReflectionError;

use super::types::{
    ApiFunctionCall, ApiToolCall, ApiToolDefinition, ChatCompletionChunk, ChatCompletionRequest,
    ChatCompletionResponse, ChatMessage, ChunkChoice, ChunkDelta, CompletionChoice, Usage,
};

pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatCompletionRequest>,
) -> Result<Response, AppError> {
    let messages = build_messages(&req)?;
    let tool_definitions = build_tool_definitions(&req.tools);
    let has_tools = !req.tools.is_empty();

    let domain_req = ConversationRequest {
        id: Uuid::new_v4(),
        model: ModelId::new(&req.model),
        messages,
        stream: req.stream.unwrap_or(true),
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        tool_definitions,
    };

    // Child token inherits from the gateway root; cancelling gateway_cancel
    // drains all in-flight requests at once (graceful shutdown).
    let request_cancel = state.gateway_cancel.child_token();

    if has_tools {
        tool_loop_response(state, domain_req, req.model).await
    } else if req.stream.unwrap_or(true) {
        stream_response(state, domain_req, req.model, request_cancel).await
    } else {
        complete_response(state, domain_req, req.model, request_cancel).await
    }
}

async fn tool_loop_response(
    state: Arc<AppState>,
    domain_req: ConversationRequest,
    model: String,
) -> Result<Response, AppError> {
    let resp = state.tool_loop.run(domain_req).await.map_err(|e| match e {
        application::tool_loop::ToolLoopError::Backend(be) => AppError::Backend(be),
    })?;

    let id = format!("chatcmpl-{}", resp.id.simple());
    let finish_reason = if resp.is_tool_call() {
        "tool_calls"
    } else {
        "stop"
    }
    .to_owned();
    let token_count = resp.content.split_whitespace().count() as u32;

    let message = if resp.is_tool_call() {
        let api_calls = resp
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|c| ApiToolCall {
                id: c.id,
                kind: c.kind,
                function: ApiFunctionCall {
                    name: c.function.name,
                    arguments: c.function.arguments,
                },
            })
            .collect();
        ChatMessage {
            role: "assistant".to_owned(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: Some(api_calls),
        }
    } else {
        ChatMessage {
            role: "assistant".to_owned(),
            content: resp.content,
            tool_call_id: None,
            tool_calls: None,
        }
    };

    Ok(Json(ChatCompletionResponse {
        id,
        object: "chat.completion",
        model,
        choices: vec![CompletionChoice {
            index: 0,
            message,
            finish_reason,
        }],
        usage: Usage {
            prompt_tokens: resp.prompt_tokens,
            completion_tokens: token_count,
            total_tokens: resp.prompt_tokens + token_count,
        },
    })
    .into_response())
}

/// Drop guard that cancels the request token when the stream pump task exits,
/// ensuring the cancellation propagates regardless of how the task ends
/// (normal completion, client disconnect, or panic unwind).
struct StreamGuard(CancellationToken);

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

async fn stream_response(
    state: Arc<AppState>,
    domain_req: ConversationRequest,
    model: String,
    cancel: CancellationToken,
) -> Result<Response, AppError> {
    let mut backend_stream = state.chat.stream(domain_req).await?;
    // Capacity 32: provides backpressure if the client stalls while keeping
    // memory bounded (~4 KB at ~128 B per event). Infallible satisfies axum's
    // Sse<S> bound without a custom error type.
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(32);
    let chunk_id = format!("chatcmpl-{}", Uuid::new_v4().simple());

    tokio::spawn(async move {
        // StreamGuard fires cancel on drop — covers all exit paths including
        // early returns and the normal end-of-stream path.
        let _guard = StreamGuard(cancel.clone());

        // OpenAI spec requires an initial delta with role="assistant" and no
        // content before the first token delta.
        let initial = ChatCompletionChunk {
            id: chunk_id.clone(),
            object: "chat.completion.chunk",
            model: model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: Some("assistant".to_owned()),
                    content: None,
                },
                finish_reason: None,
            }],
        };
        if let Ok(json) = serde_json::to_string(&initial) {
            // 60 s send timeout: a permanently-full channel means the client is dead.
            let send_result = tokio::time::timeout(
                Duration::from_secs(60),
                tx.send(Ok(Event::default().data(json))),
            )
            .await;
            if send_result.is_err() || send_result.unwrap().is_err() {
                return;
            }
        }

        // Pump stream chunks. Per-chunk timeout of 30 s prevents a stalled Ollama
        // from holding the stream open indefinitely without sending any bytes.
        loop {
            let next = tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                result = tokio::time::timeout(
                    Duration::from_secs(30),
                    backend_stream.next(),
                ) => result,
            };

            let chunk_opt = match next {
                Ok(opt) => opt,
                Err(_elapsed) => {
                    let _ = tx
                        .send(Ok(Event::default().data("[error] chunk timeout")))
                        .await;
                    break;
                }
            };

            let Some(result) = chunk_opt else { break };

            let event = match result {
                Ok(chunk) if chunk.is_terminal() => {
                    let finish = ChatCompletionChunk {
                        id: chunk_id.clone(),
                        object: "chat.completion.chunk",
                        model: model.clone(),
                        choices: vec![ChunkChoice {
                            index: 0,
                            delta: ChunkDelta {
                                role: None,
                                content: None,
                            },
                            finish_reason: Some("stop".to_owned()),
                        }],
                    };
                    match serde_json::to_string(&finish) {
                        Ok(json) => Ok(Event::default().data(json)),
                        Err(e) => Ok(Event::default().data(format!("[error] {e}"))),
                    }
                }
                Ok(chunk) => {
                    let delta = ChatCompletionChunk {
                        id: chunk_id.clone(),
                        object: "chat.completion.chunk",
                        model: model.clone(),
                        choices: vec![ChunkChoice {
                            index: 0,
                            delta: ChunkDelta {
                                role: None,
                                content: Some(chunk.delta),
                            },
                            finish_reason: None,
                        }],
                    };
                    match serde_json::to_string(&delta) {
                        Ok(json) => Ok(Event::default().data(json)),
                        Err(e) => Ok(Event::default().data(format!("[error] {e}"))),
                    }
                }
                Err(e) => Ok(Event::default().data(format!("[error] {e}"))),
            };

            // 60 s send timeout: a permanently-full channel means the client is dead.
            let send_result = tokio::time::timeout(Duration::from_secs(60), tx.send(event)).await;
            if send_result.is_err() || send_result.unwrap().is_err() {
                return;
            }
        }

        let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
    });

    Ok(Sse::new(ReceiverStream::new(rx))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response())
}

async fn complete_response(
    state: Arc<AppState>,
    domain_req: ConversationRequest,
    model: String,
    cancel: CancellationToken,
) -> Result<Response, AppError> {
    let resp = state
        .reflection
        .run(domain_req, cancel)
        .await
        .map_err(|e| match e {
            ReflectionError::Backend(be) => AppError::Backend(be),
            ReflectionError::Cancelled => AppError::Internal(anyhow::anyhow!("request cancelled")),
        })?;
    let id = format!("chatcmpl-{}", resp.id.simple());
    let token_count = resp.content.split_whitespace().count() as u32;

    Ok(Json(ChatCompletionResponse {
        id,
        object: "chat.completion",
        model,
        choices: vec![CompletionChoice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_owned(),
                content: resp.content,
                tool_call_id: None,
                tool_calls: None,
            },
            finish_reason: "stop".to_owned(),
        }],
        usage: Usage {
            prompt_tokens: resp.prompt_tokens,
            completion_tokens: token_count,
            total_tokens: resp.prompt_tokens + token_count,
        },
    })
    .into_response())
}

fn build_tool_definitions(tools: &[ApiToolDefinition]) -> Vec<ToolDefinition> {
    tools
        .iter()
        .map(|t| {
            ToolDefinition::function(
                &t.function.name,
                &t.function.description,
                t.function.parameters.clone(),
            )
        })
        .collect()
}

fn build_messages(req: &ChatCompletionRequest) -> Result<Vec<Message>, AppError> {
    if req.messages.is_empty() {
        return Err(AppError::BadRequest(
            "messages must not be empty".to_owned(),
        ));
    }
    Ok(req
        .messages
        .iter()
        .map(|m| match m.role.as_str() {
            "system" => Message::system(&m.content),
            "assistant" => {
                if let Some(ref calls) = m.tool_calls {
                    let domain_calls = calls
                        .iter()
                        .map(|c| ToolCall::new(&c.id, &c.function.name, &c.function.arguments))
                        .collect();
                    Message::assistant_with_tool_calls(domain_calls)
                } else {
                    Message::assistant(&m.content)
                }
            }
            "tool" => Message::tool_result(m.tool_call_id.as_deref().unwrap_or(""), &m.content),
            _ => Message::user(&m.content),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use super::super::types::{
        ApiFunctionCall, ApiFunctionDefinition, ApiToolCall, ApiToolDefinition, ChatMessage,
    };

    // --- build_messages ---

    #[test]
    fn build_messages_empty_returns_error() {
        let req = ChatCompletionRequest {
            model: "test".into(),
            messages: vec![],
            stream: None,
            temperature: None,
            max_tokens: None,
            tools: vec![],
        };
        assert!(build_messages(&req).is_err());
    }

    #[test]
    fn build_messages_system_role() {
        let req = ChatCompletionRequest {
            model: "test".into(),
            messages: vec![ChatMessage {
                role: "system".into(),
                content: "you are helpful".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            stream: None,
            temperature: None,
            max_tokens: None,
            tools: vec![],
        };
        let msgs = build_messages(&req).unwrap();
        assert_eq!(msgs[0].role, domain::entities::message::Role::System);
    }

    #[test]
    fn build_messages_tool_role_maps_tool_call_id() {
        let req = ChatCompletionRequest {
            model: "test".into(),
            messages: vec![
                ChatMessage {
                    role: "user".into(),
                    content: "hello".into(),
                    tool_call_id: None,
                    tool_calls: None,
                },
                ChatMessage {
                    role: "tool".into(),
                    content: "file contents".into(),
                    tool_call_id: Some("call_abc".into()),
                    tool_calls: None,
                },
            ],
            stream: None,
            temperature: None,
            max_tokens: None,
            tools: vec![],
        };
        let msgs = build_messages(&req).unwrap();
        assert_eq!(msgs[1].role, domain::entities::message::Role::Tool);
        assert_eq!(msgs[1].tool_call_id.as_deref(), Some("call_abc"));
        assert_eq!(msgs[1].text_content(), Some("file contents"));
    }

    #[test]
    fn build_messages_assistant_with_tool_calls() {
        let req = ChatCompletionRequest {
            model: "test".into(),
            messages: vec![ChatMessage {
                role: "assistant".into(),
                content: String::new(),
                tool_call_id: None,
                tool_calls: Some(vec![ApiToolCall {
                    id: "call_1".into(),
                    kind: "function".into(),
                    function: ApiFunctionCall {
                        name: "read_file".into(),
                        arguments: r#"{"path":"a.rs"}"#.into(),
                    },
                }]),
            }],
            stream: None,
            temperature: None,
            max_tokens: None,
            tools: vec![],
        };
        let msgs = build_messages(&req).unwrap();
        assert_eq!(msgs[0].role, domain::entities::message::Role::Assistant);
        let tool_calls = msgs[0].tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_1");
        assert_eq!(tool_calls[0].function.name, "read_file");
    }

    // --- build_tool_definitions ---

    #[test]
    fn build_tool_definitions_empty_vec() {
        let defs = build_tool_definitions(&[]);
        assert!(defs.is_empty());
    }

    #[test]
    fn build_tool_definitions_maps_fields() {
        let api_tool = ApiToolDefinition {
            kind: "function".into(),
            function: ApiFunctionDefinition {
                name: "read_file".into(),
                description: "Read a file".into(),
                parameters: json!({"type":"object","properties":{"path":{"type":"string"}}}),
            },
        };
        let defs = build_tool_definitions(&[api_tool]);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].kind, "function");
        assert_eq!(defs[0].function.name, "read_file");
        assert_eq!(defs[0].function.description, "Read a file");
        assert_eq!(defs[0].function.parameters["type"], "object");
    }

    // --- Serde round-trips ---

    #[test]
    fn api_tool_definition_serde_round_trip() {
        let t = ApiToolDefinition {
            kind: "function".into(),
            function: ApiFunctionDefinition {
                name: "search".into(),
                description: "Search the codebase".into(),
                parameters: json!({"type":"object"}),
            },
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: ApiToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, "function");
        assert_eq!(back.function.name, "search");
    }

    #[test]
    fn api_tool_call_serde_round_trip() {
        let call = ApiToolCall {
            id: "call_001".into(),
            kind: "function".into(),
            function: ApiFunctionCall {
                name: "read_file".into(),
                arguments: r#"{"path":"x"}"#.into(),
            },
        };
        let json = serde_json::to_string(&call).unwrap();
        let back: ApiToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "call_001");
        assert_eq!(back.function.name, "read_file");
    }

    #[test]
    fn chat_message_with_tool_calls_serde_round_trip() {
        let msg = ChatMessage {
            role: "assistant".into(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: Some(vec![ApiToolCall {
                id: "c1".into(),
                kind: "function".into(),
                function: ApiFunctionCall {
                    name: "search".into(),
                    arguments: "{}".into(),
                },
            }]),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: ChatMessage = serde_json::from_str(&json).unwrap();
        let calls = back.tool_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "c1");
    }

    #[test]
    fn chat_message_tool_result_serde_round_trip() {
        let msg = ChatMessage {
            role: "tool".into(),
            content: "result".into(),
            tool_call_id: Some("call_xyz".into()),
            tool_calls: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: ChatMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool_call_id.as_deref(), Some("call_xyz"));
        assert_eq!(back.content, "result");
    }

    #[test]
    fn chat_message_omits_optional_fields_when_none() {
        let msg = ChatMessage {
            role: "user".into(),
            content: "hi".into(),
            tool_call_id: None,
            tool_calls: None,
        };
        let v: serde_json::Value = serde_json::to_value(&msg).unwrap();
        assert!(v.get("tool_call_id").is_none());
        assert!(v.get("tool_calls").is_none());
    }
}
