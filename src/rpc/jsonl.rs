use std::io::{BufRead, BufReader, Read};
use std::sync::mpsc::Receiver;

pub(crate) const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
const INPUT_QUEUE_CAPACITY: usize = 64;

pub(crate) enum InputFrame {
    Line(String),
    Eof,
    Error(InputError),
}

#[derive(Debug)]
pub(crate) struct InputError {
    pub code: &'static str,
    pub message: String,
}

pub(crate) fn spawn_stdin_reader() -> Receiver<InputFrame> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(INPUT_QUEUE_CAPACITY);
    std::thread::Builder::new()
        .name("rpc-stdin".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut reader = BufReader::new(stdin.lock());
            loop {
                match read_frame(&mut reader) {
                    Ok(Some(line)) => {
                        if sender.send(InputFrame::Line(line)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        let _ = sender.send(InputFrame::Eof);
                        break;
                    }
                    Err(error) => {
                        let _ = sender.send(InputFrame::Error(error));
                        break;
                    }
                }
            }
        })
        .expect("failed to create RPC stdin reader");
    receiver
}

pub(crate) fn read_frame(reader: &mut impl BufRead) -> Result<Option<String>, InputError> {
    let mut bytes = Vec::new();
    let mut limited = reader
        .by_ref()
        .take((MAX_FRAME_BYTES.saturating_add(2)) as u64);
    let read = limited
        .read_until(b'\n', &mut bytes)
        .map_err(|error| InputError {
            code: "io_error",
            message: error.to_string(),
        })?;
    if read == 0 {
        return Ok(None);
    }
    if !bytes.ends_with(b"\n") {
        return Err(InputError {
            code: if bytes.len() > MAX_FRAME_BYTES {
                "frame_too_large"
            } else {
                "unterminated_frame"
            },
            message: if bytes.len() > MAX_FRAME_BYTES {
                format!("RPC frame exceeds {MAX_FRAME_BYTES} bytes")
            } else {
                "RPC frame must end with LF".into()
            },
        });
    }
    bytes.pop();
    if bytes.ends_with(b"\r") {
        bytes.pop();
    }
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(InputError {
            code: "frame_too_large",
            message: format!("RPC frame exceeds {MAX_FRAME_BYTES} bytes"),
        });
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| InputError {
            code: "invalid_utf8",
            message: error.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn framing_splits_only_on_lf_and_accepts_crlf() {
        let mut input = Cursor::new("{\"text\":\"a\u{2028}b\"}\r\nnext\n".as_bytes());
        assert_eq!(
            read_frame(&mut input).unwrap().unwrap(),
            "{\"text\":\"a\u{2028}b\"}"
        );
        assert_eq!(read_frame(&mut input).unwrap().unwrap(), "next");
        assert!(read_frame(&mut input).unwrap().is_none());
    }

    #[test]
    fn unterminated_and_oversized_frames_are_rejected() {
        let mut partial = Cursor::new(b"partial".as_slice());
        assert_eq!(
            read_frame(&mut partial).unwrap_err().code,
            "unterminated_frame"
        );

        let mut oversized = Cursor::new(vec![b'x'; MAX_FRAME_BYTES + 2]);
        assert_eq!(
            read_frame(&mut oversized).unwrap_err().code,
            "frame_too_large"
        );
    }
}
