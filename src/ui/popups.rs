//! Modal popups rendered over a `Clear`ed centered area:
//! - `Mode::AddServer`: Name / Address / Port (optional) form with live cursor
//! - `Mode::ConfirmDelete`: `Delete <name>? (y/n)` confirmation dialog
//! - `Mode::Help`: full keybinding reference

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};

use super::widgets::centered_rect;
use crate::model::ServerId;
use crate::state::{AddServerForm, FormField, Mode, ServerEntry};

const POPUP_BG: Color = Color::Rgb(20, 24, 34);

/// Render the active modal popup (if any) on top of the main UI.
pub(crate) fn draw_popup(frame: &mut Frame, area: Rect, mode: &Mode, servers: &[ServerEntry]) {
    if area.is_empty() {
        return;
    }
    match mode {
        Mode::Normal => {}
        Mode::AddServer(form) => draw_add_server_popup(frame, area, form),
        Mode::ConfirmDelete(id) => draw_confirm_delete_popup(frame, area, servers, *id),
        Mode::Help => draw_help_popup(frame, area),
    }
}

/// Return the trailing slice of `text` whose display width fits within
/// `max_cols.saturating_sub(1)` columns (leaving 1 cell for the cursor),
/// along with that slice's display width in columns.
pub(crate) fn visible_input_tail(text: &str, max_cols: u16) -> (&str, u16) {
    let avail = usize::from(max_cols.saturating_sub(1));
    if avail == 0 || text.is_empty() {
        return ("", 0);
    }

    let full_width = Span::raw(text).width();
    if full_width <= avail {
        return (text, full_width as u16);
    }

    let mut used = 0usize;
    let mut start_idx = text.len();
    let mut buf = [0u8; 4];
    for (idx, ch) in text.char_indices().rev() {
        let ch_width = Span::raw(ch.encode_utf8(&mut buf) as &str).width();
        if used.saturating_add(ch_width) > avail {
            break;
        }
        used = used.saturating_add(ch_width);
        start_idx = idx;
    }

    (&text[start_idx..], used as u16)
}

fn set_cursor_clamped(frame: &mut Frame, bounds: Rect, x: u16, y: u16) {
    if bounds.is_empty() {
        return;
    }
    if x >= bounds.x && x < bounds.right() && y >= bounds.y && y < bounds.bottom() {
        frame.set_cursor_position(Position::new(x, y));
    }
}

fn draw_add_server_popup(frame: &mut Frame, area: Rect, form: &AddServerForm) {
    let desired_height = if form.error.is_some() { 15 } else { 14 };
    let desired_width = area.width.saturating_sub(4).clamp(20, 56);
    let popup_area = centered_rect(area, desired_width, desired_height);
    if popup_area.is_empty() {
        return;
    }

    frame.render_widget(Clear, popup_area);

    let outer = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .style(Style::default().bg(POPUP_BG).fg(Color::White))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            " Add Server ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = outer.inner(popup_area);
    frame.render_widget(outer, popup_area);
    if inner.is_empty() {
        return;
    }

    let fields = [
        (FormField::Name, "Name", form.name.as_str(), "e.g. Hypixel"),
        (
            FormField::Address,
            "Address",
            form.address.as_str(),
            "e.g. mc.hypixel.net",
        ),
        (
            FormField::Port,
            "Port (optional)",
            form.port.as_str(),
            "25565 or blank for SRV",
        ),
    ];

    if inner.height >= 11 && inner.width >= 12 {
        let mut constraints = vec![
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
        ];
        if form.error.is_some() {
            constraints.push(Constraint::Min(1));
        } else {
            constraints.push(Constraint::Min(0));
        }
        constraints.push(Constraint::Length(1));

        let chunks = Layout::vertical(constraints).split(inner);

        for (i, (field, label, value, placeholder)) in fields.iter().enumerate() {
            let focused = form.focus == *field;
            draw_boxed_input(frame, area, chunks[i], label, value, placeholder, focused);
        }

        if let Some(err) = &form.error
            && !chunks[3].is_empty()
        {
            let err_widget = Paragraph::new(Span::styled(
                err.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ))
            .wrap(Wrap { trim: true });
            frame.render_widget(err_widget, chunks[3]);
        }

        draw_form_hint(frame, chunks[4]);
    } else {
        // Compact fallback for short/tiny terminals (e.g. 20×5).
        let mut row_y = inner.y;
        let bottom = inner.bottom();

        for (field, label, value, _placeholder) in fields {
            if row_y >= bottom {
                break;
            }
            let row_rect = Rect::new(inner.x, row_y, inner.width, 1);
            let focused = form.focus == field;
            draw_compact_input(frame, area, row_rect, label, value, focused);
            row_y = row_y.saturating_add(1);
        }

        if let Some(err) = &form.error
            && row_y < bottom
        {
            let err_rect = Rect::new(inner.x, row_y, inner.width, 1);
            frame.render_widget(
                Paragraph::new(Span::styled(err.clone(), Style::default().fg(Color::Red))),
                err_rect,
            );
            row_y = row_y.saturating_add(1);
        }

        if row_y < bottom {
            let hint_rect = Rect::new(inner.x, bottom.saturating_sub(1), inner.width, 1);
            draw_form_hint(frame, hint_rect);
        }
    }
}

