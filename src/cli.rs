//! Command-line interface.
//!
//! - [`Cli`] is the clap definition. Every option that also exists in the
//!   config file only overrides it for the current run; nothing is written
//!   back to disk.
//! - `--once` queries every configured server a single time through the
//!   regular poller (same timeout, retry and SRV rules as the TUI), prints an
//!   aligned plain-text table and reports whether all servers are online.

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Parser, value_parser};
use ratatui::text::Span;
use tokio::sync::mpsc;

use crate::POLL_RETRIES;
use crate::config::{AppConfig, ConfigError};
use crate::model::{AppEvent, PollResult, ServerConfig, ServerId, ServerState};
use crate::poller::{PollerSettings, spawn_poller};
use crate::ui::{format_address, motd_plain};

/// Poller period used by `--once`. The poller polls every server right away
/// and only then waits for the next tick, so a period this long guarantees
/// that no second round starts while results are still being collected.
const ONCE_PERIOD: Duration = Duration::from_secs(60 * 60 * 24 * 365);

/// Number of columns in the `--once` table.
const COLUMNS: usize = 7;

/// Column headers of the `--once` table.
const HEADERS: [&str; COLUMNS] = [
    "STATUS", "NAME", "ADDRESS", "PING", "PLAYERS", "VERSION", "MOTD",
];

/// Spaces between table columns.
const GAP: usize = 2;

/// Command-line options.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// Config file to use [default: <OS config dir>/mcstat/config.toml]
    #[arg(short, long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Refresh interval in seconds (overrides the config file for this run)
    #[arg(
        short,
        long,
        value_name = "SECS",
        value_parser = value_parser!(u64).range(1..)
    )]
    pub interval: Option<u64>,

    /// Timeout of one poll attempt in seconds (overrides the config file for this run)
    #[arg(
        short,
        long,
        value_name = "SECS",
        value_parser = value_parser!(u64).range(1..)
    )]
    pub timeout: Option<u64>,

    /// Retries after a failed poll before a server is shown as offline
    #[arg(
        short,
        long,
        value_name = "N",
        default_value_t = POLL_RETRIES,
        value_parser = value_parser!(u32).range(0..=10)
    )]
    pub retries: u32,

    /// Query every server once, print a table and exit without starting the TUI.
    /// Exit status: 0 if every server is online, 1 otherwise.
    #[arg(long)]
    pub once: bool,

    /// Print the config file path that would be used, then exit
    #[arg(long)]
    pub print_config_path: bool,
}

impl Cli {
    /// The config file to use: `--config` if given, else the OS default.
    pub fn config_path(&self) -> Result<PathBuf, ConfigError> {
        match &self.config {
            Some(path) => Ok(path.clone()),
            None => AppConfig::default_path(),
        }
    }

    /// Refresh interval in effect: the `--interval` flag beats the config value.
    pub fn refresh_interval_secs(&self, config: &AppConfig) -> u64 {
        self.interval.unwrap_or(config.refresh_interval_secs).max(1)
    }

    /// Per-attempt timeout in effect: the `--timeout` flag beats the config value.
    pub fn timeout_secs(&self, config: &AppConfig) -> u64 {
        self.timeout.unwrap_or(config.timeout_secs).max(1)
    }

    /// Poller settings for this run, combining the flags with `config`.
    pub fn poller_settings(&self, config: &AppConfig) -> PollerSettings {
        PollerSettings {
            interval: Duration::from_secs(self.refresh_interval_secs(config)),
            timeout: Duration::from_secs(self.timeout_secs(config)),
            retries: self.retries,
        }
    }
}

/// `--once`: query every configured server concurrently, print one aligned
/// table to stdout and return whether every server answered as online.
pub async fn run_once(servers: &[ServerConfig], settings: PollerSettings) -> Result<bool> {
    if servers.is_empty() {
        bail!("no servers configured; add some to the config file first");
    }

    let states = collect_once(servers, settings).await?;
    let rows: Vec<[String; COLUMNS]> = servers
        .iter()
        .zip(&states)
        .map(|(config, state)| table_row(config, state))
        .collect();

    let mut stdout = io::stdout().lock();
    stdout.write_all(render_table(&rows).as_bytes())?;
    stdout.flush()?;
    Ok(all_online(&states))
}

