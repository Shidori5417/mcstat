//! Terminal user interface (ratatui + crossterm).
//!
//! OWNERSHIP: Gemini 4 (`feat/tui-ui`).
//! Public signatures below are the contract; bodies are implemented across
//! submodules in `src/ui/` while re-exporting the exact entry points
//! `crate::ui::{draw, handle_key, motd_to_lines, motd_plain}`.

mod motd;
mod widgets;

use std::time::Instant;

use crossterm::event::KeyEvent;
use ratatui::Frame;

#[allow(unused_imports)]
pub use self::motd::{motd_plain, motd_to_lines};
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
    let _ = (app, key);
    Action::None
}
