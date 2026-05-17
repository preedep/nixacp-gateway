use std::sync::Arc;

use axum::extract::State;
use axum::Json;

use crate::state::AppState;

use super::types::{ModelListResponse, ModelObject};

pub async fn list_models(State(state): State<Arc<AppState>>) -> Json<ModelListResponse> {
    let data = if state.config.ollama.models.is_empty() {
        vec![ModelObject {
            id: state.config.ollama.default_model.clone(),
            object: "model",
            owned_by: "ollama".to_owned(),
        }]
    } else {
        state
            .config
            .ollama
            .models
            .iter()
            .map(|m| ModelObject {
                id: m.name.clone(),
                object: "model",
                owned_by: "ollama".to_owned(),
            })
            .collect()
    };

    Json(ModelListResponse {
        object: "list",
        data,
    })
}
