use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Bytes,
    extract::State,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::state::AppState;
use domain::entities::conversation::ConversationRequest;
use domain::entities::message::Message;
use domain::entities::model::ModelId;
use domain::entities::tool::ToolCall;

// ── OpenAI Responses API wire types ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ResponsesRequest {
    pub model: String,
    #[serde(default)]
    pub input: ResponsesInput,
    pub stream: Option<bool>,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub tools: Vec<Value>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(untagged)]
pub enum ResponsesInput {
    Text(String),
    Items(Vec<ResponsesInputItem>),
    #[default]
    Empty,
}

#[derive(Debug, Deserialize)]
pub struct ResponsesInputItem {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub content: Value,
    #[serde(default)]
    pub call_id: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    // function_call fields
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
}

// ── Input → domain messages ───────────────────────────────────────────────────

fn input_to_messages(input: ResponsesInput) -> Vec<Message> {
    match input {
        ResponsesInput::Text(text) => vec![Message::user(&text)],
        ResponsesInput::Items(items) => items
            .into_iter()
            .filter_map(|item| match item.kind.as_str() {
                "message" => {
                    let text = extract_text(&item.content);
                    Some(match item.role.as_str() {
                        "system" => Message::system(&text),
                        "assistant" => Message::assistant(&text),
                        _ => Message::user(&text),
                    })
                }
                "function_call" => {
                    let name = item.name.unwrap_or_default();
                    let arguments = item.arguments.unwrap_or_else(|| "{}".into());
                    let call_id = item.id.unwrap_or_default();
                    Some(Message::assistant_with_tool_calls(vec![ToolCall::new(
                        &call_id, &name, &arguments,
                    )]))
                }
                "function_call_output" => {
                    let call_id = item.call_id.as_deref().unwrap_or("");
                    let output = item.output.as_deref().unwrap_or("");
                    Some(Message::tool_result(call_id, output))
                }
                _ => None,
            })
            .collect(),
        ResponsesInput::Empty => vec![],
    }
}

fn extract_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                    b.get("text").and_then(|t| t.as_str()).map(str::to_owned)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

// ── Handler ───────────────────────────────────────────────────────────────────

/// `POST /responses` — OpenAI Responses API, streamed back in Responses API SSE format.
pub async fn responses_handler(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let req: ResponsesRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("responses: failed to parse body: {e}");
            return (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(json!({
                    "error": {"message": format!("bad request: {e}"), "type": "invalid_request_error"}
                })),
            )
                .into_response();
        }
    };

    let model = state.config.ollama.default_model.clone();
    let messages = input_to_messages(req.input);

    if messages.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(json!({
                "error": {"message": "input must not be empty", "type": "invalid_request_error"}
            })),
        )
            .into_response();
    }

    // Only run the tool loop when the conversation needs a new model response.
    // If the last message is already an assistant message, Zed is delivering
    // context from a prior turn — echo it back without re-calling the model.
    let needs_completion = messages
        .last()
        .map(|m| m.role != domain::entities::message::Role::Assistant)
        .unwrap_or(true);

    // Always inject gateway tools — ignore what the client sent (Zed sends its
    // own editor tools we cannot execute).
    let tool_definitions = state.tool_loop.tool_definitions();

    let domain_req = ConversationRequest {
        id: Uuid::new_v4(),
        model: ModelId::new(&model),
        messages,
        stream: true,
        temperature: req.temperature,
        max_tokens: req.max_output_tokens,
        tool_definitions,
    };

    if needs_completion {
        responses_tool_loop(state, domain_req, model).await
    } else {
        // Last message is already assistant — echo it back so Zed renders it.
        let last_text = domain_req
            .messages
            .last()
            .and_then(|m| m.text_content())
            .map(str::to_owned)
            .unwrap_or_default();
        responses_echo(last_text, model).await
    }
}

