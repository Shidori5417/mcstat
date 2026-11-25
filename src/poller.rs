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

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use hickory_resolver::TokioResolver;
use hickory_resolver::config::{CLOUDFLARE, ResolverConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::RData;
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, Id as TaskId, JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;

use crate::model::{
    AppEvent, DEFAULT_PORT, DEFAULT_TIMEOUT_SECS, PollResult, ServerConfig, ServerId, ServerState,
    ServerStatus,
};
use crate::protocol::query_status;

/// Time-to-live for cached DNS SRV answers (~5 minutes).
const SRV_CACHE_TTL: Duration = Duration::from_secs(300);

/// Maximum entries retained in the SRV cache before pruning expired records.
const SRV_CACHE_MAX_ENTRIES: usize = 256;

/// Linear backoff base per retry attempt (`300 ms * attempt`).
const RETRY_BACKOFF_BASE: Duration = Duration::from_millis(300);

/// Maximum time [`PollerHandle::shutdown`] waits for graceful termination
/// before aborting the poller task.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

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
        let _ = self.commands.send(PollerCommand::RefreshAll);
    }

    pub fn set_servers(&self, servers: Vec<(ServerId, ServerConfig)>) {
        let _ = self.commands.send(PollerCommand::SetServers(servers));
    }

    /// Ask the task to stop and wait for it (bounded, must not hang).
    pub async fn shutdown(self) {
        let _ = self.commands.send(PollerCommand::Shutdown);
        let mut task = self.task;
        if tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut task)
            .await
            .is_err()
        {
            task.abort();
            let _ = tokio::time::timeout(Duration::from_millis(50), task).await;
        }
    }
}

/// Spawn the poller on the current tokio runtime. It performs an initial
/// poll of all `servers` right away.
pub fn spawn_poller(
    settings: PollerSettings,
    servers: Vec<(ServerId, ServerConfig)>,
    events: mpsc::UnboundedSender<AppEvent>,
) -> PollerHandle {
    spawn_poller_with(settings, servers, events, query_server)
}

/// Spawn a poller with an injectable query function for testing.
fn spawn_poller_with<Q, Fut, E>(
    settings: PollerSettings,
    servers: Vec<(ServerId, ServerConfig)>,
    events: mpsc::UnboundedSender<AppEvent>,
    query_fn: Q,
) -> PollerHandle
where
    Q: Fn(ServerConfig, Duration) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<ServerStatus, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(run_poller(
        settings,
        servers,
        events,
        cmd_rx,
        Arc::new(query_fn),
    ));
    PollerHandle {
        commands: cmd_tx,
        task,
    }
}

/// Production query function: resolves SRV if needed and executes the SLP
/// exchange within the shared attempt timeout budget.
async fn query_server(config: ServerConfig, timeout: Duration) -> Result<ServerStatus, String> {
    let started = Instant::now();
    let target = resolve_target_with_timeout(&config, timeout).await;
    let remaining = timeout.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Err("connection timed out".to_string());
    }
    query_status(
        &target.connect_host,
        target.connect_port,
        &target.handshake_host,
        target.handshake_port,
        remaining,
    )
    .await
    .map_err(|err| err.to_string())
}

struct PollerRuntime<Q> {
    settings: PollerSettings,
    servers: Vec<(ServerId, ServerConfig)>,
    events: mpsc::UnboundedSender<AppEvent>,
    query_fn: Arc<Q>,
    in_flight: JoinSet<(ServerId, ServerState)>,
    active_tasks: HashMap<ServerId, (TaskId, AbortHandle)>,
    task_to_server: HashMap<TaskId, ServerId>,
}

