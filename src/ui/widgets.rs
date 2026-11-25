//! Responsive screen layout, header, server table, detail panel, and footer.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, HighlightSpacing, Padding, Paragraph, Row, Table, TableState, Wrap,
};

use super::motd::{motd_plain, motd_to_lines, strip_legacy_codes};
use crate::model::{ServerConfig, ServerState};
use crate::state::{App, MessageKind, Mode, ServerEntry, StatusMessage};

/// Minimum terminal width (in columns) for placing table and detail side-by-side.
pub(crate) const WIDE_LAYOUT_MIN_COLS: u16 = 110;

/// Braille animation frames for in-flight server polls.
pub(crate) const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Milliseconds per spinner frame (matches the 250ms UI tick in `main.rs`).
pub(crate) const SPINNER_STEP_MS: u128 = 250;

/// Columns that may be shown in the server table depending on available width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableColumn {
    Status,
    Name,
    Address,
    Ping,
    Players,
    Version,
    Motd,
}

const COLS_ALL: &[TableColumn] = &[
    TableColumn::Status,
    TableColumn::Name,
    TableColumn::Address,
    TableColumn::Ping,
    TableColumn::Players,
    TableColumn::Version,
    TableColumn::Motd,
];

const COLS_NO_MOTD: &[TableColumn] = &[
    TableColumn::Status,
    TableColumn::Name,
    TableColumn::Address,
    TableColumn::Ping,
    TableColumn::Players,
    TableColumn::Version,
];

const COLS_NO_VERSION: &[TableColumn] = &[
    TableColumn::Status,
    TableColumn::Name,
    TableColumn::Address,
    TableColumn::Ping,
    TableColumn::Players,
];

const COLS_CORE: &[TableColumn] = &[
    TableColumn::Status,
    TableColumn::Name,
    TableColumn::Ping,
    TableColumn::Players,
];

const COLS_TINY: &[TableColumn] = &[TableColumn::Status, TableColumn::Name];

/// Read-only snapshot of UI state so rendering can be driven both from `&App`
/// at runtime and from `TestBackend` tests without calling stubbed `App` methods.
pub(crate) struct RenderView<'a> {
    pub servers: &'a [ServerEntry],
    pub selected: usize,
    pub selected_entry: Option<&'a ServerEntry>,
    pub online_count: (usize, usize),
    pub refresh_interval_secs: u64,
    pub last_refresh: Option<Instant>,
    pub message: Option<&'a StatusMessage>,
    #[allow(dead_code)]
    pub mode: &'a Mode,
}

impl<'a> RenderView<'a> {
    pub(crate) fn from_app(app: &'a App) -> Self {
        Self {
            servers: &app.servers,
            selected: app.selected,
            selected_entry: app.selected_entry(),
            online_count: app.online_count(),
            refresh_interval_secs: app.config.refresh_interval_secs,
            last_refresh: app.last_refresh,
            message: app.message.as_ref(),
            mode: &app.mode,
        }
    }
}

/// Whether the main area uses horizontal side-by-side panels (`>= 110` cols).
pub(crate) const fn is_side_by_side(width: u16) -> bool {
    width >= WIDE_LAYOUT_MIN_COLS
}

/// Select visible table columns for a given table area width.
/// Progressively hides MOTD, then Version, then Address, and finally metrics on tiny widths.
pub(crate) const fn columns_for_width(width: u16) -> &'static [TableColumn] {
    if width >= 76 {
        COLS_ALL
    } else if width >= 64 {
        COLS_NO_MOTD
    } else if width >= 48 {
        COLS_NO_VERSION
    } else if width >= 30 {
        COLS_CORE
    } else {
        COLS_TINY
    }
}

/// Color latency in milliseconds: `< 80` green, `< 200` yellow, otherwise red.
pub(crate) const fn ping_color(ms: u128) -> Color {
    if ms < 80 {
        Color::Green
    } else if ms < 200 {
        Color::Yellow
    } else {
        Color::Red
    }
}

/// Choose a braille spinner glyph deterministically from wall-clock milliseconds.
pub(crate) const fn spinner_frame(now_epoch_ms: u128) -> &'static str {
    let idx = ((now_epoch_ms / SPINNER_STEP_MS) % (SPINNER_FRAMES.len() as u128)) as usize;
    SPINNER_FRAMES[idx]
}

