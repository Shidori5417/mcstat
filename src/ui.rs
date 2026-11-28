//! Terminal user interface (ratatui + crossterm).
//!
//! Submodules under `src/ui/` implement input handling, MOTD rendering,
//! popups and widgets while keeping the public entry points available as
//! `crate::ui::{draw, handle_key, motd_to_lines, motd_plain}`.

mod input;
mod motd;
mod popups;
mod widgets;

pub(crate) use self::widgets::format_address;

use std::time::Instant;

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::text::Line;

use self::widgets::{RenderView, draw_view, wall_clock_millis};
use crate::model::Action;
use crate::state::App;

/// Render the whole screen:
/// - header (app title, online/total count, refresh interval)
/// - server table (status badge ●Online green / ●Offline red / ○ Unknown
///   gray / spinner while `refreshing`, name, address, ping ms colored by
///   quality, players online/max, version)
/// - detail panel for the selected server (resolved endpoint, protocol,
///   full colored MOTD, player sample, last error, last updated)
/// - footer with key hints + transient `app.message`
/// - popups: add-server form (`Mode::AddServer`), delete confirmation
///   (`Mode::ConfirmDelete`), help (`Mode::Help`)
///
/// Must be responsive: on narrow terminals stack detail below the table and
/// hide low-priority columns.
pub fn draw(frame: &mut Frame, app: &App) {
    let view = RenderView::from_app(app);
    draw_view(frame, &view, Instant::now(), wall_clock_millis());
}

/// Handle a key press according to `app.mode` and return what the main loop
/// must do. Only handle `KeyEventKind::Press` (ignore Release/Repeat on
/// Windows).
///
/// Normal mode: ↑/k up, ↓/j down, g/Home first, G/End last, a add popup,
/// d delete (opens ConfirmDelete), r refresh all, ? help, q/Esc quit,
/// Ctrl+C quit.
/// AddServer: Tab/Shift+Tab/↑/↓ move focus, chars/backspace edit,
/// Enter submit (validate, `app.add_server`, on success `Action::ServersChanged`,
/// on error put message in `form.error`), Esc cancel.
/// ConfirmDelete: y/Enter delete (`app.remove_server`) -> `ServersChanged`,
/// n/Esc cancel.
/// Help: any key closes.
pub fn handle_key(app: &mut App, key: KeyEvent) -> Action {
    input::handle_key(app, key)
}

/// Convert a Minecraft MOTD (`description`) into styled lines.
/// Supports: plain JSON string with legacy `§` codes (0-9a-f colors,
/// k obfuscated, l bold, m strikethrough, n underline, o italic, r reset),
/// chat component objects (`text`, `color` incl. named colors and `#RRGGBB`,
/// `bold`/`italic`/`underlined`/`strikethrough`/`obfuscated`, `extra` with
/// style inheritance, `translate` fallback to the key), arrays of components,
/// and `\n` line breaks.
pub fn motd_to_lines(motd: &serde_json::Value) -> Vec<Line<'static>> {
    motd::motd_to_lines(motd)
}

