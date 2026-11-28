//! Application state.

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
    /// Refresh interval shown in the header. Starts as the config value; a
    /// command-line override replaces it for this run only and is never
    /// written back to `config`, so saving the file keeps the file's value.
    pub refresh_interval_secs: u64,
    next_id: ServerId,
}

impl App {
    /// Build state from a loaded config; assigns fresh sequential ids starting
    /// at 1 with state [`ServerState::Unknown`].
    pub fn new(config: AppConfig, config_path: PathBuf) -> Self {
        let refresh_interval_secs = config.refresh_interval_secs;
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
            refresh_interval_secs,
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;
    use crate::model::ServerStatus;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir().join(format!(
                "mcstat-test-{label}-{}-{nanos}-{seq}",
                std::process::id()
            ));
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sample_server(name: &str, address: &str, port: Option<u16>) -> ServerConfig {
        ServerConfig {
            name: name.to_string(),
            address: address.to_string(),
            port,
        }
    }

    fn sample_status() -> ServerStatus {
        ServerStatus {
            version_name: "Paper 1.21.4".to_string(),
            protocol_version: 769,
            players_online: 12,
            players_max: 100,
            player_sample: vec!["Steve".to_string()],
            motd: json!("Welcome!"),
            latency: Duration::from_millis(25),
            has_favicon: false,
            resolved_endpoint: "mc.example.com:25565".to_string(),
        }
    }

    #[test]
    fn form_field_cycles_forward_and_backward() {
        assert_eq!(FormField::Name.next(), FormField::Address);
        assert_eq!(FormField::Address.next(), FormField::Port);
        assert_eq!(FormField::Port.next(), FormField::Name);

        assert_eq!(FormField::Name.prev(), FormField::Port);
        assert_eq!(FormField::Port.prev(), FormField::Address);
        assert_eq!(FormField::Address.prev(), FormField::Name);
    }

    #[test]
    fn add_server_form_char_filtering_and_validation() {
        let mut form = AddServerForm {
            error: Some("previous error".to_string()),
            ..Default::default()
        };

        // Control chars ignored and do not modify field
        form.input_char('\n');
        form.input_char('\t');
        form.input_char('\x1b');
        assert!(form.name.is_empty());

        // Normal chars accepted and clear error
        for c in "My Server".chars() {
            form.input_char(c);
        }
        assert_eq!(form.name, "My Server");
        assert_eq!(form.error, None);

        // Address field
        form.focus = form.focus.next();
        for c in "play.example.org".chars() {
            form.input_char(c);
        }
        assert_eq!(form.address, "play.example.org");

        // Port field: ASCII digits only, max 5
        form.focus = form.focus.next();
        form.error = Some("bad port".to_string());
        form.input_char('a');
        form.input_char('-');
        form.input_char(' ');
        form.input_char('٢'); // non-ASCII Arabic-Indic digit
        assert!(form.port.is_empty());

        for c in "25565999".chars() {
            form.input_char(c);
        }
        assert_eq!(form.port, "25565");
        assert_eq!(form.error, None);

        // Backspace removes last char and clears error
        form.error = Some("some error".to_string());
        form.backspace();
        assert_eq!(form.port, "2556");
        assert_eq!(form.error, None);
        form.input_char('5');

        let cfg = form.to_server_config().expect("valid form");
        assert_eq!(
            cfg,
            sample_server("My Server", "play.example.org", Some(25565))
        );
    }

    #[test]
    fn navigation_on_empty_and_populated_list() {
        let tmp = TempDir::new("state-nav");
        let empty_cfg = AppConfig {
            servers: vec![],
            ..AppConfig::default()
        };
        let mut app = App::new(empty_cfg, tmp.path().join("config.toml"));
        assert_eq!(app.selected, 0);
        assert!(app.selected_entry().is_none());
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.message.is_none());
        assert!(app.last_refresh.is_none());

        app.select_next();
        app.select_prev();
        app.select_first();
        app.select_last();
        assert_eq!(app.selected, 0);
        assert!(app.selected_entry().is_none());

        let mut app = App::new(AppConfig::default(), tmp.path().join("config.toml"));
        assert_eq!(app.servers.len(), 3);
        assert_eq!(app.servers[0].id, 1);
        assert_eq!(app.servers[1].id, 2);
        assert_eq!(app.servers[2].id, 3);
        assert_eq!(app.selected_entry().map(|e| e.id), Some(1));

        // Wrap backward from 0 -> last
        app.select_prev();
        assert_eq!(app.selected, 2);
        assert_eq!(app.selected_entry().map(|e| e.id), Some(3));

        // Wrap forward from last -> 0
        app.select_next();
        assert_eq!(app.selected, 0);

        app.select_last();
        assert_eq!(app.selected, 2);

        app.select_first();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn add_and_remove_persist_and_clamp_selection() {
        let tmp = TempDir::new("state-persist");
        let cfg_path = tmp.path().join("config.toml");
        let initial = AppConfig {
            servers: vec![
                sample_server("Alpha", "alpha.example.com", None),
                sample_server("Beta", "beta.example.com", Some(25566)),
            ],
            ..AppConfig::default()
        };
        initial.save(&cfg_path).expect("save initial");

        let mut app = App::new(initial, cfg_path.clone());
        assert_eq!(app.servers.len(), 2);

        // Add third server -> selected becomes 2, persisted on disk
        let new_id = app
            .add_server(sample_server("Gamma", "gamma.example.com", Some(25567)))
            .expect("add Gamma");
        assert_eq!(new_id, 3);
        assert_eq!(app.selected, 2);
        assert_eq!(app.servers.len(), 3);
        assert_eq!(app.config.servers.len(), 3);
        assert_eq!(app.servers[2].config.name, "Gamma");
        assert_eq!(app.config.servers[2].name, "Gamma");

        let on_disk = AppConfig::load_or_create(&cfg_path).expect("reload disk");
        assert_eq!(on_disk.servers, app.config.servers);

        // Remove last selected server -> selected clamps to 1
        let removed = app.remove_server(new_id).expect("remove Gamma");
        assert_eq!(
            removed,
            Some(sample_server("Gamma", "gamma.example.com", Some(25567)))
        );
        assert_eq!(app.selected, 1);
        assert_eq!(app.servers.len(), 2);
        assert_eq!(app.config.servers.len(), 2);

        let on_disk = AppConfig::load_or_create(&cfg_path).expect("reload after remove");
        assert_eq!(on_disk.servers, app.config.servers);

        // Removing unknown id returns Ok(None)
        assert_eq!(app.remove_server(999).expect("unknown id"), None);

        // Remove remaining servers down to empty -> selected clamps to 0
        let _ = app.remove_server(2).expect("remove Beta");
        assert_eq!(app.selected, 0);
        let _ = app.remove_server(1).expect("remove Alpha");
        assert_eq!(app.selected, 0);
        assert!(app.servers.is_empty());
        assert!(app.config.servers.is_empty());

        let on_disk = AppConfig::load_or_create(&cfg_path).expect("reload empty");
        assert!(on_disk.servers.is_empty());
    }

    #[test]
    fn add_and_remove_roll_back_on_unwritable_config_path() {
        let tmp = TempDir::new("state-rollback");
        let blocking_file = tmp.path().join("blocker");
        fs::write(&blocking_file, "regular file").expect("create blocking file");
        let unwritable_path = blocking_file.join("config.toml");

        let initial = AppConfig {
            servers: vec![
                sample_server("Alpha", "alpha.example.com", None),
                sample_server("Beta", "beta.example.com", None),
            ],
            ..AppConfig::default()
        };
        let mut app = App::new(initial.clone(), unwritable_path);
        app.selected = 1;

        // Failed add_server must roll back everything
        let add_err = app
            .add_server(sample_server("Gamma", "gamma.example.com", None))
            .expect_err("add should fail on unwritable path");
        assert!(matches!(add_err, ConfigError::Io { .. }));
        assert_eq!(app.servers.len(), 2);
        assert_eq!(app.config.servers, initial.servers);
        assert_eq!(app.selected, 1);

        // Failed remove_server must roll back everything in original order
        let remove_err = app
            .remove_server(1)
            .expect_err("remove should fail on unwritable path");
        assert!(matches!(remove_err, ConfigError::Io { .. }));
        assert_eq!(app.servers.len(), 2);
        assert_eq!(app.servers[0].id, 1);
        assert_eq!(app.servers[1].id, 2);
        assert_eq!(app.config.servers, initial.servers);
        assert_eq!(app.selected, 1);

        // Verify next_id was also not wasted by failed add
        let writable_path = tmp.path().join("real-config.toml");
        app.config_path = writable_path;
        let id = app
            .add_server(sample_server("Gamma", "gamma.example.com", None))
            .expect("add succeeds on writable path");
        assert_eq!(id, 3);
    }

    #[test]
    fn poll_lifecycle_and_online_count() {
        let tmp = TempDir::new("state-poll");
        let mut app = App::new(AppConfig::default(), tmp.path().join("config.toml"));
        let targets = app.poll_targets();
        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0].0, 1);
        assert_eq!(targets[0].1.name, "Hypixel");

