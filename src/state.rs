//! Application state.
//!
//! OWNERSHIP: Neon (`feat/config-state`).
//! The struct FIELDS and public method signatures below are the contract the
//! UI (`ui.rs`) and `main.rs` are written against. Implement the bodies.
//! Adding new pub methods is fine; do not rename/remove existing ones.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::config::{AppConfig, ConfigError, validate_server_input};
use crate::model::{PollResult, ServerConfig, ServerId, ServerState};

/// Duration after which transient status-bar messages expire.
const MESSAGE_TTL: Duration = Duration::from_secs(4);

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
        match self {
            Self::Name => Self::Address,
            Self::Address => Self::Port,
            Self::Port => Self::Name,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Name => Self::Port,
            Self::Address => Self::Name,
            Self::Port => Self::Address,
        }
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
        match self.focus {
            FormField::Name => &mut self.name,
            FormField::Address => &mut self.address,
            FormField::Port => &mut self.port,
        }
    }

    /// Append a char to the focused field (Port only accepts ASCII digits,
    /// max 5 chars; control chars are ignored). Clears `error` on edit.
    pub fn input_char(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        if self.focus == FormField::Port && (!c.is_ascii_digit() || self.port.len() >= 5) {
            return;
        }
        self.focused_mut().push(c);
        self.error = None;
    }

    /// Delete last char of the focused field. Clears `error`.
    pub fn backspace(&mut self) {
        self.focused_mut().pop();
        self.error = None;
    }

    /// Validate via `config::validate_server_input`.
    pub fn to_server_config(&self) -> Result<ServerConfig, ConfigError> {
        validate_server_input(&self.name, &self.address, &self.port)
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
    /// Build state from a loaded config; assigns fresh sequential ids starting
    /// at 1 with state [`ServerState::Unknown`].
    pub fn new(config: AppConfig, config_path: PathBuf) -> Self {
        let mut next_id: ServerId = 1;
        let servers = config
            .servers
            .iter()
            .map(|server_cfg| {
                let id = next_id;
                next_id += 1;
                ServerEntry {
                    id,
                    config: server_cfg.clone(),
                    state: ServerState::Unknown,
                    last_updated: None,
                    refreshing: false,
                }
            })
            .collect();

        Self {
            servers,
            selected: 0,
            mode: Mode::Normal,
            config,
            config_path,
            message: None,
            last_refresh: None,
            next_id,
        }
    }

    pub fn selected_entry(&self) -> Option<&ServerEntry> {
        self.servers.get(self.selected)
    }

    pub fn select_next(&mut self) {
        if self.servers.is_empty() {
            self.selected = 0;
        } else {
            self.selected = (self.selected + 1) % self.servers.len();
        }
    }

    pub fn select_prev(&mut self) {
        if self.servers.is_empty() {
            self.selected = 0;
        } else if self.selected == 0 || self.selected >= self.servers.len() {
            self.selected = self.servers.len() - 1;
        } else {
            self.selected -= 1;
        }
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    pub fn select_last(&mut self) {
        self.selected = self.servers.len().saturating_sub(1);
    }

    /// Append a server, select it, and persist config to `config_path`.
    ///
    /// If saving to disk fails, the in-memory change (`servers`,
    /// `config.servers`, `selected`, and `next_id`) is rolled back so memory
    /// and disk stay consistent, and the error is returned.
    pub fn add_server(&mut self, server: ServerConfig) -> Result<ServerId, ConfigError> {
        let id = self.next_id;
        let prev_selected = self.selected;

        self.config.servers.push(server.clone());
        if let Err(err) = self.config.save(&self.config_path) {
            self.config.servers.pop();
            self.selected = prev_selected;
            return Err(err);
        }

        self.next_id += 1;
        self.servers.push(ServerEntry {
            id,
            config: server,
            state: ServerState::Unknown,
            last_updated: None,
            refreshing: false,
        });
        self.selected = self.servers.len() - 1;
        Ok(id)
    }

    /// Remove the server with `id`, clamp `selected`, and persist config.
    /// Returns the removed config, or `Ok(None)` if no such id exists.
    ///
    /// If saving to disk fails, the in-memory change (`servers`,
    /// `config.servers`, and `selected`) is rolled back so memory and disk
    /// stay consistent, and the error is returned.
    pub fn remove_server(&mut self, id: ServerId) -> Result<Option<ServerConfig>, ConfigError> {
        let Some(idx) = self.servers.iter().position(|entry| entry.id == id) else {
            return Ok(None);
        };

        let prev_config_servers = self.config.servers.clone();
        if idx < self.config.servers.len() {
            self.config.servers.remove(idx);
        }

        if let Err(err) = self.config.save(&self.config_path) {
            self.config.servers = prev_config_servers;
            return Err(err);
        }

        let removed = self.servers.remove(idx);
        if self.servers.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.servers.len() {
            self.selected = self.servers.len() - 1;
        }

        Ok(Some(removed.config))
    }

    /// `(id, config)` pairs for the poller.
    pub fn poll_targets(&self) -> Vec<(ServerId, ServerConfig)> {
        self.servers
            .iter()
            .map(|entry| (entry.id, entry.config.clone()))
            .collect()
    }

    /// Mark a server as being polled (ignores unknown ids).
    pub fn mark_refreshing(&mut self, id: ServerId) {
        if let Some(entry) = self.servers.iter_mut().find(|entry| entry.id == id) {
            entry.refreshing = true;
        }
    }

    /// Apply a poll result (ignores unknown ids, e.g. already deleted).
    pub fn apply_poll_result(&mut self, result: PollResult) {
        if let Some(entry) = self.servers.iter_mut().find(|entry| entry.id == result.id) {
            entry.state = result.state;
            entry.last_updated = Some(result.finished_at);
            entry.refreshing = false;
        }
    }

    pub fn set_message(&mut self, text: impl Into<String>, kind: MessageKind) {
        self.message = Some(StatusMessage {
            text: text.into(),
            kind,
            created: Instant::now(),
        });
    }

    /// Drop the status message if older than 4 seconds.
    pub fn expire_message(&mut self) {
        if self
            .message
            .as_ref()
            .is_some_and(|msg| msg.created.elapsed() >= MESSAGE_TTL)
        {
            self.message = None;
        }
    }

    /// `(online, total)` counts for the header.
    pub fn online_count(&self) -> (usize, usize) {
        let online = self
            .servers
            .iter()
            .filter(|entry| matches!(entry.state, ServerState::Online(_)))
            .count();
        (online, self.servers.len())
    }
}