impl<Q, Fut, E> PollerRuntime<Q>
where
    Q: Fn(ServerConfig, Duration) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<ServerStatus, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    /// Spawn a poll task for `id` unless one is already in flight.
    /// Returns `false` if the event receiver has been dropped.
    fn start_poll(&mut self, id: ServerId, config: ServerConfig) -> bool {
        if self.active_tasks.contains_key(&id) {
            return true;
        }
        if self.events.send(AppEvent::PollStarted(id)).is_err() {
            return false;
        }
        let query_fn = Arc::clone(&self.query_fn);
        let settings = self.settings;
        let abort_handle = self.in_flight.spawn(async move {
            let state = poll_one_server(config, settings, query_fn.as_ref()).await;
            (id, state)
        });
        let task_id = abort_handle.id();
        self.active_tasks.insert(id, (task_id, abort_handle));
        self.task_to_server.insert(task_id, id);
        true
    }

    /// Poll every configured server that is not currently in flight.
    fn poll_all(&mut self) -> bool {
        for idx in 0..self.servers.len() {
            let (id, config) = self.servers[idx].clone();
            if !self.start_poll(id, config) {
                return false;
            }
        }
        true
    }

    /// Replace the active server list, aborting removed in-flight polls and
    /// immediately polling newly added servers.
    fn set_servers(&mut self, new_servers: Vec<(ServerId, ServerConfig)>) -> bool {
        let old_ids: HashSet<ServerId> = self.servers.iter().map(|(id, _)| *id).collect();
        let new_ids: HashSet<ServerId> = new_servers.iter().map(|(id, _)| *id).collect();

        let removed: Vec<ServerId> = self
            .active_tasks
            .keys()
            .copied()
            .filter(|id| !new_ids.contains(id))
            .collect();
        for id in removed {
            if let Some((_, handle)) = self.active_tasks.remove(&id) {
                handle.abort();
            }
        }

        self.servers = new_servers;

        for idx in 0..self.servers.len() {
            let (id, config) = self.servers[idx].clone();
            if !old_ids.contains(&id) && !self.start_poll(id, config) {
                return false;
            }
        }
        true
    }

    /// Finalize one task entry if it is still the active task for `id`,
    /// forwarding `state` to the UI when the server is still configured.
    fn finish_task(&mut self, task_id: TaskId, id: ServerId, state: Option<ServerState>) -> bool {
        self.task_to_server.remove(&task_id);
        let is_current = self
            .active_tasks
            .get(&id)
            .is_some_and(|(active_tid, _)| *active_tid == task_id);
        if !is_current {
            return true;
        }
        self.active_tasks.remove(&id);
        let Some(state) = state else {
            return true;
        };
        if !self.servers.iter().any(|(sid, _)| *sid == id) {
            return true;
        }
        self.events
            .send(AppEvent::PollFinished(PollResult {
                id,
                state,
                finished_at: Instant::now(),
            }))
            .is_ok()
    }

    /// Reap one finished or aborted task from `in_flight`.
    fn handle_completed(
        &mut self,
        joined: Result<(TaskId, (ServerId, ServerState)), tokio::task::JoinError>,
    ) -> bool {
        match joined {
            Ok((task_id, (id, state))) => self.finish_task(task_id, id, Some(state)),
            Err(err) => {
                let task_id = err.id();
                let Some(&id) = self.task_to_server.get(&task_id) else {
                    return true;
                };
                let state = (!err.is_cancelled()).then(|| ServerState::Offline {
                    error: format!("poll task failed: {err}"),
                });
                self.finish_task(task_id, id, state)
            }
        }
    }

    fn abort_all(&mut self) {
        self.in_flight.abort_all();
        self.active_tasks.clear();
        self.task_to_server.clear();
    }
}

