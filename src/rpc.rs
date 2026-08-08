//! Strict JSONL subprocess protocol backed by the public session controller.

mod jsonl;
mod types;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;

use crate::event::AgentCommand;
use crate::runtime::Agent;
use crate::sdk::{spawn_session, AgentSession, SessionController, SessionError};

use jsonl::{spawn_stdin_reader, InputFrame};
use types::{
    ClientMessage, ErrorResponse, EventEnvelope, HelloError, HelloResponse, ProtocolErrorEnvelope,
    RequestCommand, ResponseResult, SuccessResponse,
};

const MAX_IN_FLIGHT_REQUESTS: usize = 64;
const MAX_REQUEST_IDS: usize = 65_536;
const MAX_REQUEST_ID_BYTES: usize = 256;

pub fn run(agent: Agent) -> anyhow::Result<()> {
    let input = spawn_stdin_reader();
    let stdout = std::io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    serve(agent, input, &mut output)
}

fn serve(
    agent: Agent,
    input: std::sync::mpsc::Receiver<InputFrame>,
    output: &mut impl Write,
) -> anyhow::Result<()> {
    let AgentSession { controller, events } = spawn_session(agent);
    let cleanup = controller.clone();
    let result = serve_session(controller, events, input, output);
    if result.is_err() {
        cleanup.cancel_now();
        let _ = cleanup.send_detached(AgentCommand::Shutdown);
    }
    result
}

fn serve_session(
    controller: SessionController,
    mut events: crate::sdk::SessionEvents,
    input: std::sync::mpsc::Receiver<InputFrame>,
    output: &mut impl Write,
) -> anyhow::Result<()> {
    let hello = match input
        .recv()
        .context("RPC stdin reader stopped before hello")?
    {
        InputFrame::Line(line) => match serde_json::from_str::<ClientMessage>(&line) {
            Ok(ClientMessage::Hello { version }) => version,
            Ok(ClientMessage::Request { .. }) => {
                write_json(
                    output,
                    &HelloError::new("invalid_handshake", "first frame must be hello"),
                )?;
                return Ok(());
            }
            Err(error) => {
                write_json(
                    output,
                    &HelloError::new("invalid_handshake", error.to_string()),
                )?;
                return Ok(());
            }
        },
        InputFrame::Eof => return Ok(()),
        InputFrame::Error(error) => {
            write_json(output, &HelloError::new(error.code, error.message))?;
            return Ok(());
        }
    };
    if hello != crate::sdk::PROTOCOL_VERSION {
        write_json(
            output,
            &HelloError::new(
                "version_mismatch",
                format!("unsupported protocol version {hello}"),
            ),
        )?;
        return Ok(());
    }
    write_json(
        output,
        &HelloResponse::new(controller.server_info(), controller.snapshot()?),
    )?;

    let (response_tx, response_rx) =
        std::sync::mpsc::sync_channel::<RequestOutput>(MAX_IN_FLIGHT_REQUESTS);
    let in_flight = Arc::new(AtomicUsize::new(0));
    let mut request_ids = HashSet::new();
    let mut input_closed = false;

    loop {
        while let Ok(response) = response_rx.try_recv() {
            in_flight.fetch_sub(1, Ordering::Relaxed);
            let shutdown = response.shutdown;
            write_response(output, response)?;
            if shutdown {
                return Ok(());
            }
        }
        while let Ok(event) = events.try_recv() {
            write_json(output, &EventEnvelope::new(event))?;
        }

        if input_closed {
            controller.cancel_now();
            let _ = controller.send_detached(AgentCommand::Shutdown);
            while let Ok(event) = events.recv() {
                write_json(output, &EventEnvelope::new(event))?;
            }
            return Ok(());
        }

        if in_flight.load(Ordering::Relaxed) >= MAX_IN_FLIGHT_REQUESTS {
            match events.recv_timeout(Duration::from_millis(10)) {
                Ok(event) => write_json(output, &EventEnvelope::new(event))?,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            }
            continue;
        }

        match input.recv_timeout(Duration::from_millis(10)) {
            Ok(InputFrame::Line(line)) => {
                let message = match serde_json::from_str::<ClientMessage>(&line) {
                    Ok(message) => message,
                    Err(error) => {
                        write_json(
                            output,
                            &ProtocolErrorEnvelope::new(
                                request_decode_error_code(&error),
                                error.to_string(),
                            ),
                        )?;
                        continue;
                    }
                };
                let ClientMessage::Request { id, request } = message else {
                    write_json(
                        output,
                        &ProtocolErrorEnvelope::new(
                            "invalid_request",
                            "hello may only be sent as the first frame",
                        ),
                    )?;
                    continue;
                };
                if id.trim().is_empty() {
                    write_json(
                        output,
                        &ProtocolErrorEnvelope::new(
                            "invalid_request",
                            "request id must not be empty",
                        ),
                    )?;
                    continue;
                }
                if id.len() > MAX_REQUEST_ID_BYTES {
                    write_json(
                        output,
                        &ProtocolErrorEnvelope::new(
                            "invalid_request",
                            format!("request id exceeds {MAX_REQUEST_ID_BYTES} bytes"),
                        ),
                    )?;
                    continue;
                }
                if request_ids.contains(&id) {
                    write_json(
                        output,
                        &ErrorResponse::new(
                            id,
                            "duplicate_request_id",
                            "request id was already used",
                        ),
                    )?;
                    continue;
                }
                if request_ids.len() >= MAX_REQUEST_IDS {
                    write_json(
                        output,
                        &ProtocolErrorEnvelope::new(
                            "request_limit_exceeded",
                            format!("connection exceeds {MAX_REQUEST_IDS} request ids"),
                        ),
                    )?;
                    input_closed = true;
                    continue;
                }
                request_ids.insert(id.clone());
                spawn_request(
                    controller.clone(),
                    id,
                    request,
                    response_tx.clone(),
                    Arc::clone(&in_flight),
                )?;
            }
            Ok(InputFrame::Eof) => input_closed = true,
            Ok(InputFrame::Error(error)) => {
                write_json(
                    output,
                    &ProtocolErrorEnvelope::new(error.code, error.message),
                )?;
                input_closed = true;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => input_closed = true,
        }
    }
}

fn request_decode_error_code(error: &serde_json::Error) -> &'static str {
    match error.classify() {
        serde_json::error::Category::Data => "invalid_request",
        serde_json::error::Category::Io
        | serde_json::error::Category::Syntax
        | serde_json::error::Category::Eof => "invalid_json",
    }
}

