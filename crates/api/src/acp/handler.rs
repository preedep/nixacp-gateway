use std::sync::Arc;

use crate::state::AppState;
use agent_client_protocol_schema::{
    CloseSessionRequest, ContentBlock, Implementation, InitializeRequest, InitializeResponse,
    NewSessionRequest, PromptRequest, PromptResponse, ProtocolVersion, SessionId, StopReason,
};
use axum::{extract::State, response::IntoResponse, Json};
use domain::entities::conversation::ConversationRequest;
use domain::entities::message::Message;
use domain::entities::model::ModelId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── JSON-RPC 2.0 envelope ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub id: Value,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcSuccess {
    jsonrpc: &'static str,
    id: Value,
    result: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    jsonrpc: &'static str,
    id: Value,
    error: RpcError,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

fn ok(id: Value, result: impl Serialize) -> Json<Value> {
    Json(
        serde_json::to_value(JsonRpcSuccess {
            jsonrpc: "2.0",
            id,
            result: serde_json::to_value(result).unwrap_or(Value::Null),
        })
        .unwrap_or(Value::Null),
    )
}

fn err(id: Value, code: i32, message: impl Into<String>) -> Json<Value> {
    Json(
        serde_json::to_value(JsonRpcError {
            jsonrpc: "2.0",
            id,
            error: RpcError {
                code,
                message: message.into(),
            },
        })
        .unwrap_or(Value::Null),
    )
}

// ── Handler ───────────────────────────────────────────────────────────────────

/// `POST /acp` — JSON-RPC 2.0 over HTTP for the Agent Client Protocol.
pub async fn acp_rpc(
    State(state): State<Arc<AppState>>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let id = req.id.clone();
    let params = req.params.clone().unwrap_or(Value::Null);

    match req.method.as_str() {
        "initialize" => handle_initialize(id, params),
        "session/new" => handle_session_new(id, params, &state),
        "session/prompt" => handle_session_prompt(id, params, &state).await,
        "session/close" => handle_session_close(id, params, &state),
        method => err(id, -32601, format!("method not found: {method}")),
    }
}

fn handle_initialize(id: Value, params: Value) -> Json<Value> {
    let Ok(req) = serde_json::from_value::<InitializeRequest>(params) else {
        return err(id, -32602, "invalid params for initialize");
    };

    let version = if req.protocol_version == ProtocolVersion::V1 {
        ProtocolVersion::V1
    } else {
        ProtocolVersion::LATEST
    };

    let response = InitializeResponse::new(version).agent_info(
        Implementation::new("nixacp-gateway", env!("CARGO_PKG_VERSION")).title("NixACP Gateway"),
    );

    ok(id, response)
}

fn handle_session_new(id: Value, params: Value, state: &AppState) -> Json<Value> {
    let Ok(req) = serde_json::from_value::<NewSessionRequest>(params) else {
        return err(id, -32602, "invalid params for session/new");
    };

    let (_session_id, response) = state.acp_sessions.create_session(req.cwd);
    ok(id, response)
}

async fn handle_session_prompt(id: Value, params: Value, state: &AppState) -> Json<Value> {
    let Ok(req) = serde_json::from_value::<PromptRequest>(params) else {
        return err(id, -32602, "invalid params for session/prompt");
    };

    let user_text: String = req
        .prompt
        .iter()
        .filter_map(|block| {
            if let ContentBlock::Text(t) = block {
                Some(t.text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    if user_text.is_empty() {
        return err(id, -32602, "prompt must contain at least one text block");
    }

    let Some((mut history, _cwd)) = state.acp_sessions.snapshot(&req.session_id) else {
        return err(id, -32001, "session not found");
    };

    let user_msg = Message::user(&user_text);
    history.push(user_msg.clone());

    let model = ModelId::new(&state.config.ollama.default_model);
    let conv_req = ConversationRequest {
        id: uuid::Uuid::new_v4(),
        model,
        messages: history,
        stream: false,
        temperature: None,
        max_tokens: None,
        tool_definitions: Vec::new(),
    };

    let cancel = state.gateway_cancel.child_token();
    match state.reflection.run(conv_req, cancel).await {
        Ok(resp) => {
            let assistant_msg = Message::assistant(&resp.content);
            state
                .acp_sessions
                .push_messages(&req.session_id, vec![user_msg, assistant_msg]);
            ok(id, PromptResponse::new(StopReason::EndTurn))
        }
        Err(e) => err(id, -32000, e.to_string()),
    }
}

fn handle_session_close(id: Value, params: Value, state: &AppState) -> Json<Value> {
    // Try the SDK schema type first; it uses camelCase field names.
    let session_id = if let Ok(req) = serde_json::from_value::<CloseSessionRequest>(params.clone())
    {
        req.session_id
    } else {
        #[derive(Deserialize)]
        struct Fallback {
            #[serde(rename = "sessionId")]
            session_id: SessionId,
        }
        match serde_json::from_value::<Fallback>(params) {
            Ok(f) => f.session_id,
            Err(_) => return err(id, -32602, "invalid params for session/close"),
        }
    };

    state.acp_sessions.remove_session(&session_id);
    ok(id, serde_json::json!({}))
}
