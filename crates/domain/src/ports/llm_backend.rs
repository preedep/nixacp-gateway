use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;
use thiserror::Error;

use crate::entities::{
    conversation::{ConversationRequest, ConversationResponse},
    model::ModelDescriptor,
    stream_chunk::StreamChunk,
};

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("model not found: {0}")]
    ModelNotFound(String),
    #[error("upstream {status}: {body}")]
    Upstream { status: u16, body: String },
    #[error("stream parse: {0}")]
    StreamParse(String),
    #[error("timeout")]
    Timeout,
}

pub type BackendStream = Pin<Box<dyn Stream<Item = Result<StreamChunk, BackendError>> + Send>>;

#[async_trait]
pub trait LlmBackend: Send + Sync {
    async fn stream(&self, request: ConversationRequest) -> Result<BackendStream, BackendError>;
    async fn complete(&self, request: ConversationRequest) -> Result<ConversationResponse, BackendError>;
    async fn list_models(&self) -> Result<Vec<ModelDescriptor>, BackendError>;
    async fn health_check(&self) -> Result<(), BackendError>;
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;

    use super::*;
    use crate::entities::{
        conversation::{ConversationRequest, ConversationResponse},
        message::Message,
        model::{ModelDescriptor, ModelId},
        stream_chunk::StreamChunk,
    };

    struct FakeBackend;

    #[async_trait]
    impl LlmBackend for FakeBackend {
        async fn stream(&self, _req: ConversationRequest) -> Result<BackendStream, BackendError> {
            let chunks: Vec<Result<StreamChunk, BackendError>> = vec![
                Ok(StreamChunk::delta("hello ")),
                Ok(StreamChunk::delta("world")),
                Ok(StreamChunk::stop()),
            ];
            Ok(Box::pin(futures_util::stream::iter(chunks)))
        }

        async fn complete(&self, req: ConversationRequest) -> Result<ConversationResponse, BackendError> {
            let mut stream = self.stream(req.clone()).await?;
            let mut content = String::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                if !chunk.is_terminal() {
                    content.push_str(&chunk.delta);
                }
            }
            Ok(ConversationResponse {
                id: req.id,
                model: req.model,
                content,
                prompt_tokens: 0,
                completion_tokens: 0,
                tool_calls: None,
            })
        }

        async fn list_models(&self) -> Result<Vec<ModelDescriptor>, BackendError> {
            Ok(vec![])
        }

        async fn health_check(&self) -> Result<(), BackendError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn fake_backend_complete_accumulates() {
        let backend = FakeBackend;
        let req = ConversationRequest::new(ModelId::new("test"), vec![Message::user("hi")]);
        let resp = backend.complete(req).await.unwrap();
        assert_eq!(resp.content, "hello world");
    }
}
