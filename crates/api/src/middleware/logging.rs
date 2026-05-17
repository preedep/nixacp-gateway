use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use logging::types::{LogLevel, LogRequest, LogResponse, StdAppLog};
use uuid::Uuid;

use crate::state::AppState;

pub async fn log_request_response(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let start = Instant::now();

    // Extract tracing IDs from incoming headers
    let headers = req.headers();
    let correlation_id = headers
        .get("x-correlation-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let request_id = Uuid::new_v4().to_string();
    let caller_address = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    // Collect safe headers (strip Authorization / Cookie)
    let safe_headers: HashMap<String, String> = headers
        .iter()
        .filter(|(name, _)| {
            let n = name.as_str().to_lowercase();
            n != "authorization" && n != "cookie"
        })
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.to_string(), v.to_owned()))
        })
        .collect();

    let method = req.method().to_string();
    let uri = req.uri().to_string();
    let host = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();

    // REQ_LOG
    let mut req_log = StdAppLog::req(
        LogLevel::Info,
        LogRequest {
            id: request_id.clone(),
            host: host.clone(),
            headers: safe_headers.clone(),
            url: uri.clone(),
            method: method.clone(),
            body: serde_json::Value::Null,
        },
    )
    .with_correlation_id(&correlation_id)
    .with_request_id(&request_id)
    .with_message(format!("{method} {uri}"))
    .with_code_location("api::middleware::logging");

    if let Some(id) = &state.log_ctx.app_id {
        req_log = req_log.with_app_id(id);
    }
    if let Some(v) = &state.log_ctx.app_version {
        req_log = req_log.with_app_version(v);
    }
    if let Some(addr) = &caller_address {
        req_log = req_log.with_caller_address(addr);
    }
    req_log.emit();

    // Run the handler
    let mut response = next.run(req).await;
    let elapsed_ms = start.elapsed().as_millis() as u32;
    let status = response.status().as_u16() as u32;

    // Inject X-Request-Id into response headers
    if let Ok(val) = request_id.parse() {
        response.headers_mut().insert("x-request-id", val);
    }

    // Collect safe response headers
    let res_headers: HashMap<String, String> = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.to_string(), v.to_owned()))
        })
        .collect();

    // RES_LOG
    let mut res_log = StdAppLog::res(
        if status >= 500 {
            LogLevel::Error
        } else {
            LogLevel::Info
        },
        LogResponse {
            status_code: status,
            headers: res_headers,
            body: serde_json::Value::Null,
        },
    )
    .with_correlation_id(&correlation_id)
    .with_request_id(&request_id)
    .with_execution_time(elapsed_ms)
    .with_message(format!("{method} {uri} -> {status} ({elapsed_ms}ms)"))
    .with_code_location("api::middleware::logging");

    if let Some(id) = &state.log_ctx.app_id {
        res_log = res_log.with_app_id(id);
    }
    if let Some(v) = &state.log_ctx.app_version {
        res_log = res_log.with_app_version(v);
    }
    res_log.emit();

    response
}
