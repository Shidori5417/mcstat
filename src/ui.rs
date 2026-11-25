//! Terminal user interface (ratatui + crossterm).
//!
//! OWNERSHIP: Gemini 4 (`feat/tui-ui`).
//! Public signatures below are the contract; bodies are implemented across
//! submodules in `src/ui/` while keeping the entry points available as
//! `crate::ui::{draw, handle_key, motd_to_lines, motd_plain}`.

mod input;
mod motd;
mod popups;
mod widgets;

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