async fn responses_tool_loop(
    state: Arc<AppState>,
    domain_req: ConversationRequest,
    model: String,
) -> Response {
    let resp = match state.tool_loop.run(domain_req).await {
        Ok(r) => r,
        Err(e) => {
            return (
                axum::http::StatusCode::BAD_GATEWAY,
                axum::Json(json!({"error": {"message": e.to_string(), "type": "upstream_error"}})),
            )
                .into_response();
        }
    };

    let response_id = format!("resp_{}", resp.id.simple());
    let item_id = format!("msg_{}", resp.id.simple());
    let full_text = resp.content.clone();

    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(32);

    tokio::spawn(async move {
        let send = |event: Value| {
            let tx = tx.clone();
            async move {
                let data = serde_json::to_string(&event).unwrap_or_default();
                let _ = tokio::time::timeout(
                    Duration::from_secs(60),
                    tx.send(Ok(Event::default().data(data))),
                )
                .await;
            }
        };

        send(json!({
            "type": "response.created",
            "response": {"id": response_id, "object": "response", "model": model, "status": "in_progress"}
        }))
        .await;

        send(json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {"id": item_id, "type": "message", "role": "assistant", "content": []}
        }))
        .await;

        send(json!({
            "type": "response.content_part.added",
            "output_index": 0,
            "content_index": 0,
            "part": {"type": "output_text", "text": ""}
        }))
        .await;

        send(json!({
            "type": "response.output_text.delta",
            "output_index": 0,
            "content_index": 0,
            "delta": full_text
        }))
        .await;

        send(json!({
            "type": "response.output_text.done",
            "output_index": 0,
            "content_index": 0,
            "text": full_text
        }))
        .await;

        send(json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "id": item_id,
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": full_text}]
            }
        }))
        .await;

        send(json!({
            "type": "response.completed",
            "response": {
                "id": response_id,
                "object": "response",
                "model": model,
                "status": "completed",
                "output": [{
                    "id": item_id,
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": full_text}]
                }]
            }
        }))
        .await;
    });

    Sse::new(ReceiverStream::new(rx))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

/// Echo an already-completed assistant turn back to Zed without calling the model.
/// Used when `/responses` receives a conversation whose last message is already
/// an assistant message — Zed is delivering context, not requesting new generation.
async fn responses_echo(text: String, model: String) -> Response {
    let response_id = format!("resp_{}", Uuid::new_v4().simple());
    let item_id = format!("msg_{}", Uuid::new_v4().simple());

    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(8);
    tokio::spawn(async move {
        let send = |event: Value| {
            let tx = tx.clone();
            async move {
                let data = serde_json::to_string(&event).unwrap_or_default();
                let _ = tx.send(Ok(Event::default().data(data))).await;
            }
        };

        send(json!({
            "type": "response.created",
            "response": {"id": response_id, "object": "response", "model": model, "status": "in_progress"}
        })).await;
        send(json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {"id": item_id, "type": "message", "role": "assistant", "content": []}
        })).await;
        send(json!({
            "type": "response.content_part.added",
            "output_index": 0, "content_index": 0,
            "part": {"type": "output_text", "text": ""}
        })).await;
        send(json!({
            "type": "response.output_text.delta",
            "output_index": 0, "content_index": 0,
            "delta": text
        })).await;
        send(json!({
            "type": "response.output_text.done",
            "output_index": 0, "content_index": 0,
            "text": text
        })).await;
        send(json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {"id": item_id, "type": "message", "role": "assistant",
                     "content": [{"type": "output_text", "text": text}]}
        })).await;
        send(json!({
            "type": "response.completed",
            "response": {
                "id": response_id, "object": "response", "model": model, "status": "completed",
                "output": [{"id": item_id, "type": "message", "role": "assistant",
                            "content": [{"type": "output_text", "text": text}]}]
            }
        })).await;
    });

    Sse::new(ReceiverStream::new(rx))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}