/// Query every server once through the poller and return one state per
/// server, in the same order as `servers`.
async fn collect_once(
    servers: &[ServerConfig],
    settings: PollerSettings,
) -> Result<Vec<ServerState>> {
    let targets: Vec<(ServerId, ServerConfig)> = servers
        .iter()
        .enumerate()
        .map(|(idx, config)| (id_for_index(idx), config.clone()))
        .collect();

    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    let poller = spawn_poller(
        PollerSettings {
            interval: ONCE_PERIOD,
            ..settings
        },
        targets,
        tx,
    );

    let mut states: Vec<Option<ServerState>> = vec![None; servers.len()];
    let mut pending = servers.len();
    while pending > 0 {
        let Some(event) = rx.recv().await else {
            bail!("poller stopped before every server was queried");
        };
        if let AppEvent::PollFinished(PollResult { id, state, .. }) = event
            && let Some(slot) = index_for_id(id).and_then(|idx| states.get_mut(idx))
            && slot.is_none()
        {
            *slot = Some(state);
            pending -= 1;
        }
    }
    poller.shutdown().await;

    Ok(states.into_iter().map(Option::unwrap_or_default).collect())
}

/// Poller ids for `--once`: the server's position plus one.
fn id_for_index(idx: usize) -> ServerId {
    idx as ServerId + 1
}

fn index_for_id(id: ServerId) -> Option<usize> {
    usize::try_from(id.checked_sub(1)?).ok()
}

/// Whether every server reported [`ServerState::Online`].
fn all_online(states: &[ServerState]) -> bool {
    states
        .iter()
        .all(|state| matches!(state, ServerState::Online(_)))
}

/// One table row: status, name, address, ping, players, version, MOTD.
/// Missing values are shown as `-`; an offline server shows its error in the
/// MOTD column.
fn table_row(config: &ServerConfig, state: &ServerState) -> [String; COLUMNS] {
    let dash = || "-".to_owned();
    let (status, ping, players, version, motd) = match state {
        ServerState::Online(status) => (
            "ONLINE".to_owned(),
            format!("{} ms", status.latency.as_millis()),
            format!("{}/{}", status.players_online, status.players_max),
            or_dash(&status.version_name),
            or_dash(&motd_plain(&status.motd)),
        ),
        ServerState::Offline { error } => (
            "OFFLINE".to_owned(),
            dash(),
            dash(),
            dash(),
            format!("error: {error}"),
        ),
        ServerState::Unknown => ("UNKNOWN".to_owned(), dash(), dash(), dash(), dash()),
    };

    [
        status,
        config.name.clone(),
        format_address(config),
        ping,
        players,
        version,
        motd,
    ]
}

fn or_dash(text: &str) -> String {
    if text.trim().is_empty() {
        "-".to_owned()
    } else {
        text.to_owned()
    }
}

/// Align `rows` under [`HEADERS`]. Widths are measured in terminal columns,
/// so wide characters (CJK, emoji) still line up. The last column is not
/// padded, so lines carry no trailing spaces.
fn render_table(rows: &[[String; COLUMNS]]) -> String {
    let header: [String; COLUMNS] = HEADERS.map(str::to_owned);
    let mut widths: [usize; COLUMNS] = std::array::from_fn(|col| display_width(&header[col]));
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(display_width(cell));
        }
    }

    let mut out = String::new();
    for line in std::iter::once(&header).chain(rows) {
        for (col, (cell, width)) in line.iter().zip(widths).enumerate() {
            out.push_str(cell);
            if col + 1 < COLUMNS {
                let padding = width.saturating_sub(display_width(cell)) + GAP;
                out.extend(std::iter::repeat_n(' ', padding));
            }
        }
        out.push('\n');
    }
    out
}

