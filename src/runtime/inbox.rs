use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};

use crate::event::AgentCommand;
use crate::sdk::{CommandReceipt, SessionError};

pub(crate) struct PendingCommand {
    pub command_id: Option<String>,
    pub command: AgentCommand,
    session_managed: bool,
    admission: Option<SyncSender<Result<CommandReceipt, SessionError>>>,
}

impl PendingCommand {
    pub(crate) fn direct(command: AgentCommand) -> Self {
        PendingCommand {
            command_id: None,
            command,
            session_managed: false,
            admission: None,
        }
    }

    pub(crate) fn session(
        command_id: String,
        command: AgentCommand,
        admission: SyncSender<Result<CommandReceipt, SessionError>>,
    ) -> Self {
        PendingCommand {
            command_id: Some(command_id),
            command,
            session_managed: true,
            admission: Some(admission),
        }
    }

    pub(crate) fn detached(command: AgentCommand) -> Self {
        PendingCommand {
            command_id: None,
            command,
            session_managed: true,
            admission: None,
        }
    }

    pub(crate) fn is_session(&self) -> bool {
        self.session_managed
    }

    pub(crate) fn accept(&mut self) {
        let Some(sender) = self.admission.take() else {
            return;
        };
        let command_id = self
            .command_id
            .clone()
            .expect("session command must have an id");
        let _ = sender.send(Ok(CommandReceipt { command_id }));
    }

    pub(crate) fn reject(&mut self, error: SessionError) {
        if let Some(sender) = self.admission.take() {
            let _ = sender.send(Err(error));
        }
    }
}

pub(crate) trait CommandInbox {
    fn try_recv_command(&self) -> Result<PendingCommand, TryRecvError>;
}

impl CommandInbox for Receiver<AgentCommand> {
    fn try_recv_command(&self) -> Result<PendingCommand, TryRecvError> {
        self.try_recv().map(PendingCommand::direct)
    }
}

impl CommandInbox for Receiver<PendingCommand> {
    fn try_recv_command(&self) -> Result<PendingCommand, TryRecvError> {
        self.try_recv()
    }
}
