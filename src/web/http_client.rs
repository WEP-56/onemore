use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;

use super::{clean_inline, WebSearchBackendKind, WebSearchError, WebSearchErrorKind};

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 8 * 1024;
const MAX_ERROR_CHARS: usize = 1_000;

pub(super) fn agent() -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(60))
        .timeout_write(Duration::from_secs(30));
    for key in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
    ] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            if let Ok(proxy) = ureq::Proxy::new(value) {
                builder = builder.proxy(proxy);
                break;
            }
        }
    }
    builder.build()
}

pub(super) fn execute_json(
    request: ureq::Request,
    body: Option<&Value>,
    cancel: &AtomicBool,
    backend: WebSearchBackendKind,
) -> Result<Value, WebSearchError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(aborted(backend));
    }
    let response = match body {
        Some(body) => request.send_string(&body.to_string()),
        None => request.call(),
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(aborted(backend));
    }
    match response {
        Ok(response) => {
            let text =
                read_bounded(response.into_reader(), MAX_RESPONSE_BYTES).map_err(|message| {
                    WebSearchError {
                        backend,
                        kind: WebSearchErrorKind::InvalidResponse,
                        message: format!(
                            "{} returned an invalid response: {message}",
                            backend.label()
                        ),
                        retryable: false,
                        status: Some(200),
                    }
                })?;
            serde_json::from_str(&text).map_err(|error| WebSearchError {
                backend,
                kind: WebSearchErrorKind::InvalidResponse,
                message: format!("{} returned invalid JSON: {error}", backend.label()),
                retryable: false,
                status: Some(200),
            })
        }
        Err(ureq::Error::Status(status, response)) => {
            let text = read_bounded(response.into_reader(), MAX_ERROR_BYTES)
                .unwrap_or_else(|_| String::new());
            let detail = extract_error_message(&text);
            Err(WebSearchError {
                backend,
                kind: WebSearchErrorKind::Http,
                message: format!(
                    "{} web search failed with HTTP {status}: {detail}",
                    backend.label()
                ),
                retryable: matches!(status, 408 | 429) || status >= 500,
                status: Some(status),
            })
        }
        Err(ureq::Error::Transport(error)) => {
            let raw = error.to_string();
            let lowercase = raw.to_ascii_lowercase();
            let timed_out = lowercase.contains("timed out") || lowercase.contains("timeout");
            Err(WebSearchError {
                backend,
                kind: if timed_out {
                    WebSearchErrorKind::Timeout
                } else {
                    WebSearchErrorKind::Network
                },
                message: format!(
                    "{} web search network error: {}",
                    backend.label(),
                    clean_inline(&raw, MAX_ERROR_CHARS)
                ),
                retryable: true,
                status: None,
            })
        }
    }
}

fn read_bounded(mut reader: impl Read, maximum: usize) -> Result<String, String> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take((maximum + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read response: {error}"))?;
    if bytes.len() > maximum {
        return Err(format!("response exceeded the {maximum} byte limit"));
    }
    String::from_utf8(bytes).map_err(|_| "response was not valid UTF-8".into())
}

fn extract_error_message(text: &str) -> String {
    let value: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let message = value
        .pointer("/detail/error")
        .or_else(|| value.get("error"))
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or(text);
    let message = clean_inline(message, MAX_ERROR_CHARS);
    if message.is_empty() {
        "request failed without an error message".into()
    } else {
        message
    }
}

fn aborted(backend: WebSearchBackendKind) -> WebSearchError {
    WebSearchError {
        backend,
        kind: WebSearchErrorKind::Aborted,
        message: format!("{} web search was cancelled", backend.label()),
        retryable: false,
        status: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_nested_and_plain_error_messages_without_control_text() {
        assert_eq!(
            extract_error_message(r#"{"detail":{"error":" bad\nrequest "}}"#),
            "bad request"
        );
        assert_eq!(
            extract_error_message(""),
            "request failed without an error message"
        );
    }
}