/// Width of `text` in terminal columns.
fn display_width(text: &str) -> usize {
    Span::raw(text).width()
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener as StdTcpListener;

    use clap::CommandFactory;
    use clap::error::ErrorKind;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::*;
    use crate::model::ServerStatus;
    use crate::protocol::{read_varint_async, write_varint};

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("mcstat").chain(args.iter().copied()))
    }

    fn quick_settings() -> PollerSettings {
        PollerSettings {
            interval: Duration::from_secs(1),
            timeout: Duration::from_secs(5),
            retries: 0,
        }
    }

    fn server(name: &str, port: u16) -> ServerConfig {
        ServerConfig {
            name: name.to_owned(),
            address: "127.0.0.1".to_owned(),
            port: Some(port),
        }
    }

    /// A local port with nothing listening on it.
    fn closed_port() -> u16 {
        let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr").port()
    }

    fn status(version: &str, online: i64, max: i64, motd: serde_json::Value) -> ServerStatus {
        ServerStatus {
            version_name: version.to_owned(),
            protocol_version: 769,
            players_online: online,
            players_max: max,
            player_sample: vec![],
            motd,
            latency: Duration::from_millis(42),
            has_favicon: false,
            resolved_endpoint: "127.0.0.1:25565".to_owned(),
        }
    }

    const FAKE_JSON: &str = r#"{"version":{"name":"Fake 1.21","protocol":769},
        "players":{"max":10,"online":3},"description":"§6Fake"}"#;

    async fn read_frame(sock: &mut TcpStream) {
        let len = read_varint_async(sock).await.expect("frame length");
        let mut buf = vec![0u8; usize::try_from(len).expect("positive length")];
        sock.read_exact(&mut buf).await.expect("frame body");
    }

    /// Minimal Server List Ping responder: reads the handshake and status
    /// request, answers with `json`, then closes. The client's ping then
    /// falls back to the status round trip.
    async fn spawn_status_server(json: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut body = Vec::new();
                write_varint(&mut body, 0x00);
                write_varint(&mut body, i32::try_from(json.len()).expect("len"));
                body.extend_from_slice(json.as_bytes());
                let mut packet = Vec::new();
                write_varint(&mut packet, i32::try_from(body.len()).expect("len"));
                packet.extend_from_slice(&body);
                tokio::spawn(async move {
                    read_frame(&mut sock).await;
                    read_frame(&mut sock).await;
                    let _ = sock.write_all(&packet).await;
                });
            }
        });
        port
    }

    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_flags_yield_defaults() {
        let cli = parse(&[]).expect("no flags is valid");
        assert_eq!(cli.config, None);
        assert_eq!(cli.interval, None);
        assert_eq!(cli.timeout, None);
        assert_eq!(cli.retries, POLL_RETRIES);
        assert!(!cli.once);
        assert!(!cli.print_config_path);
    }

    #[test]
    fn every_flag_is_parsed_in_short_and_long_form() {
        let cli = parse(&[
            "-c",
            "a.toml",
            "-i",
            "2",
            "-t",
            "4",
            "-r",
            "10",
            "--once",
            "--print-config-path",
        ])
        .expect("short flags");
        assert_eq!(cli.config, Some(PathBuf::from("a.toml")));
        assert_eq!(cli.interval, Some(2));
        assert_eq!(cli.timeout, Some(4));
        assert_eq!(cli.retries, 10);
        assert!(cli.once);
        assert!(cli.print_config_path);

        let cli = parse(&[
            "--config=b.toml",
            "--interval=7",
            "--timeout=8",
            "--retries=0",
        ])
        .expect("long flags");
        assert_eq!(cli.config, Some(PathBuf::from("b.toml")));
        assert_eq!(cli.interval, Some(7));
        assert_eq!(cli.timeout, Some(8));
        assert_eq!(cli.retries, 0);
        assert!(!cli.once);
        assert!(!cli.print_config_path);
    }

    #[test]
    fn invalid_values_are_rejected() {
        let cases: &[&[&str]] = &[
            &["-i", "0"],
            &["--interval=0"],
            &["-t", "0"],
            &["--timeout", "0"],
            &["-i", "soon"],
            &["-r", "11"],
            &["--retries=-1"],
            &["-r", "x"],
            &["-c"],
        ];
        for args in cases {
            let err = parse(args).expect_err(&format!("{args:?} should be rejected"));
            assert!(err.use_stderr(), "{args:?} should be a usage error: {err}");
        }
    }

    #[test]
    fn range_violations_are_value_validation_errors() {
        let err = parse(&["-r", "11"]).expect_err("out of range");
        assert_eq!(err.kind(), ErrorKind::ValueValidation);
        let err = parse(&["--interval", "0"]).expect_err("zero interval");
        assert_eq!(err.kind(), ErrorKind::ValueValidation);
    }

    #[test]
    fn help_and_version_are_display_requests() {
        let err = parse(&["--help"]).expect_err("help exits");
        assert_eq!(err.kind(), ErrorKind::DisplayHelp);
        let err = parse(&["--version"]).expect_err("version exits");
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
    }

    #[test]
    fn flags_override_config_values() {
        let config = AppConfig {
            refresh_interval_secs: 30,
            timeout_secs: 9,
            servers: vec![],
        };
        let cli = parse(&["-i", "2", "-t", "4", "-r", "0"]).expect("flags");
        assert_eq!(cli.refresh_interval_secs(&config), 2);
        assert_eq!(cli.timeout_secs(&config), 4);
        assert_eq!(
            cli.poller_settings(&config),
            PollerSettings {
                interval: Duration::from_secs(2),
                timeout: Duration::from_secs(4),
                retries: 0,
            }
        );
    }

    #[test]
    fn config_values_apply_when_flags_are_absent() {
        let config = AppConfig {
            refresh_interval_secs: 30,
            timeout_secs: 9,
            servers: vec![],
        };
        let cli = parse(&[]).expect("no flags");
        assert_eq!(
            cli.poller_settings(&config),
            PollerSettings {
                interval: Duration::from_secs(30),
                timeout: Duration::from_secs(9),
                retries: POLL_RETRIES,
            }
        );
    }

    #[test]
    fn zero_values_in_config_are_clamped_to_one_second() {
        let config = AppConfig {
            refresh_interval_secs: 0,
            timeout_secs: 0,
            servers: vec![],
        };
        let settings = parse(&[]).expect("no flags").poller_settings(&config);
        assert_eq!(settings.interval, Duration::from_secs(1));
        assert_eq!(settings.timeout, Duration::from_secs(1));
    }

    #[test]
    fn interval_override_is_never_written_back_to_the_config_file() {
        let dir = std::env::temp_dir().join(format!("mcstat-cli-override-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.toml");
        AppConfig {
            refresh_interval_secs: 30,
            timeout_secs: 3,
            servers: vec![server("Seed", 25565)],
        }
        .save(&path)
        .expect("seed config");

        let cli = parse(&["-i", "2"]).expect("flag");
        let mut app = crate::state::App::new(
            AppConfig::load_or_create(&path).expect("load"),
            path.clone(),
        );
        app.refresh_interval_secs = cli.refresh_interval_secs(&app.config);
        assert_eq!(app.refresh_interval_secs, 2, "header shows the override");

        // Any save writes `app.config`, which must still hold the file's value.
        app.add_server(server("Added", 25566)).expect("save");
        let on_disk = AppConfig::load_or_create(&path).expect("reload");
        assert_eq!(on_disk.refresh_interval_secs, 30);
        assert_eq!(on_disk.servers.len(), 2);

        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn config_flag_selects_the_config_path() {
        let cli = parse(&["-c", "custom-mcstat.toml"]).expect("flag");
        assert_eq!(
            cli.config_path().expect("explicit path"),
            PathBuf::from("custom-mcstat.toml")
        );

        let cli = parse(&[]).expect("no flags");
        if let Ok(default) = AppConfig::default_path() {
            assert_eq!(cli.config_path().expect("default path"), default);
        }
    }

    #[test]
    fn table_row_formats_online_offline_and_unknown() {
        let config = ServerConfig {
            name: "Hypixel".into(),
            address: "mc.hypixel.net".into(),
            port: None,
        };
        let online = ServerState::Online(status(
            "Requires MC 1.8 / 1.21",
            42_150,
            200_000,
            json!("§6§lHYPIXEL NETWORK\n§aSKYBLOCK"),
        ));
        assert_eq!(
            table_row(&config, &online),
            [
                "ONLINE",
                "Hypixel",
                "mc.hypixel.net",
                "42 ms",
                "42150/200000",
                "Requires MC 1.8 / 1.21",
                "HYPIXEL NETWORK SKYBLOCK",
            ]
            .map(String::from)
        );

        let with_port = ServerConfig {
            name: "DeadBox".into(),
            address: "offline.example.org".into(),
            port: Some(25570),
        };
        let offline = ServerState::Offline {
            error: "connection refused".into(),
        };
        assert_eq!(
            table_row(&with_port, &offline),
            [
                "OFFLINE",
                "DeadBox",
                "offline.example.org:25570",
                "-",
                "-",
                "-",
                "error: connection refused",
            ]
            .map(String::from)
        );

        assert_eq!(
            table_row(&with_port, &ServerState::Unknown),
            [
                "UNKNOWN",
                "DeadBox",
                "offline.example.org:25570",
                "-",
                "-",
                "-",
                "-"
            ]
            .map(String::from)
        );
    }

    #[test]
    fn table_row_shows_dash_for_empty_version_and_motd() {
        let config = server("Empty", 25565);
        let state = ServerState::Online(status("  ", 0, 0, json!("")));
        let row = table_row(&config, &state);
        assert_eq!(row[5], "-");
        assert_eq!(row[6], "-");
        assert_eq!(row[4], "0/0");
    }

    #[test]
    fn render_table_aligns_columns_by_terminal_width() {
        let rows = [
            table_row(
                &ServerConfig {
                    name: "名前".into(),
                    address: "a.example".into(),
                    port: None,
                },
                &ServerState::Online(status("1.21", 0, 1, json!("wide"))),
            ),
            table_row(
                &ServerConfig {
                    name: "Plain".into(),
                    address: "b.example".into(),
                    port: Some(25570),
                },
                &ServerState::Offline {
                    error: "refused".into(),
                },
            ),
        ];
        let table = render_table(&rows);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 3, "header plus two rows:\n{table}");

        let column_start = |line: &str, needle: &str| {
            display_width(&line[..line.find(needle).expect("needle present")])
        };
        let address = column_start(lines[0], "ADDRESS");
        assert_eq!(column_start(lines[1], "a.example"), address);
        assert_eq!(column_start(lines[2], "b.example:25570"), address);
        assert_eq!(column_start(lines[0], "PING"), column_start(lines[2], "-"));

        assert!(
            lines.iter().all(|line| !line.ends_with(' ')),
            "last column is not padded:\n{table}"
        );
    }

    #[test]
    fn all_online_requires_every_server_online() {
        let online = ServerState::Online(status("1.21", 1, 2, json!("")));
        assert!(all_online(std::slice::from_ref(&online)));
        assert!(all_online(&[online.clone(), online.clone()]));
        assert!(!all_online(&[online.clone(), ServerState::Unknown]));
        assert!(!all_online(&[
            online,
            ServerState::Offline { error: "x".into() }
        ]));
    }

    #[tokio::test]
    async fn collect_once_returns_states_in_config_order() {
        let online_port = spawn_status_server(FAKE_JSON).await;
        let servers = vec![server("Dead", closed_port()), server("Alive", online_port)];

        let states = collect_once(&servers, quick_settings())
            .await
            .expect("collect");

        assert_eq!(states.len(), 2);
        assert!(
            matches!(&states[0], ServerState::Offline { .. }),
            "{:?}",
            states[0]
        );
        match &states[1] {
            ServerState::Online(status) => {
                assert_eq!(status.version_name, "Fake 1.21");
                assert_eq!(status.players_online, 3);
                assert_eq!(status.players_max, 10);
            }
            other => panic!("expected Online, got {other:?}"),
        }
        assert!(!all_online(&states));
    }

    #[tokio::test]
    async fn collect_once_reports_all_online_when_every_server_answers() {
        let port_a = spawn_status_server(FAKE_JSON).await;
        let port_b = spawn_status_server(FAKE_JSON).await;
        let servers = vec![server("A", port_a), server("B", port_b)];

        let states = collect_once(&servers, quick_settings())
            .await
            .expect("collect");
        assert!(all_online(&states), "{states:?}");
    }

    #[tokio::test]
    async fn run_once_rejects_an_empty_server_list() {
        let err = run_once(&[], quick_settings())
            .await
            .expect_err("no servers");
        assert!(err.to_string().contains("no servers configured"), "{err}");
    }
}
