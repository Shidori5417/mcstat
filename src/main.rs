//! mcstat — live Minecraft Java server status monitor in the terminal.
//!
//! OWNERSHIP: Orchestrator (Opus). `feat/cli-tests` may wire CLI options in.

mod config;
mod model;
mod poller;
mod protocol;
mod state;
mod ui;

use std::io::{self, Stdout};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use crate::config::AppConfig;
use crate::model::{Action, AppEvent};
use crate::poller::{PollerSettings, spawn_poller};
use crate::state::App;

type Tui = Terminal<CrosstermBackend<Stdout>>;

/// UI tick period (spinner animation, message expiry).
const TICK: Duration = Duration::from_millis(250);
/// Retries per server poll before reporting Offline.
const POLL_RETRIES: u32 = 1;

#[tokio::main]
async fn main() -> Result<()> {
    let config_path = AppConfig::default_path().context("locating config file")?;
    let config = AppConfig::load_or_create(&config_path)
        .with_context(|| format!("loading config {}", config_path.display()))?;

    let settings = PollerSettings {
        interval: Duration::from_secs(config.refresh_interval_secs.max(1)),
        timeout: Duration::from_secs(config.timeout_secs.max(1)),
        retries: POLL_RETRIES,
    };
    let app = App::new(config, config_path);

    install_panic_hook();
    let mut terminal = setup_terminal().context("initializing terminal")?;
    let result = run(&mut terminal, app, settings).await;
    restore_terminal().context("restoring terminal")?;
    result
}

async fn run(terminal: &mut Tui, mut app: App, settings: PollerSettings) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    let stop = Arc::new(AtomicBool::new(false));
    let input_thread = spawn_input_thread(tx.clone(), Arc::clone(&stop));
    let ticker = spawn_ticker(tx.clone());

    let poller = spawn_poller(settings, app.poll_targets(), tx);
    app.last_refresh = Some(std::time::Instant::now());

    terminal.draw(|f| ui::draw(f, &app))?;
    while let Some(event) = rx.recv().await {
        let action = match event {
            AppEvent::Input(Event::Key(key)) => ui::handle_key(&mut app, key),
            AppEvent::Input(_) => Action::None, // resize etc.: just redraw
            AppEvent::Tick => {
                app.expire_message();
                advance_refresh_clock(&mut app, settings.interval);
                Action::None
            }
            AppEvent::PollStarted(id) => {
                app.mark_refreshing(id);
                Action::None
            }
            AppEvent::PollFinished(result) => {
                app.apply_poll_result(result);
                Action::None
            }
        };

        match action {
            Action::None => {}
            Action::Quit => break,
            Action::RefreshAll => {
                poller.refresh_all();
                app.last_refresh = Some(std::time::Instant::now());
            }
            Action::ServersChanged => poller.set_servers(app.poll_targets()),
        }
        terminal.draw(|f| ui::draw(f, &app))?;
    }

    stop.store(true, Ordering::Relaxed);
    ticker.abort();
    poller.shutdown().await;
    // The input thread polls `stop` every 100ms; joining is bounded.
    let _ = input_thread.join();
    Ok(())
}

/// Keep `app.last_refresh` in step with the poller's own interval timer so
/// the header countdown keeps cycling between automatic refreshes.
fn advance_refresh_clock(app: &mut App, interval: Duration) {
    if let Some(last) = app.last_refresh.as_mut() {
        while last.elapsed() >= interval {
            *last += interval;
        }
    }
}

/// Blocking crossterm reader on a dedicated OS thread.
fn spawn_input_thread(
    tx: mpsc::UnboundedSender<AppEvent>,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(ev) => {
                        if tx.send(AppEvent::Input(ev)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    })
}

fn spawn_ticker(tx: mpsc::UnboundedSender<AppEvent>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        loop {
            interval.tick().await;
            if tx.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    })
}

fn setup_terminal() -> io::Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(e) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(e);
    }
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    terminal.hide_cursor()?;
    terminal.clear()?;
    Ok(terminal)
}

fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        io::stdout(),
        LeaveAlternateScreen,
        crossterm::cursor::Show
    )?;
    Ok(())
}

/// Restore the terminal before the default panic message is printed, so a
/// panic never leaves the shell in raw mode / alternate screen.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        previous(info);
    }));
}
