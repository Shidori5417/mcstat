//! Application state.
//!
//! OWNERSHIP: Neon (`feat/config-state`).
//! The struct FIELDS and public method signatures below are the contract the
//! UI (`ui.rs`) and `main.rs` are written against. Implement the bodies.
//! Adding new pub methods is fine; do not rename/remove existing ones.

use std::path::PathBuf;
use std::time::Instant;

use crate::config::{AppConfig, ConfigError};
use crate::model::{PollResult, ServerConfig, ServerId, ServerState};

/// One row of the server table.
#[derive(Debug, Clone)]
pub struct ServerEntry {
    pub id: ServerId,
    pub config: ServerConfig,
    pub state: ServerState,
    /// When the last poll result arrived.
    pub last_updated: Option<Instant>,
    /// A poll is currently in flight for this server.
    pub refreshing: bool,
}

/// Which field of the add-server form has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormField {
    #[default]
    Name,
    Address,
    Port,
}

impl FormField {
    pub fn next(self) -> Self {
        todo!("feat/config-state")
    }
    pub fn prev(self) -> Self {
        todo!("feat/config-state")
    }
}

/// State of the "add server" popup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddServerForm {
    pub name: String,
    pub address: String,
    pub port: String,
    pub focus: FormField,
    /// Validation error shown inside the popup.
    pub error: Option<String>,
}

impl AddServerForm {
    /// Mutable access to the focused field's text.
    pub fn focused_mut(&mut self) -> &mut String {
        todo!("feat/config-state")
    }
    /// Append a char to the focused field (Port only accepts ASCII digits,
    /// max 5 chars). Clears `error`.
    pub fn input_char(&mut self, c: char) {
        let _ = c;
        todo!("feat/config-state")
    }
    /// Delete last char of the focused field. Clears `error`.
    pub fn backspace(&mut self) {
        todo!("feat/config-state")
    }
    /// Validate via `config::validate_server_input`.
    pub fn to_server_config(&self) -> Result<ServerConfig, ConfigError> {
        todo!("feat/config-state")
    }
}

/// Current interaction mode.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Normal,
    AddServer(AddServerForm),
    /// Waiting for y/n to delete the server with this id.
    ConfirmDelete(ServerId),
    Help,
}

/// Severity of a transient status-bar message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Info,
    Error,
}

#[derive(Debug, Clone)]
pub struct StatusMessage {
    pub text: String,
    pub kind: MessageKind,
    pub created: Instant,
}

pub struct App {
    pub servers: Vec<ServerEntry>,
    /// Index into `servers` (0 when empty).
    pub selected: usize,
    pub mode: Mode,
    pub config: AppConfig,
    pub config_path: PathBuf,
    pub message: Option<StatusMessage>,
    /// Time of the last full refresh trigger (for "next refresh in" display).
    pub last_refresh: Option<Instant>,
    next_id: ServerId,
}

impl App {
    /// Build state from a loaded config; assigns fresh ids, state Unknown.
    pub fn new(config: AppConfig, config_path: PathBuf) -> Self {
        let _ = (config, config_path);
        todo!("feat/config-state")
    }

    pub fn selected_entry(&self) -> Option<&ServerEntry> {
        todo!("feat/config-state")
    }
    pub fn select_next(&mut self) {
        todo!("feat/config-state")
    }
    pub fn select_prev(&mut self) {
        todo!("feat/config-state")
    }
    pub fn select_first(&mut self) {
        todo!("feat/config-state")
    }
    pub fn select_last(&mut self) {
        todo!("feat/config-state")
    }

    /// Append a server, select it, persist config to `config_path`.
    pub fn add_server(&mut self, server: ServerConfig) -> Result<ServerId, ConfigError> {
        let _ = server;
        todo!("feat/config-state")
    }

    /// Remove the server with `id`, fix `selected`, persist config.
    /// Returns the removed config, `Ok(None)` if no such id.
    pub fn remove_server(&mut self, id: ServerId) -> Result<Option<ServerConfig>, ConfigError> {
        let _ = id;
        todo!("feat/config-state")
    }

    /// `(id, config)` pairs for the poller.
    pub fn poll_targets(&self) -> Vec<(ServerId, ServerConfig)> {
        todo!("feat/config-state")
    }

    /// Mark a server as being polled (ignores unknown ids).
    pub fn mark_refreshing(&mut self, id: ServerId) {
        let _ = id;
        todo!("feat/config-state")
    }

    /// Apply a poll result (ignores unknown ids, e.g. already deleted).
    pub fn apply_poll_result(&mut self, result: PollResult) {
        let _ = result;
        todo!("feat/config-state")
    }

    pub fn set_message(&mut self, text: impl Into<String>, kind: MessageKind) {
        let _ = (text.into(), kind);
        todo!("feat/config-state")
    }

    /// Drop the status message if older than a few seconds.
    pub fn expire_message(&mut self) {
        todo!("feat/config-state")
    }

    /// (online, total) counts for the header.
    pub fn online_count(&self) -> (usize, usize) {
        todo!("feat/config-state")
    }
}
