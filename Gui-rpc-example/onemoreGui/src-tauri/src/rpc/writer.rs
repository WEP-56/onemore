use std::io::Write;
use std::process::ChildStdin;
use std::sync::mpsc::Receiver;
use std::thread::{self, JoinHandle};

use tauri::{AppHandle, Emitter};

use super::events::{RpcEvent, EVENT_NAME};

/// 发送端 framing：统一以 LF 结尾。
fn with_lf(frame: &str) -> Vec<u8> {
    let mut bytes = frame.as_bytes().to_vec();
    bytes.push(b'\n');
    bytes
}

/// 单 writer task：独占 stdin，一行一个完整帧。
pub fn spawn_writer(app: AppHandle, mut stdin: ChildStdin, rx: Receiver<String>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut write_error = None;
        while let Ok(frame) = rx.recv() {
            if let Err(e) = stdin
                .write_all(&with_lf(&frame))
                .and_then(|_| stdin.flush())
            {
                write_error = Some(e);
                break;
            }
        }
        drop(stdin);
        if let Some(e) = write_error {
            let _ = app.emit(
                EVENT_NAME,
                RpcEvent::TransportError {
                    code: "write_error".into(),
                    message: e.to_string(),
                },
            );
        }
    })
}
