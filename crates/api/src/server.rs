use std::sync::Arc;

use axum::{
    middleware,
    routing::{get, post},
    Router,
};

use crate::acp::handler::{acp_events, acp_rpc};
use crate::middleware::logging::log_request_response;
use crate::openai::{chat::chat_completions, models::list_models, responses::responses_handler};
use crate::state::AppState;

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        // Standard OpenAI paths (with /v1 prefix)
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        // Zed 1.2.6 omits the /v1 prefix — alias both
        .route("/chat/completions", post(chat_completions))
        .route("/models", get(list_models))
        // OpenAI Responses API — Zed agent uses this; translated to chat completions internally
        .route("/v1/responses", post(responses_handler))
        .route("/responses", post(responses_handler))
        .route("/acp", post(acp_rpc))
        .route("/acp/events", get(acp_events))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            log_request_response,
        ))
        .with_state(state)
}
