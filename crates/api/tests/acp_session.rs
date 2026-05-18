/// Integration test: ACP JSON-RPC 2.0 session lifecycle through the HTTP layer.
///
/// Topology:
///   Test client → Axum router (/acp) → AcpSessionStore + ReflectionOrchestrator → wiremock (fake Ollama)
///
/// Covers:
///   1. `initialize` — returns protocol version and agent info
///   2. `session/new` — creates a session, returns a session ID
///   3. `session/prompt` — calls the LLM, appends history, returns StopReason::EndTurn
///   4. `session/close` — removes the session
///   5. Unknown method — returns JSON-RPC method-not-found error
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use api::server::build_router;
use api::state::{AppState, Config, LogConfig, OllamaConfig, ServerConfig};

fn test_config(ollama_url: &str) -> Config {
    Config {
        server: ServerConfig {
            host: "127.0.0.1".into(),
            port: 3000,
        },
        ollama: OllamaConfig {
            url: ollama_url.to_owned(),
            default_model: "test-model".into(),
            max_concurrent: 2,
            models: vec![],
        },
        log: LogConfig::default(),
    }
}

fn ollama_plain_body(content: &str) -> String {
    json!({
        "model": "test-model",
        "message": { "role": "assistant", "content": content },
        "done": true,
        "done_reason": "stop"
    })
    .to_string()
}

async fn post_acp(router: axum::Router, body: Value) -> Value {
    let bytes = serde_json::to_vec(&body).unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/acp")
        .header("content-type", "application/json")
        .body(Body::from(bytes))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn initialize_returns_protocol_version() {
    let mock_server = MockServer::start().await;
    let config = test_config(&mock_server.uri());
    let state = Arc::new(AppState::new(config).unwrap());
    let router = build_router(state);

    let resp = post_acp(
        router,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": 1 }
        }),
    )
    .await;

    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    // Protocol version is an integer in the ACP schema.
    assert_eq!(resp["result"]["protocolVersion"], 1);
    assert_eq!(resp["result"]["agentInfo"]["name"], "nixacp-gateway");
}

#[tokio::test]
async fn session_new_returns_session_id() {
    let mock_server = MockServer::start().await;
    let config = test_config(&mock_server.uri());
    let state = Arc::new(AppState::new(config).unwrap());
    let router = build_router(state);

    let resp = post_acp(
        router,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/new",
            "params": { "cwd": "/tmp", "mcpServers": [] }
        }),
    )
    .await;

    assert_eq!(resp["jsonrpc"], "2.0");
    assert!(
        resp["result"]["sessionId"].is_string(),
        "sessionId must be a string; got: {resp}"
    );
}

#[tokio::test]
async fn session_prompt_calls_llm_and_returns_end_turn() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(ollama_plain_body("The answer is 42.")),
        )
        .mount(&mock_server)
        .await;

    let config = test_config(&mock_server.uri());
    let state = Arc::new(AppState::new(config).unwrap());
    let router = build_router(state);

    // Step 1: create a session.
    let new_resp = post_acp(
        router.clone(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "session/new",
            "params": { "cwd": "/tmp", "mcpServers": [] }
        }),
    )
    .await;
    let session_id = new_resp["result"]["sessionId"].as_str().unwrap().to_owned();

    // Step 2: send a prompt.
    let prompt_resp = post_acp(
        router,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": "What is 6 * 7?" }]
            }
        }),
    )
    .await;

    assert_eq!(prompt_resp["result"]["stopReason"], "end_turn");

    // Verify Ollama received exactly one request.
    let received = mock_server.received_requests().await.unwrap();
    assert_eq!(
        received.len(),
        1,
        "expected 1 Ollama call; got {}",
        received.len()
    );
}

#[tokio::test]
async fn session_close_removes_session() {
    let mock_server = MockServer::start().await;
    let config = test_config(&mock_server.uri());
    let state = Arc::new(AppState::new(config).unwrap());
    let router = build_router(state);

    // Create a session.
    let new_resp = post_acp(
        router.clone(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "session/new",
            "params": { "cwd": "/tmp", "mcpServers": [] }
        }),
    )
    .await;
    let session_id = new_resp["result"]["sessionId"].as_str().unwrap().to_owned();

    // Close it.
    let close_resp = post_acp(
        router.clone(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/close",
            "params": { "sessionId": session_id }
        }),
    )
    .await;
    assert!(
        close_resp["error"].is_null(),
        "close should succeed; got: {close_resp}"
    );

    // Prompt should now fail with session-not-found.
    let prompt_resp = post_acp(
        router,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": "hi" }]
            }
        }),
    )
    .await;
    assert!(
        !prompt_resp["error"].is_null(),
        "prompt after close should error; got: {prompt_resp}"
    );
}

#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let mock_server = MockServer::start().await;
    let config = test_config(&mock_server.uri());
    let state = Arc::new(AppState::new(config).unwrap());
    let router = build_router(state);

    let resp = post_acp(
        router,
        json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "no_such_method",
            "params": {}
        }),
    )
    .await;

    assert_eq!(resp["error"]["code"], -32601);
}
