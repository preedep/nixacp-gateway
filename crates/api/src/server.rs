use std::sync::Arc;

use axum::{routing::{get, post}, Router};

use crate::openai::{chat::chat_completions, models::list_models};
use crate::state::AppState;

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .with_state(state)
}
