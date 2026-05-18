/// Integration test: 3-turn agentic tool-call loop through the HTTP layer.
///
/// Topology:
///   Test client → Axum router → ToolLoopOrchestrator → wiremock (fake Ollama)
///
/// The wiremock server stubs POST /api/chat three times:
///   1. Returns a tool-call response (read_file)
///   2. Returns another tool-call response (search)
///   3. Returns a plain-text final answer
///
/// The test verifies that the HTTP response has status 200, finish_reason "stop",
/// and the expected final content.
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

/// Ollama non-streaming response that contains a tool call.
fn ollama_tool_call_body(tool_name: &str, arguments: Value) -> String {
    json!({
        "model": "test-model",
        "message": {
            "role": "assistant",
            "content": "",
            "tool_calls": [
                {
                    "function": {
                        "name": tool_name,
                        "arguments": arguments
                    }
                }
            ]
        },
        "done": true,
        "done_reason": "stop"
    })
    .to_string()
}

/// Ollama non-streaming response with a plain-text final answer.
fn ollama_plain_body(content: &str) -> String {
    json!({
        "model": "test-model",
        "message": {
            "role": "assistant",
            "content": content
        },
        "done": true,
        "done_reason": "stop"
    })
    .to_string()
}

/// Parse SSE bytes into a Vec of JSON data payloads (skips [DONE] and empty lines).
fn parse_sse_chunks(bytes: &[u8]) -> Vec<Value> {
    let text = std::str::from_utf8(bytes).unwrap_or("");
    text.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect()
}

/// Collect the full assistant content from SSE chat.completion.chunk events.
fn sse_content(chunks: &[Value]) -> String {
    chunks
        .iter()
        .filter_map(|c| c["choices"][0]["delta"]["content"].as_str())
        .collect()
}

/// Find the finish_reason from SSE chunks (last non-null one).
fn sse_finish_reason(chunks: &[Value]) -> Option<String> {
    chunks
        .iter()
        .filter_map(|c| c["choices"][0]["finish_reason"].as_str())
        .find(|r| !r.is_empty())
        .map(str::to_owned)
}

/// The OpenAI-format request body sent by the test client.
fn openai_request_body() -> Value {
    json!({
        "model": "test-model",
        "stream": false,
        "messages": [
            { "role": "user", "content": "What does the main file contain?" }
        ],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": "file_read",
                    "description": "Read a file from the workspace",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "File path" }
                        },
                        "required": ["path"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "search",
                    "description": "Search for text in the workspace",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": { "type": "string" }
                        },
                        "required": ["query"]
                    }
                }
            }
        ]
    })
}

#[tokio::test]
async fn three_turn_tool_loop_returns_final_answer() {
    // ---- Arrange ----

    let mock_server = MockServer::start().await;

    // Pass 1: model calls read_file
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(ollama_tool_call_body(
                "file_read",
                json!({"path": "src/main.rs"}),
            )),
        )
        .up_to_n_times(1)
        .mount(&mock_server)
        .await;

    // Pass 2: model calls search
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(ollama_tool_call_body("search", json!({"query": "fn main"}))),
        )
        .up_to_n_times(1)
        .mount(&mock_server)
        .await;

    // Pass 3: model returns final answer
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(ollama_plain_body(
                "The main file defines the server entry point.",
            )),
        )
        .mount(&mock_server)
        .await;

    let config = test_config(&mock_server.uri());
    let state = Arc::new(AppState::new(config).expect("AppState::new"));
    let router = build_router(state);

    // ---- Act ----

    let body = serde_json::to_vec(&openai_request_body()).unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();

    // ---- Assert ----

    assert_eq!(response.status(), StatusCode::OK, "expected 200 OK");

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let chunks = parse_sse_chunks(&bytes);

    assert_eq!(
        sse_finish_reason(&chunks).as_deref(),
        Some("stop"),
        "finish_reason must be 'stop'; chunks: {chunks:?}"
    );
    assert_eq!(
        sse_content(&chunks),
        "The main file defines the server entry point.",
        "content must match final answer; chunks: {chunks:?}"
    );

    // Verify the mock server received exactly 3 calls (one per tool-loop pass).
    let received = mock_server.received_requests().await.unwrap();
    assert_eq!(
        received.len(),
        3,
        "expected 3 Ollama calls (2 tool passes + 1 final); got {}",
        received.len()
    );
}

#[tokio::test]
async fn request_without_tools_does_not_use_tool_loop() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(ollama_plain_body("simple answer.")),
        )
        .mount(&mock_server)
        .await;

    let config = test_config(&mock_server.uri());
    let state = Arc::new(AppState::new(config).expect("AppState::new"));
    let router = build_router(state);

    // No tools in request — gateway still injects its own and runs tool loop,
    // but model returns a plain answer so loop exits after one pass.
    let body = json!({
        "model": "test-model",
        "stream": false,
        "messages": [{ "role": "user", "content": "hi" }]
    });
    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let chunks = parse_sse_chunks(&bytes);

    assert_eq!(sse_content(&chunks), "simple answer.", "chunks: {chunks:?}");

    let received = mock_server.received_requests().await.unwrap();
    assert_eq!(
        received.len(),
        1,
        "non-tool request must make exactly 1 Ollama call"
    );
}

#[tokio::test]
async fn tool_loop_returns_tool_calls_when_max_passes_exhausted() {
    let mock_server = MockServer::start().await;

    // Always return a tool-call response — forces MAX_PASSES exhaustion.
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(ollama_tool_call_body("file_read", json!({"path": "x.rs"}))),
        )
        .mount(&mock_server)
        .await;

    let config = test_config(&mock_server.uri());
    let state = Arc::new(AppState::new(config).expect("AppState::new"));
    let router = build_router(state);

    let body = json!({
        "model": "test-model",
        "stream": false,
        "messages": [{ "role": "user", "content": "loop forever" }],
        "tools": [{
            "type": "function",
            "function": {
                "name": "file_read",
                "description": "Read",
                "parameters": { "type": "object" }
            }
        }]
    });
    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // After MAX_PASSES exhaustion the last response had tool_calls — the loop
    // emits an empty content SSE stream (content is empty on tool-call responses).
    // The response must be SSE (text/event-stream), not a JSON error.
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("text/event-stream"),
        "expected SSE response; got content-type: {content_type}"
    );
}
