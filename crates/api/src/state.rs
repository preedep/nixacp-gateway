use std::sync::Arc;

use application::chat::ChatService;
use application::compression::{CompressionService, SlidingWindowCompressor};
use application::prompt::{ModelQuirksTransformer, PromptPipeline, SystemPromptBuilder};
use application::reflection::ReflectionOrchestrator;
use application::tool_loop::ToolLoopOrchestrator;
use domain::ports::tool_runtime::ToolRuntime;
use infrastructure::acp::AcpSessionStore;
use infrastructure::ollama::client::{OllamaClient, OllamaClientConfig};
use infrastructure::token_counter::TiktokenCounter;
use infrastructure::tools::file_read::FileReadTool;
use infrastructure::tools::search::SearchTool;
use logging::layer::LogFormat;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

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
pub struct LogConfig {
    #[serde(default)]
    pub format: LogFormat,
    #[serde(default = "default_log_level")]
    pub level: String,
    pub app_id: Option<String>,
    pub app_version: Option<String>,
}

fn default_log_level() -> String {
    "info".to_owned()
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub ollama: OllamaConfig,
    #[serde(default)]
    pub log: LogConfig,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            format: LogFormat::Json,
            level: default_log_level(),
            app_id: None,
            app_version: None,
        }
    }
}

/// Logging context derived from [LogConfig], shared via AppState.
#[derive(Debug, Clone, Default)]
pub struct LogContext {
    pub app_id: Option<String>,
    pub app_version: Option<String>,
}

pub struct AppState {
    pub config: Arc<Config>,
    pub chat: ChatService,
    pub tool_loop: ToolLoopOrchestrator,
    pub reflection: ReflectionOrchestrator,
    pub acp_sessions: AcpSessionStore,
    /// Root of the cancellation token tree. Cancel this to drain all in-flight requests.
    pub gateway_cancel: CancellationToken,
    pub log_ctx: LogContext,
}

impl AppState {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let ollama = Arc::new(OllamaClient::new(OllamaClientConfig {
            base_url: config.ollama.url.clone(),
            max_concurrent: config.ollama.max_concurrent,
        })?);

        // TiktokenCounter loads BPE tables (~50 ms) at construction — do it
        // once here so the first request pays no initialisation cost.
        let counter = Arc::new(TiktokenCounter::new());

        let pipeline = PromptPipeline::new(SystemPromptBuilder::default(), ModelQuirksTransformer);
        let compression = CompressionService::new(
            counter.clone(),
            Arc::new(SlidingWindowCompressor::new(counter.clone())),
        );

        let chat = ChatService::new(ollama.clone(), pipeline, compression, counter);

        let workspace_root = std::env::current_dir().unwrap_or_default();
        let built_in_tools: Vec<Arc<dyn ToolRuntime>> = vec![
            Arc::new(FileReadTool::new(workspace_root.clone())),
            Arc::new(SearchTool::new(workspace_root, "rg")),
        ];
        let tool_loop = ToolLoopOrchestrator::new(ollama.clone(), built_in_tools);
        let reflection = ReflectionOrchestrator::new(ollama);

        let log_ctx = LogContext {
            app_id: config.log.app_id.clone(),
            app_version: config.log.app_version.clone(),
        };
        Ok(Self {
            config: Arc::new(config),
            chat,
            tool_loop,
            reflection,
            acp_sessions: AcpSessionStore::new(),
            gateway_cancel: CancellationToken::new(),
            log_ctx,
        })
    }
}
