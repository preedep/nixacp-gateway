/// Integration test: write_file → read_file → patch_file loop through the HTTP layer.
///
/// Topology:
///   Test client → Axum router → ToolLoopOrchestrator → wiremock (fake Ollama)
///                                                     ↕  real tempdir (actual file I/O)
///
/// The wiremock server stubs POST /api/chat three times:
///   1. Returns a tool-call response (write_file)
///   2. Returns a tool-call response (file_read)
///   3. Returns a tool-call response (patch_file)
///   4. Returns a plain-text final answer "done"
///
/// The test verifies that actual file I/O took place (write created the file,
/// patch modified it) and that the SSE stream carries finish_reason "stop"
/// with content "done".
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use api::server::build_router;
use api::state::{AppState, Config, LogConfig, OllamaConfig, ServerConfig};

// ---------------------------------------------------------------------------
// Helpers shared across tests in this file
// ---------------------------------------------------------------------------

/// Build a Config that points at the given wiremock URL and uses `workspace_root`
/// as the on-disk workspace directory.
fn test_config(ollama_url: &str, workspace_root: &str) -> Config {
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
        workspace_root: workspace_root.to_owned(),
        tools: Default::default(),
    }
}

/// Ollama non-streaming response that contains a single tool call.
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

/// Find the finish_reason from SSE chunks (first non-empty one).
fn sse_finish_reason(chunks: &[Value]) -> Option<String> {
    chunks
        .iter()
        .filter_map(|c| c["choices"][0]["finish_reason"].as_str())
        .find(|r| !r.is_empty())
        .map(str::to_owned)
}

/// OpenAI-format request body with no client-supplied tools.
/// The gateway injects its own built-in tools on every request.
fn openai_request_body() -> Value {
    json!({
        "model": "test-model",
        "stream": false,
        "messages": [
            { "role": "user", "content": "Write a file, read it back, patch it, then say done." }
        ]
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Four-turn tool loop: write_file → file_read → patch_file → plain "done".
///
/// Actual file I/O occurs in the tempdir so we can verify the file was written
/// and patched correctly by inspecting disk after the response is received.
#[tokio::test]
async fn write_then_read_then_patch() {
    // ---- Arrange ----

    let tmp: TempDir = tempfile::tempdir().expect("tempdir");
    let workspace = tmp.path().to_str().expect("tempdir path is valid UTF-8");

    let mock_server = MockServer::start().await;

    // Pass 1: model calls write_file — creates test.txt with "hello world"
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ollama_tool_call_body(
            "write_file",
            json!({ "path": "test.txt", "content": "hello world" }),
        )))
        .up_to_n_times(1)
        .mount(&mock_server)
        .await;

    // Pass 2: model calls file_read — reads test.txt back
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ollama_tool_call_body(
            "file_read",
            json!({ "path": "test.txt" }),
        )))
        .up_to_n_times(1)
        .mount(&mock_server)
        .await;

    // Pass 3: model calls patch_file — replaces "world" with "galaxy"
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ollama_tool_call_body(
            "patch_file",
            json!({
                "path": "test.txt",
                "old_str": "world",
                "new_str": "galaxy"
            }),
        )))
        .up_to_n_times(1)
        .mount(&mock_server)
        .await;

    // Pass 4: model returns final plain-text answer
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(ollama_plain_body("done")),
        )
        .mount(&mock_server)
        .await;

    let config = test_config(&mock_server.uri(), workspace);
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

    // ---- Assert: HTTP layer ----

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
        "done",
        "final content must be 'done'; chunks: {chunks:?}"
    );

    // ---- Assert: file I/O actually happened ----

    let file_path = tmp.path().join("test.txt");
    assert!(file_path.exists(), "write_file must have created test.txt");

    let contents = std::fs::read_to_string(&file_path).expect("read test.txt");
    assert_eq!(
        contents, "hello galaxy",
        "patch_file must have replaced 'world' with 'galaxy'"
    );

    // ---- Assert: Ollama received exactly 4 calls (3 tool passes + 1 final) ----

    let received = mock_server.received_requests().await.unwrap();
    assert_eq!(
        received.len(),
        4,
        "expected 4 Ollama calls (3 tool passes + 1 final); got {}",
        received.len()
    );
}
