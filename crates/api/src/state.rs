use std::sync::Arc;

use application::chat::ChatService;
use application::compression::{CompressionService, SlidingWindowCompressor};
use application::prompt::{ModelAdapterRegistry, ModelQuirksTransformer, PromptPipeline, SystemPromptBuilder};
use application::reflection::ReflectionOrchestrator;
use application::tool_loop::ToolLoopOrchestrator;
use domain::ports::tool_runtime::ToolRuntime;
use infrastructure::acp::AcpSessionStore;
use infrastructure::adapters::{DeepSeekAdapter, DefaultAdapter, QwenAdapter};
use infrastructure::ollama::client::{OllamaClient, OllamaClientConfig};
use infrastructure::token_counter::TiktokenCounter;
use infrastructure::tools::bash::BashTool;
use infrastructure::tools::file_read::ReadFileTool;
use infrastructure::tools::find::{FindTool, ListTool};
use infrastructure::tools::patch_file::PatchFileTool;
use infrastructure::tools::search::SearchFilesTool;
use infrastructure::tools::write_file::WriteFileTool;
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
pub struct BashConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_bash_allowlist")]
    pub allowlist: Vec<String>,
}

fn default_bash_allowlist() -> Vec<String> {
    vec!["cargo".into(), "git".into(), "echo".into()]
}

impl Default for BashConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowlist: default_bash_allowlist(),
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ToolsConfig {
    #[serde(default)]
    pub bash: BashConfig,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub ollama: OllamaConfig,
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default = "default_workspace_root")]
    pub workspace_root: String,
    #[serde(default)]
    pub tools: ToolsConfig,
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

        // Build tools first so we can pass their names to SystemPromptBuilder.
        let mut built_in_tools: Vec<Arc<dyn ToolRuntime>> = vec![
            Arc::new(ReadFileTool::new(workspace_root.clone())),
            Arc::new(WriteFileTool::new(workspace_root.clone())),
            Arc::new(PatchFileTool::new(workspace_root.clone())),
            Arc::new(SearchFilesTool::new(workspace_root.clone(), "rg")),
            Arc::new(FindTool::new(workspace_root.clone())),
            Arc::new(ListTool::new(workspace_root.clone())),
        ];
        if config.tools.bash.enabled {
            built_in_tools.push(Arc::new(BashTool::new(
                workspace_root.clone(),
                config.tools.bash.allowlist.clone(),
            )));
        }
        let tool_names: Vec<String> = built_in_tools.iter().map(|t| t.name().to_owned()).collect();

        // Model adapter registry — checked in order; DefaultAdapter must be last.
        let adapter_registry = Arc::new(ModelAdapterRegistry::new(vec![
            Arc::new(QwenAdapter),
            Arc::new(DeepSeekAdapter),
            Arc::new(DefaultAdapter),
        ]));

        let pipeline = PromptPipeline::new(
            SystemPromptBuilder::new(
                adapter_registry.clone(),
                workspace_root.to_string_lossy().as_ref(),
                tool_names,
            ),
            ModelQuirksTransformer::new(adapter_registry),
        );
        let compression = CompressionService::new(
            counter.clone(),
            Arc::new(SlidingWindowCompressor::new(counter.clone())),
        );

        let chat = ChatService::new(ollama.clone(), pipeline, compression, counter);
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