async fn run_poller<Q, Fut, E>(
    settings: PollerSettings,
    servers: Vec<(ServerId, ServerConfig)>,
    events: mpsc::UnboundedSender<AppEvent>,
    mut cmd_rx: mpsc::UnboundedReceiver<PollerCommand>,
    query_fn: Arc<Q>,
) where
    Q: Fn(ServerConfig, Duration) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<ServerStatus, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    let mut state = PollerRuntime {
        settings,
        servers,
        events,
        query_fn,
        in_flight: JoinSet::new(),
        active_tasks: HashMap::new(),
        task_to_server: HashMap::new(),
    };

    let mut interval = tokio::time::interval(settings.interval.max(Duration::from_nanos(1)));
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    interval.reset();

    if !state.poll_all() {
        state.abort_all();
        return;
    }

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if !state.poll_all() {
                    break;
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(PollerCommand::RefreshAll) => {
                        if !state.poll_all() {
                            break;
                        }
                        interval.reset();
                    }
                    Some(PollerCommand::SetServers(new_servers)) => {
                        if !state.set_servers(new_servers) {
                            break;
                        }
                    }
                    Some(PollerCommand::Shutdown) | None => {
                        break;
                    }
                }
            }
            Some(joined) = state.in_flight.join_next_with_id(), if !state.in_flight.is_empty() => {
                if !state.handle_completed(joined) {
                    break;
                }
            }
            _ = state.events.closed() => {
                break;
            }
        }
    }

    state.abort_all();
}

/// Poll one server with up to `settings.retries` retries, linear backoff
/// (`300 ms * attempt`), and `settings.timeout` bounding each attempt.
async fn poll_one_server<Q, Fut, E>(
    config: ServerConfig,
    settings: PollerSettings,
    query_fn: &Q,
) -> ServerState
where
    Q: Fn(ServerConfig, Duration) -> Fut,
    Fut: Future<Output = Result<ServerStatus, E>>,
    E: std::fmt::Display,
{
    let mut last_error = String::new();
    for attempt in 0..=settings.retries {
        if attempt > 0 {
            let backoff = RETRY_BACKOFF_BASE.saturating_mul(attempt);
            tokio::time::sleep(backoff).await;
        }

        let fut = query_fn(config.clone(), settings.timeout);
        match tokio::time::timeout(settings.timeout, fut).await {
            Ok(Ok(status)) => return ServerState::Online(status),
            Ok(Err(err)) => {
                last_error = err.to_string();
            }
            Err(_) => {
                last_error = "connection timed out".to_string();
            }
        }
    }

    ServerState::Offline { error: last_error }
}

/// Resolve the connection target for a server config (SRV when
/// `config.port` is `None`, else direct).
#[allow(dead_code)] // Part of the public poller contract and tested directly.
pub async fn resolve_target(config: &ServerConfig) -> ResolvedTarget {
    resolve_target_with_timeout(config, Duration::from_secs(DEFAULT_TIMEOUT_SECS)).await
}

/// Resolve the connection target with an explicit bound on any DNS SRV lookup.
async fn resolve_target_with_timeout(config: &ServerConfig, timeout: Duration) -> ResolvedTarget {
    if let Some(port) = config.port {
        return ResolvedTarget {
            connect_host: config.address.clone(),
            connect_port: port,
            handshake_host: config.address.clone(),
            handshake_port: port,
        };
    }

    if is_ip_literal(&config.address) {
        return ResolvedTarget {
            connect_host: config.address.clone(),
            connect_port: DEFAULT_PORT,
            handshake_host: config.address.clone(),
            handshake_port: DEFAULT_PORT,
        };
    }

    let (connect_host, connect_port) = lookup_srv_cached(&config.address, timeout)
        .await
        .unwrap_or_else(|| (config.address.clone(), DEFAULT_PORT));

    ResolvedTarget {
        connect_host,
        connect_port,
        handshake_host: config.address.clone(),
        handshake_port: connect_port,
    }
}

/// Return whether `address` is a bare or bracketed IPv4/IPv6 literal.
fn is_ip_literal(address: &str) -> bool {
    let trimmed = address.trim();
    if trimmed.parse::<IpAddr>().is_ok() {
        return true;
    }
    trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .is_some_and(|inner| inner.parse::<IpAddr>().is_ok())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SrvCandidate {
    priority: u16,
    weight: u16,
    port: u16,
    target: String,
}

/// Choose the highest-priority (lowest `priority`), then highest-`weight` SRV
/// target, stripping any trailing `.`.
fn select_srv_target<'a>(
    records: impl IntoIterator<Item = &'a SrvCandidate>,
) -> Option<(String, u16)> {
    records
        .into_iter()
        .filter_map(|rec| {
            let host = rec.target.trim_end_matches('.');
            if host.is_empty() || rec.port == 0 {
                None
            } else {
                Some((rec.priority, rec.weight, host.to_string(), rec.port))
            }
        })
        .min_by(|(p1, w1, _, _), (p2, w2, _, _)| p1.cmp(p2).then_with(|| w2.cmp(w1)))
        .map(|(_, _, host, port)| (host, port))
}

