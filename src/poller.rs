//! Concurrent background poller.
//!
//! OWNERSHIP: Hydrogen (`feat/async-poller`).
//! Public signatures below are the contract; bodies are to be implemented.
//!
//! Responsibilities:
//! - Every `interval`, poll all servers concurrently (`tokio::spawn` per
//!   server or a JoinSet), never blocking the UI.
//! - Do not start a new poll for a server whose previous poll is still in
//!   flight.
//! - Resolve DNS SRV `_minecraft._tcp.<address>` when `port` is `None`
//!   (hickory-resolver), fall back to `address:25565`. Cache SRV lookups.
//! - Per-attempt timeout + retry with small backoff before reporting Offline.
//! - Send `AppEvent::PollStarted(id)` before and `AppEvent::PollFinished`
//!   after each server poll on the `events` channel.

use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::model::{AppEvent, ServerConfig, ServerId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollerSettings {
    /// Auto-refresh period.
    pub interval: Duration,
    /// Timeout for ONE attempt (connect + full SLP exchange).
    pub timeout: Duration,
    /// Extra attempts after the first failure (0 = no retry).
    pub retries: u32,
}

/// Commands accepted by the poller task.
#[derive(Debug, Clone)]
pub enum PollerCommand {
    /// Poll every server immediately (and restart the interval timer).
    RefreshAll,
    /// Replace the server list (after add/delete). Newly added servers are
    /// polled immediately; removed ones are dropped (in-flight results for
    /// removed ids may still arrive; `App` ignores them).
    SetServers(Vec<(ServerId, ServerConfig)>),
    /// Stop the poller task.
    Shutdown,
}

/// Where to connect and what to put in the handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub connect_host: String,
    pub connect_port: u16,
    /// Original address (handshake host).
    pub handshake_host: String,
    pub handshake_port: u16,
}

/// Handle owned by the main loop.
pub struct PollerHandle {
    commands: mpsc::UnboundedSender<PollerCommand>,
    task: JoinHandle<()>,
}

impl PollerHandle {
    pub fn refresh_all(&self) {
        let _ = &self.commands;
        todo!("feat/async-poller")
    }

    pub fn set_servers(&self, servers: Vec<(ServerId, ServerConfig)>) {
        let _ = servers;
        todo!("feat/async-poller")
    }

    /// Ask the task to stop and wait for it (bounded, must not hang).
    pub async fn shutdown(self) {
        let _ = self.task;
        todo!("feat/async-poller")
    }
}

/// Spawn the poller on the current tokio runtime. It performs an initial
/// poll of all `servers` right away.
pub fn spawn_poller(
    settings: PollerSettings,
    servers: Vec<(ServerId, ServerConfig)>,
    events: mpsc::UnboundedSender<AppEvent>,
) -> PollerHandle {
    let _ = (settings, servers, events);
    todo!("feat/async-poller")
}

/// Resolve the connection target for a server config (SRV when
/// `config.port` is `None`, else direct).
pub async fn resolve_target(config: &ServerConfig) -> ResolvedTarget {
    let _ = config;
    todo!("feat/async-poller")
}
