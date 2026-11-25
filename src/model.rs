//! Shared data contracts used by every module.
//!
//! OWNERSHIP: Orchestrator (Opus). Feature branches must NOT change the
//! existing items in this file. If a branch truly needs a new shared type,
//! it should add it in its own module instead.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Default Minecraft Java Edition port.
pub const DEFAULT_PORT: u16 = 25565;

/// Default auto-refresh interval.
pub const DEFAULT_REFRESH_SECS: u64 = 5;

/// Default per-attempt network timeout.
pub const DEFAULT_TIMEOUT_SECS: u64 = 3;

/// Stable runtime identifier of a server entry. Assigned by `state::App`,
/// never persisted. Survives reordering/deletion of other entries.
pub type ServerId = u64;

/// A server as the user configured it (persisted in the config file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Display name shown in the table.
    pub name: String,
    /// Hostname or IP address (no port).
    pub address: String,
    /// Explicit port. `None` means: try DNS SRV `_minecraft._tcp.<address>`
    /// first, then fall back to [`DEFAULT_PORT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// Parsed result of a successful Server List Ping.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerStatus {
    /// `version.name`, e.g. "Paper 1.21.4".
    pub version_name: String,
    /// `version.protocol`.
    pub protocol_version: i32,
    pub players_online: i64,
    pub players_max: i64,
    /// Names from `players.sample` (may be empty).
    pub player_sample: Vec<String>,
    /// Raw `description` field: either a JSON string (possibly containing
    /// legacy `§` codes) or a chat component object. Rendering is the UI's job.
    pub motd: serde_json::Value,
    /// Round-trip latency measured with the Ping/Pong packet (falls back to
    /// the status request round-trip if the server does not answer pings).
    pub latency: Duration,
    /// Whether the server sent a favicon.
    pub has_favicon: bool,
    /// `host:port` that was actually connected to (after SRV resolution).
    pub resolved_endpoint: String,
}

/// Current observed state of one server.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ServerState {
    /// Never queried yet.
    #[default]
    Unknown,
    Online(ServerStatus),
    /// Query failed; human readable reason.
    Offline {
        error: String,
    },
}

/// Result of one poll of one server, sent from the poller to the app.
#[derive(Debug, Clone)]
pub struct PollResult {
    pub id: ServerId,
    pub state: ServerState,
    pub finished_at: Instant,
}

/// Everything the main loop reacts to. Produced by the input thread, the
/// tick timer and the poller; consumed by the main loop.
#[derive(Debug)]
pub enum AppEvent {
    /// A terminal input event (keys, resize, ...).
    Input(crossterm::event::Event),
    /// Periodic UI tick (animations / expiring messages).
    Tick,
    /// Poller started querying the given server.
    PollStarted(ServerId),
    /// Poller finished querying a server.
    PollFinished(PollResult),
}

/// What the main loop must do after the UI handled a key press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing beyond a redraw.
    None,
    /// Exit the application cleanly.
    Quit,
    /// Re-query every server right now.
    RefreshAll,
    /// The server list changed (add/delete). Main loop must push the new
    /// list to the poller; `App` already persisted it to disk.
    ServersChanged,
}