#[derive(Debug, Clone)]
struct CachedSrv {
    /// `Some((host, port))` when SRV resolved; `None` when definitively absent.
    target: Option<(String, u16)>,
    expires_at: Instant,
}

#[derive(Default)]
struct SrvCache {
    entries: HashMap<String, CachedSrv>,
}

impl SrvCache {
    fn get(&self, key: &str, now: Instant) -> Option<Option<(String, u16)>> {
        self.entries
            .get(key)
            .filter(|entry| entry.expires_at > now)
            .map(|entry| entry.target.clone())
    }

    fn insert(&mut self, key: String, target: Option<(String, u16)>, now: Instant) {
        if self.entries.len() >= SRV_CACHE_MAX_ENTRIES && !self.entries.contains_key(&key) {
            self.entries.retain(|_, entry| entry.expires_at > now);
            if self.entries.len() >= SRV_CACHE_MAX_ENTRIES {
                self.entries.clear();
            }
        }
        self.entries.insert(
            key,
            CachedSrv {
                target,
                expires_at: now + SRV_CACHE_TTL,
            },
        );
    }
}

fn srv_cache() -> &'static Mutex<SrvCache> {
    static CACHE: OnceLock<Mutex<SrvCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(SrvCache::default()))
}

fn srv_cache_key(address: &str) -> String {
    address.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Shared DNS resolver initialized once, falling back to Cloudflare public DNS
/// if the host system configuration cannot be read.
fn get_resolver() -> Option<&'static TokioResolver> {
    static RESOLVER: OnceLock<Option<TokioResolver>> = OnceLock::new();
    RESOLVER
        .get_or_init(|| {
            if let Ok(builder) = TokioResolver::builder_tokio()
                && let Ok(resolver) = builder.build()
            {
                return Some(resolver);
            }
            TokioResolver::builder_with_config(
                ResolverConfig::udp_and_tcp(&CLOUDFLARE),
                TokioRuntimeProvider::default(),
            )
            .build()
            .ok()
        })
        .as_ref()
}

async fn lookup_srv_cached(address: &str, timeout: Duration) -> Option<(String, u16)> {
    let key = srv_cache_key(address);
    if key.is_empty() {
        return None;
    }

    let now = Instant::now();
    {
        let cache = srv_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = cache.get(&key, now) {
            return cached;
        }
    }

    let resolver = get_resolver()?;
    let srv_domain = format!("_minecraft._tcp.{key}.");
    let lookup_res = tokio::time::timeout(timeout, resolver.srv_lookup(srv_domain)).await;

    match lookup_res {
        Ok(Ok(lookup)) => {
            let candidates: Vec<SrvCandidate> = lookup
                .answers()
                .iter()
                .filter_map(|record| match &record.data {
                    RData::SRV(srv) => Some(SrvCandidate {
                        priority: srv.priority,
                        weight: srv.weight,
                        port: srv.port,
                        target: srv.target.to_string(),
                    }),
                    _ => None,
                })
                .collect();
            let chosen = select_srv_target(&candidates);
            let mut cache = srv_cache().lock().unwrap_or_else(|e| e.into_inner());
            cache.insert(key, chosen.clone(), Instant::now());
            chosen
        }
        Ok(Err(err)) if err.is_no_records_found() => {
            let mut cache = srv_cache().lock().unwrap_or_else(|e| e.into_inner());
            cache.insert(key, None, Instant::now());
            None
        }
        Ok(Err(_)) | Err(_) => None,
    }
}
