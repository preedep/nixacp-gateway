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
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

use super::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ChatMessage, ChunkChoice,
    ChunkDelta, CompletionChoice, Usage,
};

pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatCompletionRequest>,
) -> Result<Response, AppError> {
    let messages = build_messages(&req)?;
    let domain_req = ConversationRequest {
        id: Uuid::new_v4(),
        model: ModelId::new(&req.model),
        messages,
        stream: req.stream.unwrap_or(true),
        temperature: req.temperature,
        max_tokens: req.max_tokens,
    };

    if req.stream.unwrap_or(true) {
        stream_response(state, domain_req, req.model).await
    } else {
        complete_response(state, domain_req, req.model).await
    }
}

async fn stream_response(
    state: Arc<AppState>,
    domain_req: ConversationRequest,
    model: String,
) -> Result<Response, AppError> {
    let mut backend_stream = state.chat.stream(domain_req).await?;
    // Capacity 32: provides backpressure if the client stalls while keeping
    // memory bounded (~4 KB at ~128 B per event). Infallible satisfies axum's
    // Sse<S> bound without a custom error type.
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(32);
    let chunk_id = format!("chatcmpl-{}", Uuid::new_v4().simple());

    tokio::spawn(async move {
        // OpenAI spec requires an initial delta with role="assistant" and no
        // content before the first token delta.
        // Send initial role delta
        let initial = ChatCompletionChunk {
            id: chunk_id.clone(),
            object: "chat.completion.chunk",
            model: model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta { role: Some("assistant".to_owned()), content: None },
                finish_reason: None,
            }],
        };
        if let Ok(json) = serde_json::to_string(&initial) {
            if tx.send(Ok(Event::default().data(json))).await.is_err() {
                return;
            }
        }

        // Pump stream chunks
        while let Some(result) = backend_stream.next().await {
            let event = match result {
                Ok(chunk) if chunk.is_terminal() => {
                    let finish = ChatCompletionChunk {
                        id: chunk_id.clone(),
                        object: "chat.completion.chunk",
                        model: model.clone(),
                        choices: vec![ChunkChoice {
                            index: 0,
                            delta: ChunkDelta { role: None, content: None },
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
                            delta: ChunkDelta { role: None, content: Some(chunk.delta) },
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

            // tx.send error means the receiver (SSE body) was dropped — the
            // client disconnected. Stop pumping to avoid driving Ollama for nothing.
            if tx.send(event).await.is_err() {
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
) -> Result<Response, AppError> {
    let resp = state.chat.complete(domain_req).await?;
    let id = format!("chatcmpl-{}", resp.id.simple());
    let token_count = resp.content.split_whitespace().count() as u32;

    Ok(Json(ChatCompletionResponse {
        id,
        object: "chat.completion",
        model,
        choices: vec![CompletionChoice {
            index: 0,
            message: ChatMessage { role: "assistant".to_owned(), content: resp.content },
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

fn build_messages(req: &ChatCompletionRequest) -> Result<Vec<Message>, AppError> {
    if req.messages.is_empty() {
        return Err(AppError::BadRequest("messages must not be empty".to_owned()));
    }
    Ok(req
        .messages
        .iter()
        .map(|m| match m.role.as_str() {
            "system" => Message::system(&m.content),
            "assistant" => Message::assistant(&m.content),
            _ => Message::user(&m.content),
        })
        .collect())
}