/// Current wall-clock timestamp in milliseconds since UNIX_EPOCH.
pub(crate) fn wall_clock_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Format a configured server address as `host` or `host:port` (bracketed for raw IPv6).
pub(crate) fn format_address(cfg: &ServerConfig) -> String {
    match cfg.port {
        Some(port) => {
            if cfg.address.contains(':') && !cfg.address.starts_with('[') {
                format!("[{}]:{port}", cfg.address)
            } else {
                format!("{}:{port}", cfg.address)
            }
        }
        None => cfg.address.clone(),
    }
}

/// Compute remaining seconds until the next scheduled auto-refresh.
pub(crate) fn seconds_until_refresh(
    interval_secs: u64,
    last_refresh: Option<Instant>,
    now: Instant,
) -> Option<u64> {
    let last = last_refresh?;
    let elapsed = now.saturating_duration_since(last).as_secs();
    Some(interval_secs.saturating_sub(elapsed))
}

/// Center a rectangle of at most `(target_width, target_height)` within `area`.
/// Safe for arbitrarily small or zero-sized areas.
#[allow(dead_code)]
pub(crate) fn centered_rect(area: Rect, target_width: u16, target_height: u16) -> Rect {
    let width = target_width.min(area.width);
    let height = target_height.min(area.height);
    let x = area.x.saturating_add(area.width.saturating_sub(width) / 2);
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height) / 2);
    Rect::new(x, y, width, height)
}

/// Render the entire screen from a [`RenderView`].
pub(crate) fn draw_view(
    frame: &mut Frame,
    view: &RenderView<'_>,
    now: Instant,
    now_epoch_ms: u128,
) {
    let area = frame.area();
    if area.is_empty() {
        return;
    }

    if area.height < 3 {
        // Ultra-short viewport: prioritize rendering the main content without panicking.
        draw_main(frame, area, view, now, now_epoch_ms);
        return;
    }

    let [header_area, main_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, header_area, view, now);
    draw_main(frame, main_area, view, now, now_epoch_ms);
    draw_footer(frame, footer_area, view);
}

fn draw_header(frame: &mut Frame, area: Rect, view: &RenderView<'_>, now: Instant) {
    if area.is_empty() {
        return;
    }

    let (online, total) = view.online_count;
    let count_color = if total == 0 {
        Color::DarkGray
    } else if online > 0 {
        Color::Green
    } else {
        Color::Red
    };

    let sep = Span::styled(" │ ", Style::default().fg(Color::DarkGray));
    let mut spans = vec![
        Span::styled(
            " ⛏ mcstat ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!("● {online}/{total} online"),
            Style::default()
                .fg(count_color)
                .add_modifier(Modifier::BOLD),
        ),
        sep.clone(),
        Span::styled(
            format!("refresh {}s", view.refresh_interval_secs),
            Style::default().fg(Color::Gray),
        ),
    ];

    if let Some(remaining) =
        seconds_until_refresh(view.refresh_interval_secs, view.last_refresh, now)
    {
        spans.push(sep);
        spans.push(Span::styled(
            format!("next in {remaining}s"),
            Style::default().fg(Color::Cyan),
        ));
    }

    let bar = Paragraph::new(Line::from(spans))
        .style(Style::default().bg(Color::Rgb(24, 28, 38)).fg(Color::White));
    frame.render_widget(bar, area);
}

fn draw_main(
    frame: &mut Frame,
    area: Rect,
    view: &RenderView<'_>,
    now: Instant,
    now_epoch_ms: u128,
) {
    if area.is_empty() {
        return;
    }

    if is_side_by_side(area.width) {
        let [table_area, detail_area] =
            Layout::horizontal([Constraint::Percentage(64), Constraint::Percentage(36)])
                .areas(area);
        draw_server_table(frame, table_area, view, now_epoch_ms);
        draw_detail_panel(frame, detail_area, view, now, now_epoch_ms);
    } else if area.height >= 6 {
        let [table_area, detail_area] =
            Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(area);
        draw_server_table(frame, table_area, view, now_epoch_ms);
        draw_detail_panel(frame, detail_area, view, now, now_epoch_ms);
    } else {
        draw_server_table(frame, area, view, now_epoch_ms);
    }
}

