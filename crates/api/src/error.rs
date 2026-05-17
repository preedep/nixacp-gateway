use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use domain::ports::llm_backend::BackendError;
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("backend error: {0}")]
    Backend(#[from] BackendError),
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::Backend(e) => (StatusCode::BAD_GATEWAY, e.to_string()),
            AppError::Internal(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        let body = Json(json!({
            "error": {
                "message": message,
                "type": "api_error"
            }
        }));
        (status, body).into_response()
    }
}
