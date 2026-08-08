use serde::Serialize;

/// GUI 自己的封闭错误 DTO：稳定 code + message，不伪装成协议错误。
#[derive(Debug, Clone, Serialize)]
pub struct GuiError {
    pub code: String,
    pub message: String,
}

impl GuiError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for GuiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for GuiError {}

impl From<std::io::Error> for GuiError {
    fn from(e: std::io::Error) -> Self {
        Self::new("io_error", e.to_string())
    }
}

impl From<serde_json::Error> for GuiError {
    fn from(e: serde_json::Error) -> Self {
        Self::new("invalid_json", e.to_string())
    }
}