fn draw_server_table(frame: &mut Frame, area: Rect, view: &RenderView<'_>, now_epoch_ms: u128) {
    if area.is_empty() {
        return;
    }

    let title = format!(" Servers ({}) ", view.servers.len());
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(75, 90, 120)))
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    if view.servers.is_empty() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.is_empty() {
            return;
        }
        let hint_y = inner.y.saturating_add(inner.height.saturating_sub(1) / 2);
        let hint_rect = Rect::new(inner.x, hint_y, inner.width, 1.min(inner.height));
        let empty_hint = Paragraph::new(Line::from(vec![
            Span::styled("No servers configured — ", Style::default().fg(Color::Gray)),
            Span::styled(
                "press a to add a server",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]))
        .alignment(Alignment::Center);
        frame.render_widget(empty_hint, hint_rect);
        return;
    }

    let columns = columns_for_width(area.width);
    let spinner = spinner_frame(now_epoch_ms);

    let header_cells = columns.iter().map(|col| {
        let label = match col {
            TableColumn::Status => "Status",
            TableColumn::Name => "Name",
            TableColumn::Address => "Address",
            TableColumn::Ping => "Ping",
            TableColumn::Players => "Players",
            TableColumn::Version => "Version",
            TableColumn::Motd => "MOTD",
        };
        Cell::from(label).style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
    });
    let header = Row::new(header_cells).height(1);

    let rows = view.servers.iter().map(|entry| {
        let cells = columns
            .iter()
            .map(|col| build_table_cell(*col, entry, spinner));
        Row::new(cells).height(1)
    });

    let widths: Vec<Constraint> = columns.iter().map(|col| column_constraint(*col)).collect();

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(1)
        .highlight_symbol("▸ ")
        .highlight_spacing(HighlightSpacing::Always)
        .row_highlight_style(
            Style::default()
                .bg(Color::Rgb(36, 44, 64))
                .add_modifier(Modifier::BOLD),
        );

    let selected = view.selected.min(view.servers.len().saturating_sub(1));
    let mut state = TableState::default().with_selected(Some(selected));
    frame.render_stateful_widget(table, area, &mut state);
}

const fn column_constraint(col: TableColumn) -> Constraint {
    match col {
        TableColumn::Status => Constraint::Length(9),
        TableColumn::Name => Constraint::Fill(2),
        TableColumn::Address => Constraint::Fill(2),
        TableColumn::Ping => Constraint::Length(7),
        TableColumn::Players => Constraint::Length(11),
        TableColumn::Version => Constraint::Length(12),
        TableColumn::Motd => Constraint::Fill(3),
    }
}

