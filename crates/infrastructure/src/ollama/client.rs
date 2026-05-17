use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use domain::{
    entities::{
        conversation::{ConversationRequest, ConversationResponse},
        message::ContentPart,
        model::{ModelDescriptor, ModelId},
    },
    ports::llm_backend::{BackendError, BackendStream, LlmBackend},
};
use futures_util::StreamExt;

use super::{
    stream,
    types::{OllamaChatMessage, OllamaGenerateRequest, OllamaOptions, OllamaTagsResponse},
};

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
        let http = reqwest::Client::builder()
            .pool_max_idle_per_host(config.max_concurrent * 2)
            .tcp_nodelay(true)
            .build()
            .context("failed to build reqwest client")?;
        Ok(Self { http, config: Arc::new(config) })
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
            OllamaChatMessage { role: m.role.to_string(), content }
        })
        .collect()
}


#[async_trait]
impl LlmBackend for OllamaClient {
    async fn stream(&self, request: ConversationRequest) -> Result<BackendStream, BackendError> {
        let body = OllamaGenerateRequest {
            model: request.model.as_str().to_owned(),
            messages: domain_messages(&request),
            stream: true,
            options: build_options(&request),
        };

        let response = self
            .http
            .post(&self.chat_url())
            .json(&body)
            .send()
            .await
            .map_err(|e| BackendError::Transport(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(BackendError::Upstream { status: status.as_u16(), body: body_text });
        }

        Ok(stream::into_stream(response))
    }

    async fn complete(&self, request: ConversationRequest) -> Result<ConversationResponse, BackendError> {
        let body = OllamaGenerateRequest {
            model: request.model.as_str().to_owned(),
            messages: domain_messages(&request),
            stream: false,
            options: build_options(&request),
        };

        let response = self
            .http
            .post(&self.chat_url())
            .json(&body)
            .send()
            .await
            .map_err(|e| BackendError::Transport(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(BackendError::Upstream { status: status.as_u16(), body: body_text });
        }

        let mut stream = stream::into_stream(response);
        let mut content = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if !chunk.is_terminal() {
                content.push_str(&chunk.delta);
            }
        }

        Ok(ConversationResponse {
            id: request.id,
            model: request.model,
            content,
            prompt_tokens: 0,
            completion_tokens: 0,
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
            return Err(BackendError::Upstream { status: status.as_u16(), body: body_text });
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

fn build_options(req: &ConversationRequest) -> Option<OllamaOptions> {
    if req.temperature.is_none() && req.max_tokens.is_none() {
        return None;
    }
    Some(OllamaOptions {
        temperature: req.temperature,
        num_predict: req.max_tokens,
    })
}
