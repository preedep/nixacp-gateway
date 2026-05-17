use std::sync::Arc;

use anyhow::Context;
use api::{server::build_router, state::AppState};
use figment::{providers::{Env, Format, Toml}, Figment};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(num_cpus::get_physical())
        .max_blocking_threads(64)
        .thread_stack_size(512 * 1024)
        .enable_all()
        .build()
        .context("failed to build Tokio runtime")?
        .block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    let config: api::state::Config = Figment::new()
        .merge(Toml::file("gateway.toml"))
        .merge(Env::prefixed("GATEWAY_").split("_"))
        .extract()
        .context("failed to load config from gateway.toml")?;

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let state = Arc::new(AppState::new(config).context("failed to initialise AppState")?);
    let router = build_router(state);

    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind to {addr}"))?;

    tracing::info!("nixacp-gateway listening on {addr}");
    axum::serve(listener, router).await.context("server error")?;
    Ok(())
}
