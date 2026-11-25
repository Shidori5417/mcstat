//! Keyboard input routing across application modes (`Normal`, `AddServer`,
//! `ConfirmDelete`, and `Help`).

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::model::{Action, ServerId};
use crate::state::{AddServerForm, App, MessageKind, Mode};

/// Handle a key press according to `app.mode` and return what the main loop
/// must do. Only handle `KeyEventKind::Press` (ignore Release/Repeat on
/// Windows).
pub fn handle_key(app: &mut App, key: KeyEvent) -> Action {
    if key.kind != KeyEventKind::Press {
        return Action::None;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'C'))
    {
        return Action::Quit;
    }

    let mode = std::mem::take(&mut app.mode);
    match mode {
        Mode::Normal => handle_normal_key(app, key),
        Mode::AddServer(form) => handle_add_server_key(app, form, key),
        Mode::ConfirmDelete(id) => handle_confirm_delete_key(app, id, key),
        Mode::Help => Action::None,
    }
}

fn has_ctrl_or_alt(modifiers: KeyModifiers) -> bool {
    modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

fn handle_normal_key(app: &mut App, key: KeyEvent) -> Action {
    if has_ctrl_or_alt(key.modifiers) {
        return Action::None;
    }

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            app.select_prev();
            Action::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.select_next();
            Action::None
        }
        KeyCode::Home | KeyCode::Char('g') => {
            app.select_first();
            Action::None
        }
        KeyCode::End | KeyCode::Char('G') => {
            app.select_last();
            Action::None
        }
        KeyCode::Char('a') => {
            app.mode = Mode::AddServer(AddServerForm::default());
            Action::None
        }
        KeyCode::Char('d') => {
            if let Some(id) = app.selected_entry().map(|entry| entry.id) {
                app.mode = Mode::ConfirmDelete(id);
            }
            Action::None
        }
        KeyCode::Char('r') => {
            app.set_message("Refreshing…", MessageKind::Info);
            Action::RefreshAll
        }
        KeyCode::Char('?') => {
            app.mode = Mode::Help;
            Action::None
        }
        KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
        _ => Action::None,
    }
}

fn handle_add_server_key(app: &mut App, mut form: AddServerForm, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::None,
        KeyCode::BackTab | KeyCode::Up => {
            form.focus = form.focus.prev();
            app.mode = Mode::AddServer(form);
            Action::None
        }
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
            form.focus = form.focus.prev();
            app.mode = Mode::AddServer(form);
            Action::None
        }
        KeyCode::Tab | KeyCode::Down => {
            form.focus = form.focus.next();
            app.mode = Mode::AddServer(form);
            Action::None
        }
        KeyCode::Backspace => {
            form.backspace();
            app.mode = Mode::AddServer(form);
            Action::None
        }
        KeyCode::Enter => match form.to_server_config() {
            Ok(cfg) => {
                let name = cfg.name.clone();
                match app.add_server(cfg) {
                    Ok(_) => {
                        app.set_message(format!("Added {name}"), MessageKind::Info);
                        Action::ServersChanged
                    }
                    Err(err) => {
                        let msg = err.to_string();
                        form.error = Some(msg.clone());
                        app.set_message(msg, MessageKind::Error);
                        app.mode = Mode::AddServer(form);
                        Action::None
                    }
                }
            }
            Err(err) => {
                form.error = Some(err.to_string());
                app.mode = Mode::AddServer(form);
                Action::None
            }
        },
        KeyCode::Char(c) if !has_ctrl_or_alt(key.modifiers) && !c.is_control() => {
            form.input_char(c);
            app.mode = Mode::AddServer(form);
            Action::None
        }
        _ => {
            app.mode = Mode::AddServer(form);
            Action::None
        }
    }
}

fn handle_confirm_delete_key(app: &mut App, id: ServerId, key: KeyEvent) -> Action {
    if has_ctrl_or_alt(key.modifiers) {
        app.mode = Mode::ConfirmDelete(id);
        return Action::None;
    }

    match key.code {
        KeyCode::Char('y' | 'Y') | KeyCode::Enter => match app.remove_server(id) {
            Ok(Some(removed)) => {
                app.set_message(format!("Deleted {}", removed.name), MessageKind::Info);
                Action::ServersChanged
            }
            Ok(None) => {
                app.set_message("Server removed", MessageKind::Info);
                Action::ServersChanged
            }
            Err(err) => {
                app.set_message(err.to_string(), MessageKind::Error);
                Action::ServersChanged
            }
        },
        KeyCode::Char('n' | 'N') | KeyCode::Esc => Action::None,
        _ => {
            app.mode = Mode::ConfirmDelete(id);
            Action::None
        }
    }
}