struct RequestOutput {
    id: String,
    result: Result<ResponseResult, SessionError>,
    shutdown: bool,
}

fn spawn_request(
    controller: SessionController,
    id: String,
    request: RequestCommand,
    responses: SyncSender<RequestOutput>,
    in_flight: Arc<AtomicUsize>,
) -> anyhow::Result<()> {
    in_flight.fetch_add(1, Ordering::Relaxed);
    let worker_in_flight = Arc::clone(&in_flight);
    let spawn = std::thread::Builder::new()
        .name("rpc-request".into())
        .spawn(move || {
            let requested_shutdown = matches!(request, RequestCommand::Shutdown);
            let result = handle_request(&controller, request);
            let shutdown = requested_shutdown && result.is_ok();
            if responses
                .send(RequestOutput {
                    id,
                    result,
                    shutdown,
                })
                .is_err()
            {
                worker_in_flight.fetch_sub(1, Ordering::Relaxed);
            }
        });
    if let Err(error) = spawn {
        in_flight.fetch_sub(1, Ordering::Relaxed);
        return Err(error).context("failed to create RPC request worker");
    }
    Ok(())
}

fn handle_request(
    controller: &SessionController,
    request: RequestCommand,
) -> Result<ResponseResult, SessionError> {
    Ok(match request {
        RequestCommand::Prompt { text } => ResponseResult::Prompt {
            command_id: controller.prompt(text)?.command_id,
        },
        RequestCommand::Steer { text } => ResponseResult::Steer {
            command_id: controller.steer(text)?.command_id,
        },
        RequestCommand::FollowUp { text } => ResponseResult::FollowUp {
            command_id: controller.follow_up(text)?.command_id,
        },
        RequestCommand::Abort => ResponseResult::Abort {
            command_id: controller.abort()?.command_id,
        },
        RequestCommand::Compact => ResponseResult::Compact {
            command_id: controller.compact()?.command_id,
        },
        RequestCommand::SetModel {
            provider,
            model,
            effort,
        } => ResponseResult::SetModel {
            command_id: controller
                .set_model(crate::sdk::ModelSelection {
                    provider,
                    model,
                    effort,
                })?
                .command_id,
        },
        RequestCommand::ClearConversation => ResponseResult::ClearConversation {
            command_id: controller.clear_conversation()?.command_id,
        },
        RequestCommand::ListSessions { all } => ResponseResult::ListSessions {
            sessions: if all {
                controller.list_all_sessions()?
            } else {
                controller.list_sessions()?
            },
        },
        RequestCommand::LoadSession { session_id } => ResponseResult::LoadSession {
            command_id: controller.load_session(session_id)?.command_id,
        },
        RequestCommand::ListModels => ResponseResult::ListModels {
            models: controller.list_models(),
        },
        RequestCommand::GetSnapshot => ResponseResult::GetSnapshot {
            snapshot: Box::new(controller.snapshot()?),
        },
        RequestCommand::ApprovalResponse {
            request_id,
            decision,
        } => {
            controller.respond_to_approval(crate::sdk::ApprovalResponseView {
                request_id,
                decision,
            })?;
            ResponseResult::ApprovalResponse
        }
        RequestCommand::Shutdown => ResponseResult::Shutdown {
            command_id: controller.shutdown()?.command_id,
        },
    })
}

fn write_response(output: &mut impl Write, response: RequestOutput) -> anyhow::Result<()> {
    match response.result {
        Ok(result) => write_json(output, &SuccessResponse::new(response.id, result)),
        Err(error) => write_json(
            output,
            &ErrorResponse::new(response.id, error.code.as_str(), error.message),
        ),
    }
}

fn write_json(output: &mut impl Write, value: &impl serde::Serialize) -> anyhow::Result<()> {
    serde_json::to_writer(&mut *output, value).context("failed to encode RPC frame")?;
    output
        .write_all(b"\n")
        .context("failed to write RPC frame")?;
    output.flush().context("failed to flush RPC frame")?;
    Ok(())
}
