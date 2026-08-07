use std::io::BufRead;

use serde_json::Value;

/// JSONL 读取错误：全部映射为明确 transport error，不静默忽略。
#[derive(Debug)]
pub enum ReadError {
    Io(std::io::Error),
    InvalidUtf8,
    MalformedJson(String),
    FrameTooLong(usize),
    /// EOF 时仍有未完成帧
    HalfFrame,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Io(e) => write!(f, "io error: {e}"),
            ReadError::InvalidUtf8 => write!(f, "frame is not valid UTF-8"),
            ReadError::MalformedJson(e) => write!(f, "malformed JSON: {e}"),
            ReadError::FrameTooLong(n) => write!(f, "frame exceeds max length ({n} bytes)"),
            ReadError::HalfFrame => write!(f, "EOF in the middle of a frame"),
        }
    }
}

/// 增量 UTF-8 JSONL reader：仅以 LF 分帧，容忍 LF 前的 CR，跳过空行。
pub struct JsonlReader<R: BufRead> {
    inner: R,
    max_frame_bytes: usize,
}

impl<R: BufRead> JsonlReader<R> {
    pub fn new(inner: R, max_frame_bytes: usize) -> Self {
        Self {
            inner,
            max_frame_bytes,
        }
    }

    /// 返回下一帧；`Ok(None)` 表示干净 EOF。
    pub fn next_frame(&mut self) -> Result<Option<Value>, ReadError> {
        let mut buf: Vec<u8> = Vec::new();
        let n = self
            .inner
            .read_until(b'\n', &mut buf)
            .map_err(ReadError::Io)?;
        if n == 0 {
            return Ok(None);
        }
        if buf.len() > self.max_frame_bytes {
            return Err(ReadError::FrameTooLong(buf.len()));
        }
        if buf[buf.len() - 1] != b'\n' {
            return Err(ReadError::HalfFrame);
        }
        while matches!(buf.last(), Some(b'\n') | Some(b'\r')) {
            buf.pop();
        }
        let text = String::from_utf8(buf).map_err(|_| ReadError::InvalidUtf8)?;
        if text.trim().is_empty() {
            return self.next_frame();
        }
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| ReadError::MalformedJson(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    fn parse(bytes: &[u8]) -> Vec<Result<Option<Value>, ReadError>> {
        let mut r = JsonlReader::new(BufReader::new(bytes), 1024);
        let mut out = Vec::new();
        loop {
            match r.next_frame() {
                Ok(Some(v)) => out.push(Ok(Some(v))),
                other => {
                    out.push(other);
                    break;
                }
            }
        }
        out
    }

    #[test]
    fn parses_multiple_lf_frames() {
        let out = parse(b"{\"a\":1}\n{\"b\":2}\n");
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].as_ref().unwrap().as_ref().unwrap()["a"], 1);
        assert_eq!(out[1].as_ref().unwrap().as_ref().unwrap()["b"], 2);
        assert!(out[2].is_ok());
        assert!(out[2].as_ref().unwrap().is_none());
    }

    #[test]
    fn tolerates_crlf_and_empty_lines() {
        let out = parse(b"\r\n{\"a\":1}\r\n\n{\"b\":2}\r\n");
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].as_ref().unwrap().as_ref().unwrap()["a"], 1);
        assert_eq!(out[1].as_ref().unwrap().as_ref().unwrap()["b"], 2);
    }

    #[test]
    fn half_frame_eof_is_error() {
        let out = parse(b"{\"a\":1}");
        assert!(matches!(out[0], Err(ReadError::HalfFrame)));
    }

    #[test]
    fn invalid_utf8_is_error() {
        let out = parse(b"{\"a\":\"\xff\"}\n");
        assert!(matches!(out[0], Err(ReadError::InvalidUtf8)));
    }

    #[test]
    fn malformed_json_is_error() {
        let out = parse(b"{\"a\": }\n");
        assert!(matches!(out[0], Err(ReadError::MalformedJson(_))));
    }

    #[test]
    fn overlong_frame_is_error() {
        let data = format!("{}\n", "x".repeat(2048));
        let mut r = JsonlReader::new(BufReader::new(data.as_bytes()), 1024);
        assert!(matches!(r.next_frame(), Err(ReadError::FrameTooLong(_))));
    }

    #[test]
    fn clean_eof_is_none() {
        let out = parse(b"");
        assert!(matches!(out.as_slice(), [Ok(None)]));
    }
}
