use std::sync::Arc;

use axum::{middleware, routing::{get, post}, Router};

use crate::middleware::logging::log_request_response;
use crate::openai::{chat::chat_completions, models::list_models};
use crate::state::AppState;

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .layer(middleware::from_fn_with_state(state.clone(), log_request_response))
        .with_state(state)
}
