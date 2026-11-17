//! Interactive terminal dashboard.
//!
//! `--tui` opens a live view of a running batch: results stream in while the
//! user browses, filters and sorts them. The normal report mode is untouched.

mod model;
pub mod ui;

use std::io::{IsTerminal, Stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use crate::checker::{self, CheckSettings};
use crate::cli::Sort;
use crate::proxy::Proxy;

pub use model::{Dashboard, Filter};
pub use ui::View;

/// Settings the dashboard needs beyond the check options.
#[derive(Debug, Clone)]
pub struct TuiConfig {
    pub judge: String,
    pub workers: usize,
    pub show_credentials: bool,
    pub sort: Sort,
}

/// Runs the dashboard until the user quits.
///
/// The checks keep running in the background; leaving the dashboard cancels
/// the remaining work and restores the terminal.
pub async fn run(
    proxies: Vec<Proxy>,
    settings: Arc<CheckSettings>,
    config: TuiConfig,
) -> Result<()> {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        bail!("--tui needs an interactive terminal; use --quiet or --format for piping");
    }

    let total = proxies.len();
    let view = View {
        judge: &config.judge,
        workers: config.workers,
        show_credentials: config.show_credentials,
    };
    let mut app = Dashboard::new(total, config.sort);
    let mut session = TerminalSession::start()?;

    let (result_sender, mut results) = mpsc::unbounded_channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let _workers = checker::spawn_workers(
        proxies,
        settings,
        config.workers,
        Arc::clone(&cancel),
        result_sender,
    );

    let (event_sender, mut events) = mpsc::unbounded_channel();
    spawn_event_reader(event_sender, Arc::clone(&cancel));

    let mut ticker = tokio::time::interval(Duration::from_millis(120));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut results_open = true;
    let mut events_open = true;
    let mut quit = false;

    while !quit {
        tokio::select! {
            _ = ticker.tick() => {
                session.draw(&app, &view)?;
            }
            received = results.recv(), if results_open => {
                match received {
                    Some(result) => app.push(result),
                    None => results_open = false,
                }
            }
            event = events.recv(), if events_open => {
                match event {
                    Some(event) => {
                        quit = handle_key(event, &mut app);
                        session.draw(&app, &view)?;
                    }
                    None => events_open = false,
                }
            }
        }
    }

    cancel.store(true, Ordering::Relaxed);
    Ok(())
}

/// Owns the terminal state and restores it on drop, including on panic.
struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn start() -> Result<Self> {
        enable_raw_mode().context("cannot enable raw mode")?;

        let mut stdout = std::io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(error).context("cannot enter the alternate screen");
        }

        let terminal =
            Terminal::new(CrosstermBackend::new(stdout)).context("cannot create the terminal")?;

        install_panic_hook();
        Ok(Self { terminal })
    }

    fn draw(&mut self, app: &Dashboard, view: &View) -> Result<()> {
        self.terminal
            .draw(|frame| ui::draw(frame, app, view))
            .context("cannot draw the dashboard")?;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen, Show);
        let _ = self.terminal.show_cursor();
    }
}

/// Restores the terminal before a panic message is printed.
fn install_panic_hook() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = disable_raw_mode();
            let _ = execute!(std::io::stdout(), LeaveAlternateScreen, Show);
            previous(info);
        }));
    });
}

/// Reads terminal events on a blocking thread until the dashboard closes.
fn spawn_event_reader(sender: mpsc::UnboundedSender<Event>, cancel: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while !cancel.load(Ordering::Relaxed) {
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(event) => {
                        if sender.send(event).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    });
}

/// Applies one terminal event. Returns true when the dashboard should close.
fn handle_key(event: Event, app: &mut Dashboard) -> bool {
    let Event::Key(key) = event else {
        return false;
    };
    if key.kind != KeyEventKind::Press {
        return false;
    }

    match key.code {
        KeyCode::Char('q') => true,
        KeyCode::Esc => {
            if app.help {
                app.toggle_help();
                false
            } else {
                true
            }
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => true,
        KeyCode::Char('?') => {
            app.toggle_help();
            false
        }
        KeyCode::Char('f') => {
            app.cycle_filter();
            false
        }
        KeyCode::Char('s') => {
            app.cycle_sort();
            false
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.select_prev();
            false
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.select_next();
            false
        }
        KeyCode::PageUp => {
            app.select_page(-10);
            false
        }
        KeyCode::PageDown => {
            app.select_page(10);
            false
        }
        KeyCode::Home | KeyCode::Char('g') => {
            app.select_first();
            false
        }
        KeyCode::End | KeyCode::Char('G') => {
            app.select_last();
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn press(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn q_quits() {
        let mut app = Dashboard::new(0, Sort::Latency);
        assert!(handle_key(press(KeyCode::Char('q')), &mut app));
    }

    #[test]
    fn escape_closes_help_before_quitting() {
        let mut app = Dashboard::new(0, Sort::Latency);
        app.toggle_help();
        assert!(!handle_key(press(KeyCode::Esc), &mut app));
        assert!(!app.help);
        assert!(handle_key(press(KeyCode::Esc), &mut app));
    }

    #[test]
    fn f_and_s_change_the_view() {
        let mut app = Dashboard::new(0, Sort::Latency);
        assert!(!handle_key(press(KeyCode::Char('f')), &mut app));
        assert_eq!(app.filter(), Filter::Alive);
        assert!(!handle_key(press(KeyCode::Char('s')), &mut app));
        assert_eq!(app.sort(), Sort::Proxy);
    }

    #[test]
    fn navigation_keys_move_the_cursor() {
        use crate::proxy::{Protocol, Proxy};
        use crate::report::CheckResult;

        let mut app = Dashboard::new(2, Sort::Latency);
        app.push(CheckResult::alive(
            Proxy::new(Protocol::Http, "10.0.0.1", 1),
            Duration::from_millis(10),
            Some(200),
        ));
        app.push(CheckResult::alive(
            Proxy::new(Protocol::Http, "10.0.0.1", 2),
            Duration::from_millis(20),
            Some(200),
        ));

        assert!(!handle_key(press(KeyCode::Down), &mut app));
        assert_eq!(app.selected_index(), 1);
        assert!(!handle_key(press(KeyCode::Char('k')), &mut app));
        assert_eq!(app.selected_index(), 0);
    }

    #[test]
    fn non_key_and_release_events_are_ignored() {
        let mut app = Dashboard::new(0, Sort::Latency);
        assert!(!handle_key(Event::Resize(80, 24), &mut app));

        let release = Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));
        assert!(!handle_key(release, &mut app));
    }
}
