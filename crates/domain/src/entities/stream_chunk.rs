use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
}

impl std::fmt::Display for FinishReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stop => write!(f, "stop"),
            Self::Length => write!(f, "length"),
            Self::ToolCalls => write!(f, "tool_calls"),
            Self::ContentFilter => write!(f, "content_filter"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    /// Token delta. Phase 6 will change this to `smol_str::SmolStr`.
    pub delta: String,
    pub finish_reason: Option<FinishReason>,
}

impl StreamChunk {
    pub fn delta(text: impl Into<String>) -> Self {
        Self { delta: text.into(), finish_reason: None }
    }

    pub fn stop() -> Self {
        Self { delta: String::new(), finish_reason: Some(FinishReason::Stop) }
    }

    pub fn is_terminal(&self) -> bool {
        self.finish_reason.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_not_terminal() {
        assert!(!StreamChunk::delta("hi").is_terminal());
    }

    #[test]
    fn stop_is_terminal() {
        assert!(StreamChunk::stop().is_terminal());
    }

    #[test]
    fn finish_reason_display() {
        assert_eq!(FinishReason::Stop.to_string(), "stop");
        assert_eq!(FinishReason::Length.to_string(), "length");
        assert_eq!(FinishReason::ToolCalls.to_string(), "tool_calls");
    }
}
