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
use infrastructure::tools::find::{FindTool, ListTool};
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
    #[serde(default = "default_workspace_root")]
    pub workspace_root: String,
}

fn default_workspace_root() -> String {
    std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
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

        let workspace_root = std::path::PathBuf::from(&config.workspace_root);

        let system_prompt = format!(
            "You are a coding assistant with tool access. \
            The workspace root is: {workspace_root}. \
            RULES (follow exactly, no exceptions): \
            1. When asked about files or directories, call list_dir or find IMMEDIATELY. Do NOT describe what you will do first. \
            2. When asked to read a file, call file_read IMMEDIATELY. \
            3. When asked to search code, call search IMMEDIATELY. \
            4. ALWAYS use relative paths from the workspace root (e.g. \"src/main.rs\", not absolute paths). \
            5. NEVER write shell commands like `ls` or `cat` as text. Call the tool instead. \
            6. NEVER say 'Let me...' or 'I will...' before a tool call. Just call the tool. \
            Available tools: file_read, search, find, list_dir.",
            workspace_root = workspace_root.display()
        );
        let pipeline = PromptPipeline::new(
            SystemPromptBuilder::with_default(system_prompt),
            ModelQuirksTransformer,
        );
        let compression = CompressionService::new(
            counter.clone(),
            Arc::new(SlidingWindowCompressor::new(counter.clone())),
        );

        let chat = ChatService::new(ollama.clone(), pipeline, compression, counter);

        let built_in_tools: Vec<Arc<dyn ToolRuntime>> = vec![
            Arc::new(FileReadTool::new(workspace_root.clone())),
            Arc::new(SearchTool::new(workspace_root.clone(), "rg")),
            Arc::new(FindTool::new(workspace_root.clone())),
            Arc::new(ListTool::new(workspace_root.clone())),
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
