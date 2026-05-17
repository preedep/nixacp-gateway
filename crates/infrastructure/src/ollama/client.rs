use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use async_trait::async_trait;
use domain::{
    entities::{
        conversation::{ConversationRequest, ConversationResponse},
        message::ContentPart,
        model::{ModelDescriptor, ModelId},
        tool::ToolDefinition,
    },
    ports::llm_backend::{BackendError, BackendStream, LlmBackend},
};
use logging::types::{LogLevel, LogRequest, LogResponse, StdAppLog};
use serde_json::json;

use super::{
    stream,
    types::{
        OllamaChatMessage, OllamaGenerateRequest, OllamaOptions, OllamaTagsResponse, OllamaTool,
        OllamaToolFunction,
    },
};
use crate::tools::normalizer::ToolCallNormalizer;

pub struct OllamaClientConfig {
    pub base_url: String,
    pub max_concurrent: usize,
}

pub struct OllamaClient {
    http: reqwest::Client,
    config: Arc<OllamaClientConfig>,
}

impl OllamaClient {
    pub fn new(config: OllamaClientConfig) -> anyhow::Result<Self> {
        // Pool size is 2× concurrency: one idle connection per in-flight request
        // plus one spare so the next request doesn't wait for a TLS handshake.
        // tcp_nodelay eliminates the 40 ms Nagle delay on the first chunk of each
        // streaming response — critical for perceived latency.
        let http = reqwest::Client::builder()
            .pool_max_idle_per_host(config.max_concurrent * 2)
            .tcp_nodelay(true)
            .build()
            .context("failed to build reqwest client")?;
        Ok(Self {
            http,
            config: Arc::new(config),
        })
    }

    fn chat_url(&self) -> String {
        format!("{}/api/chat", self.config.base_url)
    }

    fn tags_url(&self) -> String {
        format!("{}/api/tags", self.config.base_url)
    }
}

fn domain_messages(req: &ConversationRequest) -> Vec<OllamaChatMessage> {
    req.messages
        .iter()
        .map(|m| {
            let content = m
                .content
                .iter()
                .filter_map(|p| {
                    let ContentPart::Text(s) = p;
                    Some(s.as_str())
                })
                .collect::<Vec<_>>()
                .join("");
            OllamaChatMessage {
                role: m.role.to_string(),
                content,
                tool_calls: None,
                tool_call_id: m.tool_call_id.clone(),
            }
        })
        .collect()
}

fn domain_tools(defs: &[ToolDefinition]) -> Option<Vec<OllamaTool>> {
    if defs.is_empty() {
        return None;
    }
    Some(
        defs.iter()
            .map(|d| OllamaTool {
                kind: d.kind.clone(),
                function: OllamaToolFunction {
                    name: d.function.name.clone(),
                    description: d.function.description.clone(),
                    parameters: d.function.parameters.clone(),
                },
            })
            .collect(),
    )
}

fn build_options(req: &ConversationRequest) -> Option<OllamaOptions> {
    if req.temperature.is_none() && req.max_tokens.is_none() {
        return None;
    }
    Some(OllamaOptions {
        temperature: req.temperature,
        num_predict: req.max_tokens,
    })
}

fn log_req_ex(url: &str, model: &str, msg_count: usize) {
    StdAppLog::req_ex(
        LogLevel::Debug,
        LogRequest {
            id: String::new(),
            host: String::new(),
            headers: HashMap::new(),
            url: url.to_owned(),
            method: "POST".to_owned(),
            // Log model and message count only — full content is too large for logs
            body: json!({ "model": model, "message_count": msg_count }),
        },
    )
    .with_code_location("infrastructure::ollama::client")
    .with_message(format!("Ollama {url}"))
    .emit();
}

fn log_res_ex(url: &str, status: u32, elapsed_ms: u32) {
    StdAppLog::res_ex(
        if status >= 500 {
            LogLevel::Error
        } else {
            LogLevel::Info
        },
        LogResponse {
            status_code: status,
            headers: HashMap::new(),
            body: serde_json::Value::Null,
        },
    )
    .with_code_location("infrastructure::ollama::client")
    .with_execution_time(elapsed_ms)
    .with_message(format!("Ollama {url} -> {status} ({elapsed_ms}ms)"))
    .emit();
}

