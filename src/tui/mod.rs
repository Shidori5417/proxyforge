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
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::checker::{self, CheckSettings};
use crate::cli::Sort;
use crate::proxy::Proxy;
use crate::report::CheckResult;

pub use model::{Dashboard, Filter};
pub use ui::View;

/// Settings the dashboard needs beyond the check options.
#[derive(Debug, Clone)]
pub struct TuiConfig {
    pub judge: String,
    pub workers: usize,
    pub show_credentials: bool,
    pub sort: Sort,
    /// File the `e` key writes to; mirrors `--save` when given.
    pub export_path: String,
}

/// What the event handler wants the run loop to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Quit,
    Export,
    Recheck,
    Browse,
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
        export_path: config.export_path.clone(),
    };
    let mut app = Dashboard::new(total, config.sort);
    let mut session = TerminalSession::start()?;

    let (result_sender, mut results) = unbounded_channel();
    let recheck_sender = result_sender.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let _workers = checker::spawn_workers(
        proxies,
        Arc::clone(&settings),
        config.workers,
        Arc::clone(&cancel),
        result_sender,
    );

    let (event_sender, mut events) = unbounded_channel();
    spawn_event_reader(event_sender, Arc::clone(&cancel));

    let mut ticker = tokio::time::interval(Duration::from_millis(120));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut results_open = true;
    let mut events_open = true;
    let mut quit = false;

    while !quit {
        tokio::select! {
            _ = ticker.tick() => {
                session.draw(&mut app, &view)?;
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
                        let action = handle_event(event, &mut app);
                        match action {
                            Action::Quit => quit = true,
                            Action::Export => export(&mut app, &view),
                            Action::Recheck => recheck(&mut app, &settings, &recheck_sender),
                            Action::Browse => {}
                        }
                        session.draw(&mut app, &view)?;
                    }
                    None => events_open = false,
                }
            }
        }
    }

    cancel.store(true, Ordering::Relaxed);
    Ok(())
}

/// Writes the visible alive endpoints to the configured export file.
fn export(app: &mut Dashboard, view: &View) {
    let path = &view.export_path;
    let text = app.export_endpoints(view.show_credentials);
    if text.is_empty() {
        app.notice("nothing to export: no alive endpoint is visible");
        return;
    }

    let count = text.lines().count();
    match std::fs::write(path, text) {
        Ok(()) => app.notice(format!("exported {count} endpoints to {path}")),
        Err(error) => app.notice(format!("cannot write {path}: {error}")),
    }
}

/// Re-checks the selected endpoint on a detached task; the result arrives
/// through the regular channel and replaces the previous one.
fn recheck(
    app: &mut Dashboard,
    settings: &Arc<CheckSettings>,
    sender: &UnboundedSender<CheckResult>,
) {
    let selected = app.selected().cloned();
    let Some(selected) = selected else {
        return;
    };
    app.notice(format!("re-checking {}", selected.proxy.redacted()));

    let settings = Arc::clone(settings);
    let sender = sender.clone();
    tokio::spawn(async move {
        let result = checker::check_proxy(&selected.proxy, &settings).await;
        let _ = sender.send(result);
    });
}

/// Owns the terminal state and restores it on drop, including on panic.
struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn start() -> Result<Self> {
        enable_raw_mode().context("cannot enable raw mode")?;

        let mut stdout = std::io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide, EnableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(error).context("cannot enter the alternate screen");
        }

        let terminal =
            Terminal::new(CrosstermBackend::new(stdout)).context("cannot create the terminal")?;

        install_panic_hook();
        Ok(Self { terminal })
    }

    fn draw(&mut self, app: &mut Dashboard, view: &View) -> Result<()> {
        self.terminal
            .draw(|frame| ui::draw(frame, app, view))
            .context("cannot draw the dashboard")?;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            Show,
            DisableMouseCapture
        );
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
            let _ = execute!(
                std::io::stdout(),
                LeaveAlternateScreen,
                Show,
                DisableMouseCapture
            );
            previous(info);
        }));
    });
}

/// Reads terminal events on a blocking thread until the dashboard closes.
fn spawn_event_reader(sender: UnboundedSender<Event>, cancel: Arc<AtomicBool>) {
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

/// Applies one terminal event to the dashboard and says what to do next.
fn handle_event(event: Event, app: &mut Dashboard) -> Action {
    app.clear_notice();

    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => handle_key(key, app),
        Event::Mouse(mouse) => {
            handle_mouse(mouse, app);
            Action::Browse
        }
        _ => Action::Browse,
    }
}

