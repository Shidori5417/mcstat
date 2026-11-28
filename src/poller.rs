//! Concurrent background poller.
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use serde_json::Value;
    use tokio::sync::Notify;

    use super::*;

    fn cfg(name: &str, address: &str, port: Option<u16>) -> ServerConfig {
        ServerConfig {
            name: name.to_string(),
            address: address.to_string(),
            port,
        }
    }

    fn fake_status(endpoint: &str) -> ServerStatus {
        ServerStatus {
            version_name: "1.21.4".to_string(),
            protocol_version: 769,
            players_online: 5,
            players_max: 20,
            player_sample: vec![],
            motd: Value::String("Hello".to_string()),
            latency: Duration::from_millis(12),
            has_favicon: false,
            resolved_endpoint: endpoint.to_string(),
        }
    }

    struct AbortGuard(Arc<AtomicBool>);
    impl Drop for AbortGuard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn resolve_target_explicit_port_skips_srv() {
        let host_target = resolve_target(&cfg("Custom", "play.example.com", Some(25577))).await;
        assert_eq!(
            host_target,
            ResolvedTarget {
                connect_host: "play.example.com".to_string(),
                connect_port: 25577,
                handshake_host: "play.example.com".to_string(),
                handshake_port: 25577,
            }
        );

        let ip_target = resolve_target(&cfg("Local", "127.0.0.1", Some(19132))).await;
        assert_eq!(ip_target.connect_host, "127.0.0.1");
        assert_eq!(ip_target.connect_port, 19132);
        assert_eq!(ip_target.handshake_host, "127.0.0.1");
        assert_eq!(ip_target.handshake_port, 19132);
    }

    #[tokio::test]
    async fn resolve_target_ip_literal_uses_default_port_without_srv() {
        for ip in ["127.0.0.1", "::1", "[::1]"] {
            let target = resolve_target(&cfg("IP", ip, None)).await;
            assert_eq!(
                target,
                ResolvedTarget {
                    connect_host: ip.to_string(),
                    connect_port: DEFAULT_PORT,
                    handshake_host: ip.to_string(),
                    handshake_port: DEFAULT_PORT,
                }
            );
        }
    }

    #[test]
    fn select_srv_target_picks_lowest_priority_then_highest_weight_and_strips_dot() {
        let candidates = [
            SrvCandidate {
                priority: 20,
                weight: 100,
                port: 25565,
                target: "backup.example.com.".to_string(),
            },
            SrvCandidate {
                priority: 10,
                weight: 5,
                port: 25566,
                target: "low-weight.example.com.".to_string(),
            },
            SrvCandidate {
                priority: 10,
                weight: 50,
                port: 25567,
                target: "winner.example.com.".to_string(),
            },
            SrvCandidate {
                priority: 0,
                weight: 100,
                port: 25565,
                target: ".".to_string(),
            },
            SrvCandidate {
                priority: 0,
                weight: 100,
                port: 0,
                target: "zero-port.example.com.".to_string(),
            },
        ];

        assert_eq!(
            select_srv_target(&candidates),
            Some(("winner.example.com".to_string(), 25567))
        );
        assert_eq!(select_srv_target(&[]), None);
        assert_eq!(
            select_srv_target(&[SrvCandidate {
                priority: 0,
                weight: 0,
                port: 25565,
                target: ".".to_string(),
            }]),
            None
        );
    }

    #[test]
    fn srv_cache_respects_ttl_and_key_normalization() {
        assert_eq!(srv_cache_key("  Play.Example.COM. "), "play.example.com");

        let mut cache = SrvCache::default();
        let t0 = Instant::now();
        cache.insert(
            "play.example.com".to_string(),
            Some(("node1.example.com".to_string(), 25566)),
            t0,
        );
        assert_eq!(
            cache.get("play.example.com", t0 + Duration::from_secs(10)),
            Some(Some(("node1.example.com".to_string(), 25566)))
        );
        assert_eq!(cache.get("play.example.com", t0 + SRV_CACHE_TTL), None);
    }

    #[tokio::test]
    #[ignore = "requires internet access"]
    async fn resolve_target_real_srv_2b2t() {
        let target = resolve_target(&cfg("2b2t", "2b2t.org", None)).await;
        assert_eq!(target.handshake_host, "2b2t.org");
        assert_eq!(target.connect_host, "connect.2b2t.org");
        assert_eq!(target.connect_port, 25565);
        assert_eq!(target.handshake_port, 25565);
    }

    #[tokio::test]
    async fn initial_poll_emits_started_and_finished_for_all() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let settings = PollerSettings {
            interval: Duration::from_secs(30),
            timeout: Duration::from_secs(2),
            retries: 0,
        };
        let servers = vec![
            (1, cfg("A", "a.example.com", Some(25565))),
            (2, cfg("B", "b.example.com", Some(25566))),
        ];

        let handle = spawn_poller_with(settings, servers, tx, |sc, _| async move {
            Ok::<_, std::convert::Infallible>(fake_status(&sc.address))
        });

        let mut started = HashSet::new();
        let mut finished = HashMap::new();
        while finished.len() < 2 {
            let ev = tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .expect("event within timeout")
                .expect("channel open");
            match ev {
                AppEvent::PollStarted(id) => {
                    started.insert(id);
                }
                AppEvent::PollFinished(res) => {
                    assert!(
                        started.contains(&res.id),
                        "PollFinished arrived before PollStarted for {}",
                        res.id
                    );
                    finished.insert(res.id, res.state);
                }
                _ => {}
            }
        }

        assert_eq!(started, HashSet::from([1, 2]));
        assert!(matches!(finished.get(&1), Some(ServerState::Online(_))));
        assert!(matches!(finished.get(&2), Some(ServerState::Online(_))));
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn slow_server_does_not_block_fast_server() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let settings = PollerSettings {
            interval: Duration::from_secs(30),
            timeout: Duration::from_secs(5),
            retries: 0,
        };
        let servers = vec![
            (1, cfg("Slow", "slow.example.com", None)),
            (2, cfg("Fast", "fast.example.com", None)),
        ];

        let handle = spawn_poller_with(settings, servers, tx, |sc, _| async move {
            if sc.address.starts_with("slow") {
                tokio::time::sleep(Duration::from_millis(120)).await;
            }
            Ok::<_, std::convert::Infallible>(fake_status(&sc.address))
        });

        let mut finished_order = Vec::new();
        let mut started_before_first_finish = HashSet::new();
        while finished_order.len() < 2 {
            let ev = rx.recv().await.expect("event");
            match ev {
                AppEvent::PollStarted(id) => {
                    if finished_order.is_empty() {
                        started_before_first_finish.insert(id);
                    }
                }
                AppEvent::PollFinished(res) => finished_order.push(res.id),
                _ => {}
            }
        }

        assert_eq!(finished_order, vec![2, 1]);
        assert_eq!(started_before_first_finish, HashSet::from([1, 2]));
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn no_overlapping_polls_on_refresh_all() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let settings = PollerSettings {
            interval: Duration::from_secs(60),
            timeout: Duration::from_secs(10),
            retries: 0,
        };

        let gate = Arc::new(Notify::new());
        let gate_ref = Arc::clone(&gate);
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_ref = Arc::clone(&calls);
        let in_flight = Arc::new(AtomicUsize::new(0));
        let in_flight_ref = Arc::clone(&in_flight);
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        let max_in_flight_ref = Arc::clone(&max_in_flight);

        let handle = spawn_poller_with(
            settings,
            vec![(1, cfg("One", "one.example.com", None))],
            tx,
            move |sc, _| {
                let gate = Arc::clone(&gate_ref);
                let calls = Arc::clone(&calls_ref);
                let in_flight = Arc::clone(&in_flight_ref);
                let max_in_flight = Arc::clone(&max_in_flight_ref);
                async move {
                    let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_in_flight.fetch_max(cur, Ordering::SeqCst);
                    if n == 1 {
                        gate.notified().await;
                    }
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    Ok::<_, std::convert::Infallible>(fake_status(&sc.address))
                }
            },
        );

        assert!(matches!(rx.recv().await, Some(AppEvent::PollStarted(1))));
        tokio::task::yield_now().await;

        // RefreshAll while poll 1 is still in flight must skip server 1.
        handle.refresh_all();
        handle.refresh_all();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(max_in_flight.load(Ordering::SeqCst), 1);

        gate.notify_one();
        assert!(matches!(
            rx.recv().await,
            Some(AppEvent::PollFinished(PollResult { id: 1, .. }))
        ));

        // Once finished, RefreshAll polls again.
        handle.refresh_all();
        assert!(matches!(rx.recv().await, Some(AppEvent::PollStarted(1))));
        assert!(matches!(
            rx.recv().await,
            Some(AppEvent::PollFinished(PollResult { id: 1, .. }))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(max_in_flight.load(Ordering::SeqCst), 1);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn periodic_interval_skips_in_flight_server() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let settings = PollerSettings {
            interval: Duration::from_millis(80),
            timeout: Duration::from_secs(2),
            retries: 0,
        };
        let slow_calls = Arc::new(AtomicUsize::new(0));
        let fast_calls = Arc::new(AtomicUsize::new(0));
        let slow_ref = Arc::clone(&slow_calls);
        let fast_ref = Arc::clone(&fast_calls);

        let handle = spawn_poller_with(
            settings,
            vec![
                (1, cfg("Slow", "slow.example.com", None)),
                (2, cfg("Fast", "fast.example.com", None)),
            ],
            tx,
            move |sc, _| {
                let slow_calls = Arc::clone(&slow_ref);
                let fast_calls = Arc::clone(&fast_ref);
                async move {
                    if sc.address.starts_with("slow") {
                        slow_calls.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    } else {
                        fast_calls.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    Ok::<_, std::convert::Infallible>(fake_status(&sc.address))
                }
            },
        );

        // Wait for the 80ms interval tick to re-poll server 2 while server 1 (300ms) is in flight.
        let mut fast_finishes = 0;
        while fast_finishes < 2 {
            if let Some(AppEvent::PollFinished(PollResult { id: 2, .. })) = rx.recv().await {
                fast_finishes += 1;
            }
        }

        assert_eq!(fast_calls.load(Ordering::SeqCst), 2);
        assert_eq!(slow_calls.load(Ordering::SeqCst), 1);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn set_servers_polls_new_ids_and_drops_removed() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let settings = PollerSettings {
            interval: Duration::from_secs(60),
            timeout: Duration::from_secs(10),
            retries: 0,
        };

        let aborted = Arc::new(AtomicBool::new(false));
        let aborted_ref = Arc::clone(&aborted);
        let fast_calls = Arc::new(AtomicUsize::new(0));
        let fast_ref = Arc::clone(&fast_calls);

        let handle = spawn_poller_with(
            settings,
            vec![
                (1, cfg("DropMe", "drop.example.com", None)),
                (2, cfg("KeepMe", "keep.example.com", None)),
            ],
            tx,
            move |sc, _| {
                let aborted = Arc::clone(&aborted_ref);
                let fast_calls = Arc::clone(&fast_ref);
                async move {
                    if sc.address.starts_with("drop") {
                        let _guard = AbortGuard(aborted);
                        std::future::pending::<()>().await;
                    } else if sc.address.starts_with("keep") {
                        fast_calls.fetch_add(1, Ordering::SeqCst);
                    }
                    Ok::<_, std::convert::Infallible>(fake_status(&sc.address))
                }
            },
        );

        let mut saw_keep_finish = false;
        let mut saw_drop_start = false;
        while !(saw_keep_finish && saw_drop_start) {
            match rx.recv().await.expect("event") {
                AppEvent::PollStarted(1) => saw_drop_start = true,
                AppEvent::PollFinished(PollResult { id: 2, .. }) => saw_keep_finish = true,
                _ => {}
            }
        }

        handle.set_servers(vec![
            (2, cfg("KeepMe", "keep.example.com", None)),
            (3, cfg("NewOne", "new.example.com", None)),
        ]);

        let mut saw_new_start = false;
        let mut saw_new_finish = false;
        while !(saw_new_start && saw_new_finish) {
            match rx.recv().await.expect("event") {
                AppEvent::PollStarted(3) => saw_new_start = true,
                AppEvent::PollFinished(PollResult { id: 3, .. }) => saw_new_finish = true,
                AppEvent::PollFinished(PollResult { id: 1, .. }) => {
                    panic!("removed server 1 must not emit PollFinished after abort");
                }
                _ => {}
            }
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(aborted.load(Ordering::SeqCst));
        assert_eq!(fast_calls.load(Ordering::SeqCst), 1);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn retries_with_backoff_then_offline() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let settings = PollerSettings {
            interval: Duration::from_secs(60),
            timeout: Duration::from_secs(1),
            retries: 2,
        };
        let attempt_times = Arc::new(Mutex::new(Vec::new()));
        let attempt_times_ref = Arc::clone(&attempt_times);

        let handle = spawn_poller_with(
            settings,
            vec![(1, cfg("Down", "down.example.com", None))],
            tx,
            move |_, _| {
                let attempt_times = Arc::clone(&attempt_times_ref);
                async move {
                    attempt_times.lock().expect("lock").push(Instant::now());
                    Err::<ServerStatus, _>("connection refused")
                }
            },
        );

        assert!(matches!(rx.recv().await, Some(AppEvent::PollStarted(1))));
        let finished = match rx.recv().await {
            Some(AppEvent::PollFinished(res)) => res,
            other => panic!("unexpected event: {other:?}"),
        };
        assert_eq!(finished.id, 1);
        assert_eq!(
            finished.state,
            ServerState::Offline {
                error: "connection refused".to_string(),
            }
        );

        {
            let times = attempt_times.lock().expect("lock");
            assert_eq!(times.len(), 3);
            assert!(times[1].duration_since(times[0]) >= Duration::from_millis(280));
            assert!(times[2].duration_since(times[1]) >= Duration::from_millis(580));
        }

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn retry_succeeds_after_transient_failure_and_timeout_bounds_attempts() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let settings = PollerSettings {
            interval: Duration::from_secs(60),
            timeout: Duration::from_millis(150),
            retries: 1,
        };

        let server1_attempts = Arc::new(AtomicUsize::new(0));
        let s1_ref = Arc::clone(&server1_attempts);

        let handle = spawn_poller_with(
            settings,
            vec![
                (1, cfg("Flaky", "flaky.example.com", None)),
                (2, cfg("Hung", "hung.example.com", None)),
            ],
            tx,
            move |sc, _| {
                let s1_attempts = Arc::clone(&s1_ref);
                async move {
                    if sc.address.starts_with("flaky") {
                        let attempt = s1_attempts.fetch_add(1, Ordering::SeqCst);
                        if attempt == 0 {
                            Err("temporary reset")
                        } else {
                            Ok(fake_status(&sc.address))
                        }
                    } else {
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        Ok(fake_status(&sc.address))
                    }
                }
            },
        );

        let mut results = HashMap::new();
        while results.len() < 2 {
            if let Some(AppEvent::PollFinished(res)) = rx.recv().await {
                results.insert(res.id, res.state);
            }
        }

        assert!(matches!(results.get(&1), Some(ServerState::Online(_))));
        assert_eq!(
            results.get(&2),
            Some(&ServerState::Offline {
                error: "connection timed out".to_string(),
            })
        );
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_returns_promptly_with_hung_poll() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let settings = PollerSettings {
            interval: Duration::from_secs(60),
            timeout: Duration::from_secs(30),
            retries: 0,
        };
        let aborted = Arc::new(AtomicBool::new(false));
        let aborted_ref = Arc::clone(&aborted);

        let handle = spawn_poller_with(
            settings,
            vec![(1, cfg("Hung", "hung.example.com", None))],
            tx,
            move |_, _| {
                let aborted = Arc::clone(&aborted_ref);
                async move {
                    let _guard = AbortGuard(aborted);
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Ok::<_, std::convert::Infallible>(fake_status("hung"))
                }
            },
        );

        assert!(matches!(rx.recv().await, Some(AppEvent::PollStarted(1))));
        let started = Instant::now();
        handle.shutdown().await;
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(aborted.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn poller_stops_when_event_receiver_dropped() {
        let (tx, rx) = mpsc::unbounded_channel();
        let settings = PollerSettings {
            interval: Duration::from_secs(5),
            timeout: Duration::from_secs(1),
            retries: 0,
        };

        let handle = spawn_poller_with(
            settings,
            vec![(1, cfg("S", "s.example.com", None))],
            tx,
            |sc, _| async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok::<_, std::convert::Infallible>(fake_status(&sc.address))
            },
        );

        drop(rx);
        tokio::time::timeout(Duration::from_secs(1), handle.task)
            .await
            .expect("poller task must exit when receiver is dropped")
            .expect("poller task should not panic");
    }
}
