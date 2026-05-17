use std::collections::HashMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LogType {
    AppLog,
    ReqLog,
    ReqExLog,
    ResLog,
    ResExLog,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct LogRequest {
    pub id: String,
    pub host: String,
    pub headers: HashMap<String, String>,
    pub url: String,
    pub method: String,
    pub body: Value,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct LogResponse {
    pub status_code: u32,
    pub headers: HashMap<String, String>,
    pub body: Value,
}

/// Structured log entry conforming to Standard Application Log v1.0.
/// PII_LOG is intentionally excluded from this implementation.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StdAppLog {
    pub event_date_time: String,
    pub log_type: LogType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geo_location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_pod_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_channel_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
    pub level: LogLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_time: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<LogRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<LogResponse>,
}

impl StdAppLog {
    fn now() -> String {
        Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
    }

    pub fn app(level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            event_date_time: Self::now(),
            log_type: LogType::AppLog,
            level,
            message: Some(message.into()),
            ..Self::default_fields()
        }
    }

    pub fn req(level: LogLevel, request: LogRequest) -> Self {
        Self {
            event_date_time: Self::now(),
            log_type: LogType::ReqLog,
            level,
            request: Some(request),
            ..Self::default_fields()
        }
    }

    pub fn res(level: LogLevel, response: LogResponse) -> Self {
        Self {
            event_date_time: Self::now(),
            log_type: LogType::ResLog,
            level,
            response: Some(response),
            ..Self::default_fields()
        }
    }

    pub fn req_ex(level: LogLevel, request: LogRequest) -> Self {
        Self {
            event_date_time: Self::now(),
            log_type: LogType::ReqExLog,
            level,
            request: Some(request),
            ..Self::default_fields()
        }
    }

    pub fn res_ex(level: LogLevel, response: LogResponse) -> Self {
        Self {
            event_date_time: Self::now(),
            log_type: LogType::ResExLog,
            level,
            response: Some(response),
            ..Self::default_fields()
        }
    }

    fn default_fields() -> Self {
        Self {
            event_date_time: Self::now(),
            log_type: LogType::AppLog,
            app_id: None,
            app_version: None,
            app_address: None,
            geo_location: None,
            service_id: None,
            service_version: None,
            service_pod_name: None,
            code_location: None,
            caller_channel_name: None,
            caller_user: None,
            caller_address: None,
            correlation_id: None,
            request_id: None,
            trace_id: None,
            span_id: None,
            level: LogLevel::Info,
            execution_time: None,
            message: None,
            request: None,
            response: None,
        }
    }

    pub fn with_app_id(mut self, id: impl Into<String>) -> Self {
        self.app_id = Some(id.into());
        self
    }

    pub fn with_app_version(mut self, v: impl Into<String>) -> Self {
        self.app_version = Some(v.into());
        self
    }

    pub fn with_app_address(mut self, addr: impl Into<String>) -> Self {
        self.app_address = Some(addr.into());
        self
    }

    pub fn with_service_id(mut self, id: impl Into<String>) -> Self {
        self.service_id = Some(id.into());
        self
    }

    pub fn with_code_location(mut self, loc: impl Into<String>) -> Self {
        self.code_location = Some(loc.into());
        self
    }

    pub fn with_correlation_id(mut self, id: impl Into<String>) -> Self {
        self.correlation_id = Some(id.into());
        self
    }

    pub fn with_request_id(mut self, id: impl Into<String>) -> Self {
        self.request_id = Some(id.into());
        self
    }

    pub fn with_trace_id(mut self, id: impl Into<String>) -> Self {
        self.trace_id = Some(id.into());
        self
    }

    pub fn with_span_id(mut self, id: impl Into<String>) -> Self {
        self.span_id = Some(id.into());
        self
    }

    pub fn with_execution_time(mut self, ms: u32) -> Self {
        self.execution_time = Some(ms);
        self
    }

    pub fn with_message(mut self, msg: impl Into<String>) -> Self {
        self.message = Some(msg.into());
        self
    }

    pub fn with_caller_address(mut self, addr: impl Into<String>) -> Self {
        self.caller_address = Some(addr.into());
        self
    }

    /// Serialize to JSON and emit via `tracing::info!`.
    /// Works regardless of subscriber format (JSON or text).
    pub fn emit(self) {
        match serde_json::to_string(&self) {
            Ok(json) => tracing::info!(target: "std_app_log", "{}", json),
            Err(e) => tracing::error!("failed to serialize StdAppLog: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_log_serializes_required_fields() {
        let log = StdAppLog::app(LogLevel::Info, "started");
        let json = serde_json::to_value(&log).unwrap();
        assert_eq!(json["log_type"], "APP_LOG");
        assert_eq!(json["level"], "info");
        assert_eq!(json["message"], "started");
        assert!(json.get("pii_log").is_none());
    }

    #[test]
    fn optional_fields_omitted_when_none() {
        let log = StdAppLog::app(LogLevel::Info, "test");
        let json = serde_json::to_string(&log).unwrap();
        assert!(!json.contains("app_id"));
        assert!(!json.contains("request_id"));
        assert!(!json.contains("pii_log"));
    }

    #[test]
    fn req_log_includes_request() {
        let req = LogRequest {
            id: "req-1".into(),
            host: "localhost".into(),
            url: "/v1/chat/completions".into(),
            method: "POST".into(),
            ..Default::default()
        };
        let log = StdAppLog::req(LogLevel::Info, req)
            .with_request_id("req-1")
            .with_execution_time(42);
        let json = serde_json::to_value(&log).unwrap();
        assert_eq!(json["log_type"], "REQ_LOG");
        assert_eq!(json["execution_time"], 42);
        assert_eq!(json["request"]["method"], "POST");
    }

    #[test]
    fn log_type_screaming_snake_case() {
        assert_eq!(
            serde_json::to_string(&LogType::ReqExLog).unwrap(),
            "\"REQ_EX_LOG\""
        );
        assert_eq!(
            serde_json::to_string(&LogType::ResExLog).unwrap(),
            "\"RES_EX_LOG\""
        );
    }
}
