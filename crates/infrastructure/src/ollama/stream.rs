use bytes::BytesMut;
use futures_util::stream;
use domain::ports::llm_backend::{BackendError, BackendStream};
use domain::entities::stream_chunk::StreamChunk;

use super::types::OllamaChatChunk;

pub(crate) fn parse_ndjson_line(line: &[u8]) -> Result<StreamChunk, BackendError> {
    let chunk: OllamaChatChunk = serde_json::from_slice(line)
        .map_err(|e| BackendError::StreamParse(e.to_string()))?;
    if chunk.done {
        Ok(StreamChunk::stop())
    } else {
        Ok(StreamChunk::delta(chunk.message.content))
    }
}

pub fn into_stream(response: reqwest::Response) -> BackendStream {
    struct State {
        response: reqwest::Response,
        buf: BytesMut,
        done: bool,
    }

    let initial = State {
        response,
        buf: BytesMut::with_capacity(8 * 1024),
        done: false,
    };

    let s = stream::unfold(initial, |mut state| async move {
        loop {
            if state.done {
                return None;
            }

            if let Some(pos) = memchr::memchr(b'\n', &state.buf) {
                let line_bytes = state.buf.split_to(pos + 1);
                let trimmed = line_bytes.trim_ascii();
                if trimmed.is_empty() {
                    continue;
                }
                let chunk = parse_ndjson_line(trimmed);
                let is_terminal = chunk.as_ref().map(|c| c.is_terminal()).unwrap_or(false);
                if is_terminal {
                    state.done = true;
                }
                return Some((chunk, state));
            }

            match state.response.chunk().await {
                Ok(Some(bytes)) => {
                    state.buf.extend_from_slice(&bytes);
                }
                Ok(None) => {
                    let remaining = state.buf.split();
                    let trimmed = remaining.trim_ascii().to_owned();
                    state.done = true;
                    if !trimmed.is_empty() {
                        let chunk = parse_ndjson_line(&trimmed);
                        return Some((chunk, state));
                    }
                    return None;
                }
                Err(e) => {
                    state.done = true;
                    return Some((Err(BackendError::Transport(e.to_string())), state));
                }
            }
        }
    });

    Box::pin(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_delta_line() {
        let line = br#"{"model":"qwen2.5-coder:14b","message":{"role":"assistant","content":"hi"},"done":false}"#;
        let chunk = parse_ndjson_line(line).unwrap();
        assert!(!chunk.is_terminal());
        assert_eq!(chunk.delta, "hi");
    }

    #[test]
    fn parse_done_line() {
        let line = br#"{"model":"qwen2.5-coder:14b","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop"}"#;
        let chunk = parse_ndjson_line(line).unwrap();
        assert!(chunk.is_terminal());
    }

    #[test]
    fn parse_invalid_json() {
        let line = b"not json at all";
        let err = parse_ndjson_line(line).unwrap_err();
        assert!(matches!(err, BackendError::StreamParse(_)));
    }
}