fn draw_boxed_input(
    frame: &mut Frame,
    viewport: Rect,
    rect: Rect,
    label: &str,
    value: &str,
    placeholder: &str,
    focused: bool,
) {
    if rect.is_empty() {
        return;
    }

    let (border_style, title_style) = if focused {
        (
            Style::default().fg(Color::Cyan),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::Gray),
        )
    };

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(Span::styled(format!(" {label} "), title_style));

    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.is_empty() {
        return;
    }

    if value.is_empty() && !focused {
        frame.render_widget(
            Paragraph::new(Span::styled(
                placeholder,
                Style::default().fg(Color::DarkGray),
            )),
            inner,
        );
    } else {
        let (visible, width) = visible_input_tail(value, inner.width);
        let text_style = if focused {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::Gray)
        };
        frame.render_widget(Paragraph::new(Span::styled(visible, text_style)), inner);
        if focused {
            set_cursor_clamped(frame, viewport, inner.x.saturating_add(width), inner.y);
        }
    }
}

fn draw_compact_input(
    frame: &mut Frame,
    viewport: Rect,
    rect: Rect,
    label: &str,
    value: &str,
    focused: bool,
) {
    if rect.is_empty() {
        return;
    }

    let short_label = match label {
        "Port (optional)" if rect.width < 28 => "Port",
        other => other,
    };
    let prefix = format!("{short_label}: ");
    let prefix_width = (Span::raw(&prefix).width() as u16).min(rect.width);
    let label_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };

    let input_x = rect.x.saturating_add(prefix_width);
    let input_width = rect.width.saturating_sub(prefix_width);
    let (visible, val_width) = visible_input_tail(value, input_width);

    let line = Line::from(vec![
        Span::styled(prefix, label_style),
        Span::styled(visible, Style::default().fg(Color::White)),
    ]);
    frame.render_widget(Paragraph::new(line), rect);

    if focused && input_width > 0 {
        set_cursor_clamped(frame, viewport, input_x.saturating_add(val_width), rect.y);
    }
}

fn draw_form_hint(frame: &mut Frame, rect: Rect) {
    if rect.is_empty() {
        return;
    }
    let key = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let desc = Style::default().fg(Color::Gray);
    let dot = Style::default().fg(Color::DarkGray);
    let line = Line::from(vec![
        Span::styled("Tab", key),
        Span::styled(" next", desc),
        Span::styled(" · ", dot),
        Span::styled("Enter", key),
        Span::styled(" save", desc),
        Span::styled(" · ", dot),
        Span::styled("Esc", key),
        Span::styled(" cancel", desc),
    ]);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), rect);
}

fn draw_confirm_delete_popup(frame: &mut Frame, area: Rect, servers: &[ServerEntry], id: ServerId) {
    let name = servers
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.config.name.as_str())
        .unwrap_or("server");

    let prompt = format!("Delete {name}? (y/n)");
    let desired_width = (Span::raw(&prompt).width() as u16)
        .saturating_add(10)
        .clamp(28, 60);
    let popup_area = centered_rect(area, desired_width, 7);
    if popup_area.is_empty() {
        return;
    }

    frame.render_widget(Clear, popup_area);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Red))
        .style(Style::default().bg(POPUP_BG).fg(Color::White))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            " Confirm Delete ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);
    if inner.is_empty() {
        return;
    }

    let mut lines = Vec::new();
    if inner.height >= 4 {
        lines.push(Line::default());
    }
    lines.push(Line::from(Span::styled(
        prompt,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    if inner.height >= 3 {
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled(
                "y / Enter",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" confirm · ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "n / Esc",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
        ]));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        inner,
    );
}

fn draw_help_popup(frame: &mut Frame, area: Rect) {
    let popup_area = centered_rect(area, 58, 20);
    if popup_area.is_empty() {
        return;
    }

    frame.render_widget(Clear, popup_area);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .style(Style::default().bg(POPUP_BG).fg(Color::White))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            " Help — Keybindings ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(Span::styled(
                " Press any key to close ",
                Style::default().fg(Color::DarkGray),
            ))
            .centered(),
        );

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);
    if inner.is_empty() {
        return;
    }

    let heading = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let help_row = |keys: &'static str, action: &'static str| -> Line<'static> {
        Line::from(vec![
            Span::styled(
                format!("{keys:<16}"),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(action, Style::default().fg(Color::White)),
        ])
    };

    let lines = vec![
        Line::from(Span::styled("Normal Mode", heading)),
        help_row("↑ / k", "Move selection up"),
        help_row("↓ / j", "Move selection down"),
        help_row("g / Home", "Jump to first server"),
        help_row("G / End", "Jump to last server"),
        help_row("a", "Add a new server"),
        help_row("d", "Delete selected server"),
        help_row("r", "Refresh all servers now"),
        help_row("?", "Toggle help popup"),
        help_row("q / Esc", "Quit mcstat"),
        Line::default(),
        Line::from(Span::styled("Popups & Global", heading)),
        help_row("Tab / ↓", "Next input field (Add Server)"),
        help_row("Shift+Tab / ↑", "Previous input field (Add Server)"),
        help_row("Enter / Esc", "Confirm / cancel active dialog"),
        help_row("y / n", "Confirm / cancel deletion"),
        help_row("Ctrl+C", "Force quit from any mode"),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_input_tail_clips_and_handles_unicode() {
        assert_eq!(visible_input_tail("", 10), ("", 0));
        assert_eq!(visible_input_tail("abc", 0), ("", 0));
        assert_eq!(visible_input_tail("abc", 1), ("", 0));
        assert_eq!(
            visible_input_tail("mc.hypixel.net", 20),
            ("mc.hypixel.net", 14)
        );

        // Scrolls to keep trailing chars + 1 cursor cell inside `max_cols`
        let (tail, width) = visible_input_tail("play.very-long-domain.example.net", 12);
        assert_eq!(tail, "example.net");
        assert_eq!(width, 11);

        // Multi-byte UTF-8 never panics on char boundaries
        let (utf8_tail, utf8_w) = visible_input_tail("sunucu-şğüöç", 6);
        assert_eq!(utf8_tail, "şğüöç");
        assert_eq!(utf8_w, 5);
    }
}
