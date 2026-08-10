//! End-to-end JSONL test through the real `onemore --rpc` subprocess.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

#[test]
fn rpc_subprocess_prompt_stream_is_clean_and_settles() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let provider = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        read_http_request(&mut reader).expect("RPC subprocess must call the provider");
        let sse = concat!(
            "event: response.output_item.added\n",
            r#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_rpc","role":"assistant","content":[]}}"#,
            "\n\n",
            "event: response.output_text.delta\n",
            r#"data: {"type":"response.output_text.delta","output_index":0,"delta":"subprocess answer"}"#,
            "\n\n",
            "event: response.output_item.done\n",
            r#"data: {"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_rpc","role":"assistant","content":[{"type":"output_text","text":"subprocess answer"}]}}"#,
            "\n\n",
            "event: response.completed\n",
            r#"data: {"type":"response.completed","response":{"id":"resp_rpc","status":"completed","usage":{"input_tokens":4,"output_tokens":2}}}"#,
            "\n\n",
        );
        write!(
            writer,
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            sse.len(),
            sse
        )
        .unwrap();
        writer.flush().unwrap();
    });

    let root = std::env::temp_dir().join(format!("onemore-rpc-wire-{port}"));
    std::fs::create_dir_all(&root).unwrap();
    let config = root.join("config.toml");
    std::fs::write(
        &config,
        format!(
            r#"
[agent]
provider = "mock"

[providers.mock]
api = "responses"
base_url = "http://127.0.0.1:{port}"
model = "test-model"
api_key = "test-key"
"#
        ),
    )
    .unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_onemore"));
    command
        .arg("--rpc")
        .arg("--config")
        .arg(&config)
        .current_dir(&root)
        .env("ONEMORE_HOME", root.join("state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
    ] {
        command.env_remove(name);
    }
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (line_tx, line_rx) = std::sync::mpsc::channel();
    let stdout_reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            line_tx.send(line).unwrap();
        }
    });

    send(&mut stdin, json!({"type": "hello", "version": 3}));
    let hello = recv_frame(&line_rx);
    assert_eq!(hello["type"], "hello");
    send(
        &mut stdin,
        json!({"type": "request", "id": "prompt", "request": {"command": "prompt", "text": "hello"}}),
    );

    let mut frames = vec![hello];
    let mut command_id = None;
    let mut finished_index = None;
    loop {
        let frame = recv_frame(&line_rx);
        if frame["type"] == "response" && frame["id"] == "prompt" {
            assert_eq!(frame["ok"], true);
            command_id = frame
                .pointer("/result/command_id")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        if frame.pointer("/event/type") == Some(&json!("command_finished")) {
            finished_index = Some(frames.len());
        }
        let settled = frame.pointer("/event/type") == Some(&json!("settled"));
        frames.push(frame);
        if settled {
            break;
        }
    }
    let command_id = command_id.expect("prompt response must include command_id");
    let settled_index = frames.len() - 1;
    assert!(finished_index.is_some_and(|index| index < settled_index));
    assert!(frames.iter().any(|frame| {
        frame.pointer("/event/command_id") == Some(&json!(command_id))
            && frame.pointer("/event/status") == Some(&json!("succeeded"))
    }));
    assert!(frames
        .iter()
        .any(|frame| frame.to_string().contains("subprocess answer")));

    send(
        &mut stdin,
        json!({"type": "request", "id": "shutdown", "request": {"command": "shutdown"}}),
    );
    loop {
        let frame = recv_frame(&line_rx);
        if frame["type"] == "response" && frame["id"] == "shutdown" {
            assert_eq!(frame["ok"], true);
            break;
        }
    }
    drop(stdin);

    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("RPC subprocess did not exit after shutdown");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    stdout_reader.join().unwrap();
    let mut diagnostics = String::new();
    BufReader::new(stderr)
        .read_to_string(&mut diagnostics)
        .unwrap();
    assert!(status.success(), "RPC stderr: {diagnostics}");
    provider.join().unwrap();
    let _ = std::fs::remove_dir_all(root);
}

fn send(stdin: &mut impl Write, frame: Value) {
    writeln!(stdin, "{frame}").unwrap();
    stdin.flush().unwrap();
}

fn recv_frame(lines: &Receiver<Result<String, std::io::Error>>) -> Value {
    let line = lines
        .recv_timeout(Duration::from_secs(5))
        .expect("timed out waiting for RPC subprocess output")
        .unwrap();
    serde_json::from_str(&line)
        .unwrap_or_else(|error| panic!("RPC stdout contains non-protocol data: {error}: {line:?}"))
}

fn read_http_request(reader: &mut BufReader<impl Read>) -> Option<String> {
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).ok()? == 0 {
        return None;
    }
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().ok()?;
        }
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}