fn handle_key(key: KeyEvent, app: &mut Dashboard) -> Action {
    match key.code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Esc => {
            if app.help {
                app.toggle_help();
                Action::Browse
            } else {
                Action::Quit
            }
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
        KeyCode::Char('?') => {
            app.toggle_help();
            Action::Browse
        }
        KeyCode::Char('e') => Action::Export,
        KeyCode::Char('r') => Action::Recheck,
        KeyCode::Char('f') => {
            app.cycle_filter();
            Action::Browse
        }
        KeyCode::Char('s') => {
            app.cycle_sort();
            Action::Browse
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.select_prev();
            Action::Browse
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.select_next();
            Action::Browse
        }
        KeyCode::PageUp => {
            app.select_page(-10);
            Action::Browse
        }
        KeyCode::PageDown => {
            app.select_page(10);
            Action::Browse
        }
        KeyCode::Home | KeyCode::Char('g') => {
            app.select_first();
            Action::Browse
        }
        KeyCode::End | KeyCode::Char('G') => {
            app.select_last();
            Action::Browse
        }
        _ => Action::Browse,
    }
}

fn handle_mouse(mouse: MouseEvent, app: &mut Dashboard) {
    match mouse.kind {
        MouseEventKind::ScrollDown => app.select_next(),
        MouseEventKind::ScrollUp => app.select_prev(),
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(index) = app.row_at(mouse.row) {
                app.select_visible_index(index);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::{Protocol, Proxy};
    use model::Geometry;

    fn press(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn dashboard_with(rows: usize) -> (Dashboard, ()) {
        let mut app = Dashboard::new(rows, Sort::Latency);
        for port in 1..=rows as u16 {
            app.push(CheckResult::alive(
                Proxy::new(Protocol::Http, "10.0.0.1", port),
                Duration::from_millis(10),
                Some(200),
            ));
        }
        (app, ())
    }

    #[test]
    fn q_quits() {
        let (mut app, _) = dashboard_with(0);
        assert_eq!(
            handle_event(press(KeyCode::Char('q')), &mut app),
            Action::Quit
        );
    }

    #[test]
    fn escape_closes_help_before_quitting() {
        let (mut app, _) = dashboard_with(0);
        app.toggle_help();
        assert_eq!(handle_event(press(KeyCode::Esc), &mut app), Action::Browse);
        assert!(!app.help);
        assert_eq!(handle_event(press(KeyCode::Esc), &mut app), Action::Quit);
    }

    #[test]
    fn f_and_s_change_the_view() {
        let (mut app, _) = dashboard_with(0);
        assert_eq!(
            handle_event(press(KeyCode::Char('f')), &mut app),
            Action::Browse
        );
        assert_eq!(app.filter(), Filter::Alive);
        assert_eq!(
            handle_event(press(KeyCode::Char('s')), &mut app),
            Action::Browse
        );
        assert_eq!(app.sort(), Sort::Proxy);
    }

    #[test]
    fn e_and_r_request_export_and_recheck() {
        let (mut app, _) = dashboard_with(0);
        assert_eq!(
            handle_event(press(KeyCode::Char('e')), &mut app),
            Action::Export
        );
        assert_eq!(
            handle_event(press(KeyCode::Char('r')), &mut app),
            Action::Recheck
        );
    }

    #[test]
    fn navigation_keys_move_the_cursor() {
        let (mut app, _) = dashboard_with(2);
        assert_eq!(handle_event(press(KeyCode::Down), &mut app), Action::Browse);
        assert_eq!(app.selected_index(), 1);
        assert_eq!(
            handle_event(press(KeyCode::Char('k')), &mut app),
            Action::Browse
        );
        assert_eq!(app.selected_index(), 0);
    }

    #[test]
    fn mouse_wheel_scrolls_and_click_selects() {
        let (mut app, _) = dashboard_with(2);
        app.set_table_geometry(
            0,
            Geometry {
                x: 0,
                y: 5,
                width: 100,
                height: 20,
            },
        );

        let scroll_down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(handle_event(scroll_down, &mut app), Action::Browse);
        assert_eq!(app.selected_index(), 1);

        let click_first = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: 7,
            modifiers: KeyModifiers::NONE,
        });
        handle_event(click_first, &mut app);
        assert_eq!(app.selected_index(), 0);
    }

    #[test]
    fn non_key_and_release_events_are_ignored() {
        let (mut app, _) = dashboard_with(0);
        assert_eq!(
            handle_event(Event::Resize(80, 24), &mut app),
            Action::Browse
        );

        let release = Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));
        assert_eq!(handle_event(release, &mut app), Action::Browse);
    }

    #[test]
    fn any_event_clears_the_notice() {
        let (mut app, _) = dashboard_with(0);
        app.notice("exported");
        handle_event(press(KeyCode::Char('j')), &mut app);
        assert_eq!(app.current_notice(), None);
    }
}