#[async_trait]
impl LlmBackend for OllamaClient {
    async fn stream(&self, request: ConversationRequest) -> Result<BackendStream, BackendError> {
        let body = OllamaGenerateRequest {
            model: request.model.as_str().to_owned(),
            messages: domain_messages(&request),
            stream: true,
            options: build_options(&request),
            tools: domain_tools(&request.tool_definitions),
        };
        let url = self.chat_url();
        let msg_count = request.messages.len();

        log_req_ex(&url, request.model.as_str(), msg_count);
        let start = Instant::now();

        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| BackendError::Transport(e.to_string()))?;

        let status = response.status();
        // RES_EX_LOG is emitted here, before the error check, so the round-trip
        // time is always recorded even for error responses.
        log_res_ex(
            &url,
            status.as_u16() as u32,
            start.elapsed().as_millis() as u32,
        );

        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(BackendError::Upstream {
                status: status.as_u16(),
                body: body_text,
            });
        }

        Ok(stream::into_stream(response))
    }

    async fn complete(
        &self,
        request: ConversationRequest,
    ) -> Result<ConversationResponse, BackendError> {
        let body = OllamaGenerateRequest {
            model: request.model.as_str().to_owned(),
            messages: domain_messages(&request),
            stream: false,
            options: build_options(&request),
            tools: domain_tools(&request.tool_definitions),
        };
        let url = self.chat_url();
        let msg_count = request.messages.len();

        log_req_ex(&url, request.model.as_str(), msg_count);
        let start = Instant::now();

        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| BackendError::Transport(e.to_string()))?;

        let status = response.status();
        log_res_ex(
            &url,
            status.as_u16() as u32,
            start.elapsed().as_millis() as u32,
        );

        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(BackendError::Upstream {
                status: status.as_u16(),
                body: body_text,
            });
        }

        // Non-streaming: Ollama returns a single JSON object (the terminal
        // NDJSON line with done:true). Parse it directly to capture tool_calls
        // from the message struct — they don't appear in the delta text.
        let raw_bytes = response
            .bytes()
            .await
            .map_err(|e| BackendError::Transport(e.to_string()))?;

        let ollama_resp: crate::ollama::types::OllamaChatChunk = serde_json::from_slice(&raw_bytes)
            .map_err(|e| BackendError::StreamParse(e.to_string()))?;

        let tool_calls = ToolCallNormalizer::normalize(&ollama_resp.message)?;
        let content = ollama_resp.message.content;

        Ok(ConversationResponse {
            id: request.id,
            model: request.model,
            content,
            prompt_tokens: 0,
            completion_tokens: 0,
            tool_calls,
        })
    }

    async fn list_models(&self) -> Result<Vec<ModelDescriptor>, BackendError> {
        let response = self
            .http
            .get(&self.tags_url())
            .send()
            .await
            .map_err(|e| BackendError::Transport(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(BackendError::Upstream {
                status: status.as_u16(),
                body: body_text,
            });
        }

        let tags: OllamaTagsResponse = response
            .json()
            .await
            .map_err(|e| BackendError::Transport(e.to_string()))?;

        Ok(tags
            .models
            .into_iter()
            .map(|m| ModelDescriptor {
                id: ModelId::new(m.name),
                owned_by: "ollama".to_owned(),
                description: None,
                max_tokens: None,
            })
            .collect())
    }

    async fn health_check(&self) -> Result<(), BackendError> {
        let response = self
            .http
            .get(&self.tags_url())
            .send()
            .await
            .map_err(|e| BackendError::Transport(e.to_string()))?;

        if !response.status().is_success() {
            return Err(BackendError::Upstream {
                status: response.status().as_u16(),
                body: String::new(),
            });
        }

        Ok(())
    }
}