        assert_eq!(app.online_count(), (0, 3));

        app.mark_refreshing(1);
        app.mark_refreshing(999); // unknown id ignored
        assert!(app.servers[0].refreshing);
        assert!(!app.servers[1].refreshing);

        let now = Instant::now();
        app.apply_poll_result(PollResult {
            id: 1,
            state: ServerState::Online(sample_status()),
            finished_at: now,
        });
        app.apply_poll_result(PollResult {
            id: 2,
            state: ServerState::Offline {
                error: "connection refused".to_string(),
            },
            finished_at: now,
        });
        // Unknown id must be silently ignored
        app.apply_poll_result(PollResult {
            id: 999,
            state: ServerState::Online(sample_status()),
            finished_at: now,
        });

        assert!(!app.servers[0].refreshing);
        assert_eq!(app.servers[0].last_updated, Some(now));
        assert!(matches!(app.servers[0].state, ServerState::Online(_)));
        assert!(matches!(app.servers[1].state, ServerState::Offline { .. }));
        assert!(matches!(app.servers[2].state, ServerState::Unknown));
        assert_eq!(app.online_count(), (1, 3));
    }

    #[test]
    fn status_message_set_and_expire() {
        let tmp = TempDir::new("state-msg");
        let mut app = App::new(AppConfig::default(), tmp.path().join("config.toml"));

        app.set_message("Saved", MessageKind::Info);
        assert!(app.message.is_some());
        app.expire_message();
        assert!(app.message.is_some());

        if let Some(old_time) = Instant::now().checked_sub(Duration::from_secs(5)) {
            app.message = Some(StatusMessage {
                text: "Old error".to_string(),
                kind: MessageKind::Error,
                created: old_time,
            });
            app.expire_message();
            assert!(app.message.is_none());
        }
    }
}
