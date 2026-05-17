use std::sync::Arc;

use anyhow::Context;
use api::{server::build_router, state::AppState};
use figment::{providers::{Env, Format, Toml}, Figment};
use logging::{
    layer::init_subscriber,
    types::{LogLevel, StdAppLog},
};
use tokio::net::TcpListener;

fn main() -> anyhow::Result<()> {
    // Config must be loaded before the subscriber so we know the log format.
    let config: api::state::Config = Figment::new()
        .merge(Toml::file("gateway.toml"))
        .merge(Env::prefixed("GATEWAY_").split("_"))
        .extract()
        .context("failed to load config from gateway.toml")?;

    init_subscriber(&config.log.format, &config.log.level);

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(num_cpus::get_physical())
        .max_blocking_threads(64)
        .thread_stack_size(512 * 1024)
        .enable_all()
        .build()
        .context("failed to build Tokio runtime")?
        .block_on(async_main(config))
}

async fn async_main(config: api::state::Config) -> anyhow::Result<()> {
    let addr = format!("{}:{}", config.server.host, config.server.port);

    let mut startup_log = StdAppLog::app(LogLevel::Info, "nixacp-gateway starting")
        .with_code_location("main");
    if let Some(id) = &config.log.app_id {
        startup_log = startup_log.with_app_id(id);
    }
    if let Some(v) = &config.log.app_version {
        startup_log = startup_log.with_app_version(v);
    }
    startup_log.emit();

    let state = Arc::new(AppState::new(config).context("failed to initialise AppState")?);
    let router = build_router(state.clone());

    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind to {addr}"))?;

    StdAppLog::app(LogLevel::Info, format!("listening on {addr}"))
        .with_code_location("main")
        .with_app_id(state.log_ctx.app_id.as_deref().unwrap_or(""))
        .emit();

    axum::serve(listener, router).await.context("server error")?;
    Ok(())
}
