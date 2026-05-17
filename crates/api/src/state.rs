use std::sync::Arc;

use application::chat::ChatService;
use infrastructure::ollama::client::{OllamaClient, OllamaClientConfig};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize)]
pub struct OllamaModelConfig {
    pub name: String,
    pub display_name: Option<String>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct OllamaConfig {
    pub url: String,
    pub default_model: String,
    pub max_concurrent: usize,
    #[serde(default)]
    pub models: Vec<OllamaModelConfig>,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub ollama: OllamaConfig,
}

pub struct AppState {
    pub config: Arc<Config>,
    pub chat: ChatService,
}

impl AppState {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let ollama = Arc::new(OllamaClient::new(OllamaClientConfig {
            base_url: config.ollama.url.clone(),
            max_concurrent: config.ollama.max_concurrent,
        })?);
        let chat = ChatService::new(ollama);
        Ok(Self { config: Arc::new(config), chat })
    }
}