/// Plain-text version of the MOTD (formatting removed, lines joined by
/// a space) for the narrow table column.
pub fn motd_plain(motd: &serde_json::Value) -> String {
    motd::motd_plain(motd)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use serde_json::json;

    use super::widgets::{RenderView, SPINNER_STEP_MS, draw_view};
    use crate::model::{ServerConfig, ServerState, ServerStatus};
    use crate::state::{AddServerForm, FormField, MessageKind, Mode, ServerEntry, StatusMessage};

    fn render_to_terminal(
        width: u16,
        height: u16,
        view: &RenderView<'_>,
        now: Instant,
        epoch_ms: u128,
    ) -> Terminal<TestBackend> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("in-memory TestBackend");
        terminal
            .draw(|frame| draw_view(frame, view, now, epoch_ms))
            .expect("TestBackend draw is infallible");
        terminal
    }

    fn sample_servers(now: Instant) -> Vec<ServerEntry> {
        vec![
            ServerEntry {
                id: 1,
                config: ServerConfig {
                    name: "Hypixel".into(),
                    address: "mc.hypixel.net".into(),
                    port: None,
                },
                state: ServerState::Online(ServerStatus {
                    version_name: "Requires MC 1.8 / 1.21".into(),
                    protocol_version: 767,
                    players_online: 42_150,
                    players_max: 200_000,
                    player_sample: vec!["Notch".into(), "jeb_".into()],
                    motd: json!("§6§lHYPIXEL NETWORK\n§aSKYBLOCK §7| §eBEDWARS"),
                    latency: Duration::from_millis(42),
                    has_favicon: true,
                    resolved_endpoint: "172.65.128.35:25565".into(),
                }),
                last_updated: Some(now - Duration::from_secs(3)),
                refreshing: false,
            },
            ServerEntry {
                id: 2,
                config: ServerConfig {
                    name: "DeadBox".into(),
                    address: "offline.example.org".into(),
                    port: Some(25570),
                },
                state: ServerState::Offline {
                    error: "connection refused".into(),
                },
                last_updated: Some(now - Duration::from_secs(8)),
                refreshing: false,
            },
            ServerEntry {
                id: 3,
                config: ServerConfig {
                    name: "FreshSrv".into(),
                    address: "play.fresh.gg".into(),
                    port: None,
                },
                state: ServerState::Unknown,
                last_updated: None,
                refreshing: true,
            },
        ]
    }

    #[test]
    fn render_empty_state_shows_header_add_hint_and_footer() {
        let now = Instant::now();
        let mode = Mode::Normal;
        let view = RenderView {
            servers: &[],
            selected: 0,
            selected_entry: None,
            online_count: (0, 0),
            refresh_interval_secs: 5,
            last_refresh: Some(now - Duration::from_secs(1)),
            message: None,
            mode: &mode,
        };

        let term = render_to_terminal(120, 24, &view, now, 0);
        let backend = term.backend();
        let screen = backend.to_string();
        assert!(screen.contains("mcstat"));
        assert!(screen.contains("0/0 online"));
        assert!(screen.contains("refresh 5s"));
        assert!(screen.contains("next in 4s"));
        assert!(screen.contains("press a to add a server"));
        assert!(screen.contains("↑↓/jk move · a add · d delete · r refresh · ? help · q quit"));
        assert!(!backend.cursor_visible());
    }

    #[test]
    fn render_wide_layout_shows_table_badges_spinner_and_selected_details() {
        let now = Instant::now();
        let servers = sample_servers(now);
        let mode = Mode::Normal;
        let view = RenderView {
            servers: &servers,
            selected: 0,
            selected_entry: servers.first(),
            online_count: (1, 3),
            refresh_interval_secs: 10,
            last_refresh: Some(now - Duration::from_secs(4)),
            message: None,
            mode: &mode,
        };

        // `SPINNER_STEP_MS * 2` selects the 3rd braille frame `"⠹"`
        let term = render_to_terminal(180, 28, &view, now, SPINNER_STEP_MS * 2);
        let screen = term.backend().to_string();

        // Header
        assert!(screen.contains("1/3 online"));
        assert!(screen.contains("next in 6s"));

        // Table badges & columns (including wide MOTD plain text joined on one line)
        assert!(screen.contains("● Online"));
        assert!(screen.contains("● Offline"));
        assert!(screen.contains("⠹ Polling"));
        assert!(screen.contains("42 ms"));
        assert!(screen.contains("42150/200000"));
        assert!(screen.contains("offline.example.org:25570"));
        assert!(screen.contains("HYPIXEL NETWORK SKYBLOCK"));

        // Detail panel on right
        assert!(screen.contains("172.65.128.35:25565"));
        assert!(screen.contains("protocol 767"));
        assert!(screen.contains("Notch, jeb_"));
        assert!(screen.contains("updated 3s ago"));
        assert!(screen.contains("HYPIXEL NETWORK"));
    }

    #[test]
    fn render_narrow_layout_stacks_detail_and_shows_offline_error_and_status_message() {
        let now = Instant::now();
        let servers = sample_servers(now);
        let mode = Mode::Normal;
        let msg = StatusMessage {
            text: "Failed to reach DeadBox".into(),
            kind: MessageKind::Error,
            created: now,
        };
        let view = RenderView {
            servers: &servers,
            selected: 1,
            selected_entry: servers.get(1),
            online_count: (1, 3),
            refresh_interval_secs: 5,
            last_refresh: Some(now),
            message: Some(&msg),
            mode: &mode,
        };

        let term = render_to_terminal(68, 26, &view, now, 0);
        let backend = term.backend();
        let screen = backend.to_string();

        // Offline error appears in stacked detail panel
        assert!(screen.contains("Details — DeadBox"));
        assert!(screen.contains("connection refused"));
        assert!(screen.contains("updated 8s ago"));
        // Footer status message rendered in Error red
        assert!(screen.contains("Failed to reach DeadBox"));
        let buf = backend.buffer();
        let bottom_y = buf.area.height - 1;
        let has_red_footer_cell = (0..buf.area.width).any(|x| buf[(x, bottom_y)].fg == Color::Red);
        assert!(has_red_footer_cell);
    }

    #[test]
    fn render_popups_set_cursor_and_show_confirm_and_help() {
        let now = Instant::now();
        let servers = sample_servers(now);

        // 1. AddServer popup with validation error and cursor in Address field
        let add_mode = Mode::AddServer(AddServerForm {
            name: "MySMP".into(),
            address: "play.mysmp.net".into(),
            port: String::new(),
            focus: FormField::Address,
            error: Some("invalid server: port out of range".into()),
        });
        let add_view = RenderView {
            servers: &servers,
            selected: 0,
            selected_entry: servers.first(),
            online_count: (1, 3),
            refresh_interval_secs: 5,
            last_refresh: Some(now),
            message: None,
            mode: &add_mode,
        };
        let add_term = render_to_terminal(100, 26, &add_view, now, 0);
        let add_backend = add_term.backend();
        let add_screen = add_backend.to_string();
        assert!(add_screen.contains("Add Server"));
        assert!(add_screen.contains("Port (optional)"));
        assert!(add_screen.contains("invalid server: port out of range"));
        assert!(add_screen.contains("Tab next · Enter save · Esc cancel"));
        assert!(add_backend.cursor_visible());
        let pos = add_backend.cursor_position();
        assert!(pos.x < 100 && pos.y < 26);

        // 2. ConfirmDelete popup
        let del_mode = Mode::ConfirmDelete(1);
        let del_view = RenderView {
            mode: &del_mode,
            ..add_view
        };
        let del_term = render_to_terminal(100, 26, &del_view, now, 0);
        assert!(
            del_term
                .backend()
                .to_string()
                .contains("Delete Hypixel? (y/n)")
        );

        // 3. Help popup
        let help_mode = Mode::Help;
        let help_view = RenderView {
            mode: &help_mode,
            ..add_view
        };
        let help_term = render_to_terminal(100, 26, &help_view, now, 0);
        let help_screen = help_term.backend().to_string();
        assert!(help_screen.contains("Help — Keybindings"));
        assert!(help_screen.contains("Ctrl+C"));
    }

    #[test]
    fn tiny_terminals_never_panic_in_any_mode() {
        let now = Instant::now();
        let servers = sample_servers(now);
        let modes = [
            Mode::Normal,
            Mode::AddServer(AddServerForm {
                name: "VeryLongServerNameThatExceedsTwentyColumns".into(),
                address: "mc.very-long-hostname.example.com".into(),
                port: "25565".into(),
                focus: FormField::Port,
                error: Some("validation error".into()),
            }),
            Mode::ConfirmDelete(1),
            Mode::Help,
        ];
        let sizes = [(20, 5), (15, 3), (8, 2), (3, 1), (1, 1), (0, 0)];

        for (w, h) in sizes {
            for mode in &modes {
                let view = RenderView {
                    servers: &servers,
                    selected: 0,
                    selected_entry: servers.first(),
                    online_count: (1, 3),
                    refresh_interval_secs: 5,
                    last_refresh: Some(now),
                    message: None,
                    mode,
                };
                let term = render_to_terminal(w, h, &view, now, 500);
                let backend = term.backend();
                if backend.cursor_visible() && w > 0 && h > 0 {
                    let pos = backend.cursor_position();
                    assert!(pos.x < w && pos.y < h);
                }
            }
        }
    }
}