fn status_badge_spans(entry: &ServerEntry, spinner: &'static str) -> Vec<Span<'static>> {
    if entry.refreshing {
        let spin_span = Span::styled(
            format!("{spinner} "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
        let state_span = match &entry.state {
            ServerState::Online(_) => Span::styled("Online", Style::default().fg(Color::Green)),
            ServerState::Offline { .. } => Span::styled("Offline", Style::default().fg(Color::Red)),
            ServerState::Unknown => Span::styled("Polling", Style::default().fg(Color::Cyan)),
        };
        vec![spin_span, state_span]
    } else {
        match &entry.state {
            ServerState::Online(_) => {
                vec![Span::styled("● Online", Style::default().fg(Color::Green))]
            }
            ServerState::Offline { .. } => {
                vec![Span::styled("● Offline", Style::default().fg(Color::Red))]
            }
            ServerState::Unknown => vec![Span::styled(
                "○ Unknown",
                Style::default().fg(Color::DarkGray),
            )],
        }
    }
}

fn build_table_cell(col: TableColumn, entry: &ServerEntry, spinner: &'static str) -> Cell<'static> {
    let muted = Style::default().fg(Color::DarkGray);
    match col {
        TableColumn::Status => Cell::from(Line::from(status_badge_spans(entry, spinner))),
        TableColumn::Name => Cell::from(Span::styled(
            entry.config.name.clone(),
            Style::default().fg(Color::White),
        )),
        TableColumn::Address => Cell::from(Span::styled(
            format_address(&entry.config),
            Style::default().fg(Color::Gray),
        )),
        TableColumn::Ping => match &entry.state {
            ServerState::Online(status) => {
                let ms = status.latency.as_millis();
                Cell::from(Span::styled(
                    format!("{ms} ms"),
                    Style::default().fg(ping_color(ms)),
                ))
            }
            ServerState::Offline { .. } | ServerState::Unknown => {
                Cell::from(Span::styled("—", muted))
            }
        },
        TableColumn::Players => match &entry.state {
            ServerState::Online(status) => {
                let style = if status.players_online > 0 {
                    Style::default().fg(Color::White)
                } else {
                    Style::default().fg(Color::Gray)
                };
                Cell::from(Span::styled(
                    format!("{}/{}", status.players_online, status.players_max),
                    style,
                ))
            }
            ServerState::Offline { .. } | ServerState::Unknown => {
                Cell::from(Span::styled("—", muted))
            }
        },
        TableColumn::Version => match &entry.state {
            ServerState::Online(status) => Cell::from(Span::styled(
                strip_legacy_codes(&status.version_name),
                Style::default().fg(Color::Gray),
            )),
            ServerState::Offline { .. } | ServerState::Unknown => {
                Cell::from(Span::styled("—", muted))
            }
        },
        TableColumn::Motd => match &entry.state {
            ServerState::Online(status) => {
                let plain = motd_plain(&status.motd);
                if plain.is_empty() {
                    Cell::from(Span::styled("—", muted))
                } else {
                    Cell::from(Span::styled(plain, Style::default().fg(Color::Gray)))
                }
            }
            ServerState::Offline { error } => Cell::from(Span::styled(
                strip_legacy_codes(error),
                Style::default().fg(Color::Red),
            )),
            ServerState::Unknown => Cell::from(Span::styled("—", muted)),
        },
    }
}

fn draw_detail_panel(
    frame: &mut Frame,
    area: Rect,
    view: &RenderView<'_>,
    now: Instant,
    now_epoch_ms: u128,
) {
    if area.is_empty() {
        return;
    }

    let title = match view.selected_entry {
        Some(entry) => format!(" Details — {} ", entry.config.name),
        None => " Details ".to_owned(),
    };

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(75, 90, 120)))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    let Some(entry) = view.selected_entry else {
        let empty = Paragraph::new(Span::styled(
            "Select or add a server to inspect its details.",
            Style::default().fg(Color::DarkGray),
        ))
        .block(block)
        .alignment(Alignment::Center);
        frame.render_widget(empty, area);
        return;
    };

    let spinner = spinner_frame(now_epoch_ms);
    let label_style = Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(Color::DarkGray);

    let updated_text = match entry.last_updated {
        Some(ts) => {
            let secs = now.saturating_duration_since(ts).as_secs();
            format!("updated {secs}s ago")
        }
        None => "not updated yet".to_owned(),
    };

    let mut status_line = vec![
        Span::styled("Server:   ", label_style),
        Span::styled(
            entry.config.name.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
    ];
    status_line.extend(status_badge_spans(entry, spinner));
    status_line.push(Span::raw("  "));
    status_line.push(Span::styled(format!("({updated_text})"), muted));

    let mut addr_spans = vec![
        Span::styled("Address:  ", label_style),
        Span::styled(
            format_address(&entry.config),
            Style::default().fg(Color::White),
        ),
    ];
    if entry.config.port.is_none() {
        addr_spans.push(Span::styled(" (SRV / 25565)", muted));
    }

    let mut lines: Vec<Line<'static>> = vec![Line::from(status_line), Line::from(addr_spans)];

    match &entry.state {
        ServerState::Online(status) => {
            let mut endpoint_spans = vec![
                Span::styled("Endpoint: ", label_style),
                Span::styled(
                    status.resolved_endpoint.clone(),
                    Style::default().fg(Color::Cyan),
                ),
            ];
            if status.has_favicon {
                endpoint_spans.push(Span::styled(" · favicon ✓", muted));
            }
            lines.push(Line::from(endpoint_spans));

            lines.push(Line::from(vec![
                Span::styled("Version:  ", label_style),
                Span::styled(
                    strip_legacy_codes(&status.version_name),
                    Style::default().fg(Color::White),
                ),
                Span::styled(format!(" (protocol {})", status.protocol_version), muted),
            ]));

            let ms = status.latency.as_millis();
            lines.push(Line::from(vec![
                Span::styled("Ping:     ", label_style),
                Span::styled(format!("{ms} ms"), Style::default().fg(ping_color(ms))),
            ]));

            lines.push(Line::from(vec![
                Span::styled("Players:  ", label_style),
                Span::styled(
                    format!("{}/{}", status.players_online, status.players_max),
                    Style::default().fg(Color::White),
                ),
            ]));

            if !status.player_sample.is_empty() {
                let sample = status
                    .player_sample
                    .iter()
                    .map(|name| strip_legacy_codes(name))
                    .filter(|name| !name.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ");
                if !sample.is_empty() {
                    lines.push(Line::from(vec![
                        Span::styled("Sample:   ", label_style),
                        Span::styled(sample, Style::default().fg(Color::Cyan)),
                    ]));
                }
            }

            if area.height > 9 {
                lines.push(Line::default());
            }
            lines.push(Line::from(Span::styled("MOTD:", label_style)));
            let motd_lines = motd_to_lines(&status.motd);
            if motd_lines.is_empty() {
                lines.push(Line::from(Span::styled(
                    "(empty MOTD)",
                    muted.add_modifier(Modifier::ITALIC),
                )));
            } else {
                lines.extend(motd_lines);
            }
        }
        ServerState::Offline { error } => {
            if area.height > 6 {
                lines.push(Line::default());
            }
            lines.push(Line::from(Span::styled(
                "Error:",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                strip_legacy_codes(error),
                Style::default().fg(Color::Red),
            )));
        }
        ServerState::Unknown => {
            if area.height > 6 {
                lines.push(Line::default());
            }
            let text = if entry.refreshing {
                "Querying server…"
            } else {
                "Waiting for first poll — press r to refresh."
            };
            lines.push(Line::from(Span::styled(text, muted)));
        }
    }

    let detail = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(detail, area);
}

fn key_hint_spans() -> Vec<Span<'static>> {
    let key = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let desc = Style::default().fg(Color::Gray);
    let dot = Style::default().fg(Color::DarkGray);

    vec![
        Span::styled("↑↓/jk", key),
        Span::styled(" move", desc),
        Span::styled(" · ", dot),
        Span::styled("a", key),
        Span::styled(" add", desc),
        Span::styled(" · ", dot),
        Span::styled("d", key),
        Span::styled(" delete", desc),
        Span::styled(" · ", dot),
        Span::styled("r", key),
        Span::styled(" refresh", desc),
        Span::styled(" · ", dot),
        Span::styled("?", key),
        Span::styled(" help", desc),
        Span::styled(" · ", dot),
        Span::styled("q", key),
        Span::styled(" quit", desc),
    ]
}

fn draw_footer(frame: &mut Frame, area: Rect, view: &RenderView<'_>) {
    if area.is_empty() {
        return;
    }

    let bar_bg = Style::default().bg(Color::Rgb(20, 24, 32));
    frame.render_widget(Block::new().style(bar_bg), area);

    let hints_line = Line::from(key_hint_spans());
    let hints_width = hints_line.width() as u16;

    if let Some(msg) = view.message {
        let msg_color = match msg.kind {
            MessageKind::Info => Color::Cyan,
            MessageKind::Error => Color::Red,
        };
        let msg_line = Line::from(vec![
            Span::raw(" "),
            Span::styled(
                msg.text.clone(),
                Style::default().fg(msg_color).add_modifier(Modifier::BOLD),
            ),
        ]);

        if area.width > hints_width.saturating_add(16) {
            let [msg_area, hints_area] = Layout::horizontal([
                Constraint::Min(12),
                Constraint::Length(hints_width.saturating_add(1)),
            ])
            .areas(area);
            frame.render_widget(Paragraph::new(msg_line).style(bar_bg), msg_area);
            frame.render_widget(
                Paragraph::new(hints_line)
                    .style(bar_bg)
                    .alignment(Alignment::Right),
                hints_area,
            );
        } else {
            frame.render_widget(Paragraph::new(msg_line).style(bar_bg), area);
        }
    } else {
        let mut padded = vec![Span::raw(" ")];
        padded.extend(hints_line.spans);
        frame.render_widget(Paragraph::new(Line::from(padded)).style(bar_bg), area);
    }
}
