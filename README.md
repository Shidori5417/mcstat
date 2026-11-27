# mcstat

[![CI](https://github.com/Shidori5417/mcstat/actions/workflows/ci.yml/badge.svg)](https://github.com/Shidori5417/mcstat/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A live terminal monitor for Minecraft Java Edition servers. It polls every
configured server in parallel with the Server List Ping protocol and shows
status, latency, player counts, version and MOTD in a
[ratatui](https://ratatui.rs) dashboard. A `--once` mode prints a plain table
for scripts.

## Features

- Live table of all configured servers, refreshed automatically on an interval
- Parallel, non-blocking polling: a slow or dead server never delays the others
- Per-server details: resolved endpoint, protocol, players and sample names,
  favicon flag, last error and last update time
- MOTD rendering with legacy `§` colour codes and chat-component JSON
- DNS SRV record lookup (`_minecraft._tcp.<host>`) when no port is set
- Add and delete servers from inside the TUI; changes are saved to the config
- `--once` mode for scripts: one aligned table, exit status 0 when all servers are online
- Built on crossterm and `dirs`, so it targets Linux, macOS and Windows.
  Development and testing so far were on Linux.

## Screenshot

Rendered frame with three servers (one online, one offline, one still polling):

```text
 ⛏ mcstat  ● 1/3 online │ refresh 5s │ next in 3s
╭ Servers (3) ──────────────────────────────────────────────────────────────╮╭ Details — Hypixel ──────────────────────╮
│  Status    Name     Address              Ping    Players      Version     ││ Server:   Hypixel  ● Online  (updated   │
│▸ ● Online  Hypixel  mc.hypixel.net       42 ms   42150/200000 Requires MC ││ 3s ago)                                 │
│  ● Offline DeadBox  offline.example.org: —       —            —           ││ Address:  mc.hypixel.net (SRV / 25565)  │
│  ⠹ Polling FreshSrv play.fresh.gg        —       —            —           ││ Endpoint: 172.65.128.35:25565 · favicon │
│                                                                           ││ ✓                                       │
│                                                                           ││ Version:  Requires MC 1.8 / 1.21        │
│                                                                           ││ (protocol 767)                          │
│                                                                           ││ Ping:     42 ms                         │
│                                                                           ││ Players:  42150/200000                  │
│                                                                           ││ Sample:   Notch, jeb_                   │
│                                                                           ││                                         │
│                                                                           ││ MOTD:                                   │
│                                                                           ││ HYPIXEL NETWORK                         │
│                                                                           ││ SKYBLOCK | BEDWARS                      │
│                                                                           ││                                         │
│                                                                           ││                                         │
│                                                                           ││                                         │
│                                                                           ││                                         │
│                                                                           ││                                         │
│                                                                           ││                                         │
╰───────────────────────────────────────────────────────────────────────────╯╰─────────────────────────────────────────╯
 ↑↓/jk move · a add · d delete · r refresh · ? help · q quit
```

## Installation

mcstat needs Rust 1.88 or newer (Rust edition 2024).

Install the binary into `~/.cargo/bin`:

```sh
cargo install --git https://github.com/Shidori5417/mcstat
# or from a local checkout:
cargo install --path .
```

Or run it straight from the checkout:

```sh
cargo run --release
```

## Usage

```text
mcstat [OPTIONS]

Options:
  -c, --config <PATH>      Config file to use [default: <OS config dir>/mcstat/config.toml]
  -i, --interval <SECS>    Refresh interval in seconds (overrides the config file for this run)
  -t, --timeout <SECS>     Timeout of one poll attempt in seconds (overrides the config file for this run)
  -r, --retries <N>        Retries after a failed poll before a server is shown as offline [default: 1]
      --once               Query every server once and print a table, without starting the TUI
      --print-config-path  Print the config file path that would be used, then exit
  -h, --help               Print help
  -V, --version            Print version
```

- `-i`, `-t` and `-c` only apply to the current run. Nothing they set is written
  back to the config file; adding or deleting a server in the TUI still saves
  the file's own values.
- `-i` must be at least 1. `-t` must be at least 1. `-r` accepts 0 to 10.
- `--once` ignores `-i`; it honours `-t` and `-r`.

### `--once` for scripts

`--once` polls all servers concurrently, prints one table and exits. The exit
status is `0` when every server is online and `1` when any server is offline.
An empty server list is an error (exit status `1`).

```text
$ mcstat --once
STATUS  NAME       ADDRESS             PING    PLAYERS       VERSION                 MOTD
ONLINE  Hypixel    mc.hypixel.net      159 ms  14433/200000  Requires MC 1.8 / 1.21  Hypixel Network [1.8/26.3] SB MAGIC FIND, ACCESSORY BAG REWORK
ONLINE  CubeCraft  play.cubecraft.net  73 ms   389/5000      CubeCraft               ⏴ CᴜʙᴇCʀᴀꜰᴛ Gᴀᴍᴇꜱ [EU] ⏵ BEDWARS + PILLARS + SKYBLOCK
ONLINE  2b2t       2b2t.org            56 ms   1070/1        Velocity 1.7.2-26.3     2B Updated to 1.21.4 2T 2b2t.org - status.2b2t.org
$ echo $?
0
```

An offline server shows `OFFLINE` and its error in the MOTD column. Values
change from run to run; the output above is a real run with the default servers.

```sh
mcstat --once -t 2 -r 0 || notify-send "Minecraft server down"
```

## Keybindings

| Key | Action |
| --- | --- |
| `↑` / `k` | Select the previous server |
| `↓` / `j` | Select the next server |
| `g` / `Home` | Select the first server |
| `G` / `End` | Select the last server |
| `a` | Open the add-server form |
| `d` | Delete the selected server (asks for confirmation) |
| `r` | Refresh all servers now |
| `?` | Show the help popup |
| `q` / `Esc` | Quit |
| `Ctrl+C` | Quit |

Popup keys:

| Popup | Keys |
| --- | --- |
| Add server | `Tab` / `↓` next field, `Shift+Tab` / `↑` previous field, type to edit, `Backspace` delete, `Enter` save, `Esc` cancel |
| Delete confirmation | `y` / `Enter` delete, `n` / `Esc` keep |
| Help | any key closes it |

In the add-server form the port field accepts digits only (up to five). An
address may also contain the port (`host:port` or `[ipv6]:port`), but not both.

## Configuration

The config file is TOML. It is created with the defaults on first start if it does not exist.

| OS | Default location |
| --- | --- |
| Linux | `$XDG_CONFIG_HOME/mcstat/config.toml` (usually `~/.config/mcstat/config.toml`) |
| macOS | `~/Library/Application Support/mcstat/config.toml` |
| Windows | `%APPDATA%\mcstat\config.toml` |

Run `mcstat --print-config-path` to see the exact path on your system.

Example:

```toml
# Seconds between automatic refreshes (default 5, minimum 1).
refresh_interval_secs = 5

# Timeout for one poll attempt, in seconds (default 3, minimum 1).
timeout_secs = 3

[[servers]]
name = "Hypixel"            # shown in the table
address = "mc.hypixel.net"  # hostname or IP, no port

[[servers]]
name = "Survival"
address = "play.example.net"
port = 25570                # optional; skips the SRV lookup

[[servers]]
name = "Local"
address = "::1"             # bare IPv6 literal, as the add form writes it
```

Fields:

- `refresh_interval_secs` and `timeout_secs` are optional; missing values use the defaults shown above.
- `servers` is a list of `[[servers]]` tables. `name` and `address` are required; `port` is optional.
- Unknown keys are ignored. A misspelled key silently falls back to its default,
  and the key is dropped the next time mcstat saves the file.

To use a different file, pass `--config /path/to/file.toml`.

## How servers are reached

- **Explicit port:** when `port` is set, mcstat connects to `address:port` directly.
- **IP address:** an IPv4 or IPv6 literal without a port connects to port 25565.
- **Hostname without a port:** mcstat looks up the DNS SRV record
  `_minecraft._tcp.<address>`. If it exists, mcstat connects to the target host
  and port from the record (lowest priority, then highest weight) and still
  sends the original address in the handshake. If there is no record, or the
  lookup fails or times out, mcstat falls back to `address:25565`.

SRV lookups use the system DNS configuration and fall back to Cloudflare
(1.1.1.1) when that is unavailable. Results are cached for five minutes.

Each poll attempt is bounded by `timeout_secs`, covering connect, the status
exchange and the ping. A failed attempt is retried after a short linear backoff
(300 ms per attempt, `--retries` times). If every attempt fails the server is
shown as offline with the last error.

## Architecture

| Module | Responsibility |
| --- | --- |
| `model` | Shared data types: server config, poll results, app events, actions |
| `protocol` | Minecraft Server List Ping: VarInt framing, handshake, status and ping, JSON parsing |
| `poller` | Background task that polls servers concurrently, resolves SRV records, applies timeouts and retries |
| `config` | TOML config: load, atomic save, validation of user-entered server addresses |
| `state` | Application state: server list, selection, popups, status messages, persistence of changes |
| `ui` | ratatui rendering (`widgets`, `popups`, `motd`) and key handling (`input`) |
| `cli` | Command-line flags and the `--once` plain-text mode |
| `main` | Terminal setup and teardown, panic hook, the main event loop |

### Event flow

```text
 input thread ──┐  crossterm key/resize events
 ticker (250ms)─┼─▶ AppEvent ──▶ mpsc channel ──▶ main loop ──▶ ui::draw
 poller task ───┘  PollStarted / PollFinished          │
                                                       ├─ Action::RefreshAll    ─▶ poller.refresh_all()
                                                       └─ Action::ServersChanged ─▶ poller.set_servers()

 poller task: one JoinSet entry per server (no overlapping polls for the same server)
              └─ attempt = SRV lookup + SLP exchange, bounded by timeout_secs
```

- The **input thread** blocks on `crossterm` and forwards events to the channel.
  It checks a stop flag every 100 ms so shutdown is bounded.
- The **ticker** sends a tick every 250 ms for the spinner, message expiry and
  the refresh countdown.
- The **poller** polls every server on the refresh interval, plus immediately on
  `r` and when a server is added. A server with a poll still in flight is not polled again.
- The **main loop** owns the `App` state, handles each event, redraws after
  each one, and tells the poller when the list changes.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Two tests talk to the real internet (a Hypixel status query and an SRV lookup
for 2b2t.org). They are ignored by default. Run them with:

```sh
cargo test -- --ignored
```

## License

MIT. See [LICENSE](LICENSE).
