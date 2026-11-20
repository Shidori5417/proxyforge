//! Rendering of the interactive dashboard.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Cell, Clear, Gauge, Paragraph, Row, Table, TableState, Wrap};

use crate::anonymity::AnonymityLevel;
use crate::report::CheckResult;

use super::model::{Dashboard, Geometry};

/// Static information shown around the live results.
pub struct View<'a> {
    pub judge: &'a str,
    pub workers: usize,
    pub show_credentials: bool,
    pub export_path: String,
}

const HINTS: &str = " q quit · ↑↓/jk move · f filter · s sort · e export · r recheck · ? help ";
const HELP: &[(&str, &str)] = &[
    ("q / Esc", "quit the dashboard"),
    ("? ", "toggle this help"),
    (
        "e",
        "export the visible alive endpoints to proxyforge-export.txt",
    ),
    ("r", "re-check the selected proxy"),
    ("↑ / k", "select previous proxy"),
    ("↓ / j", "select next proxy"),
    ("PgUp/PgDn", "move one page"),
    ("g / G", "first / last proxy"),
    ("f", "cycle filter: all → alive → dead"),
    ("s", "cycle sort: latency → proxy → status"),
    ("e", "export alive rows to proxyforge-export.txt"),
    ("r", "re-check the selected proxy"),
    ("wheel", "scroll the results"),
    ("click", "select a row"),
];

pub fn draw(frame: &mut Frame, app: &mut Dashboard, view: &View) {
    let area = frame.area();
    let [header, body, details, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(4),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, app, view, header);
    draw_results(frame, app, view, body);
    draw_details(frame, app, view, details);
    draw_footer(frame, app, footer);

    if app.help {
        draw_help(frame, area);
    }
}

fn draw_header(frame: &mut Frame, app: &Dashboard, view: &View, area: Rect) {
    let title = Line::from(vec![
        Span::styled(
            " proxyforge ",
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("· {} workers · judge {} ", view.workers, view.judge),
            Style::new().fg(Color::DarkGray),
        ),
    ]);

    let timing = if app.is_finished() {
        format!("done in {}", human_duration(app.elapsed()))
    } else {
        human_duration(app.elapsed())
    };
    let stats = format!(
        "{}/{} · {} alive · {} dead · {timing} ",
        app.checked(),
        app.total(),
        app.alive(),
        app.dead(),
    );

    let block = Block::bordered()
        .title(title)
        .title_top(Line::from(Span::styled(stats, Style::new().fg(Color::Gray))).right_aligned());

    let gauge = Gauge::default()
        .block(block)
        .gauge_style(Style::new().fg(Color::Cyan).bg(Color::Black))
        .ratio(app.progress().clamp(0.0, 1.0))
        .label(format!("{:.0}%", app.progress() * 100.0));

    frame.render_widget(gauge, area);
}

fn draw_results(frame: &mut Frame, app: &mut Dashboard, view: &View, area: Rect) {
    let rows = app.visible();
    let title = format!(
        " results · filter: {} · sort: {} · {} shown ",
        app.filter().label(),
        app.sort().as_str(),
        rows.len()
    );
    let block = Block::bordered().title(title);

    if rows.is_empty() {
        let message = if app.checked() == 0 {
            "waiting for results…"
        } else {
            "no proxy matches the current filter"
        };
        let paragraph = Paragraph::new(message)
            .block(block)
            .alignment(Alignment::Center)
            .style(Style::new().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }

    let header = Row::new([
        "Proxy",
        "Proto",
        "Status",
        "Latency",
        "Anonymity",
        "Exit IP",
        "Geo",
        "Detail",
    ])
    .style(Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD));

    let table_rows: Vec<Row> = rows
        .iter()
        .map(|result| row(result, view.show_credentials))
        .collect();

    let widths = [
        Constraint::Length(29),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(9),
        Constraint::Length(12),
        Constraint::Length(15),
        Constraint::Length(14),
        Constraint::Min(10),
    ];

    let table = Table::new(table_rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(
            Style::new()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");

    let mut state = TableState::new().with_selected(Some(app.selected_index()));
    frame.render_stateful_widget(table, area, &mut state);

    // Remember where the table is and how far it scrolled, so mouse clicks
    // can be mapped back to results.
    app.set_table_geometry(
        *state.offset_mut(),
        Geometry {
            x: area.x,
            y: area.y,
            width: area.width,
            height: area.height,
        },
    );
}

fn row(result: &CheckResult, show_credentials: bool) -> Row<'static> {
    let endpoint = if show_credentials {
        result.proxy.to_url()
    } else {
        result.proxy.redacted()
    };

    let status = if result.alive {
        Cell::from("alive").style(Style::new().fg(Color::Green))
    } else {
        Cell::from("dead").style(Style::new().fg(Color::Red))
    };

    let anonymity = match result.anonymity {
        Some(AnonymityLevel::Elite) => Cell::from("elite").style(Style::new().fg(Color::Green)),
        Some(AnonymityLevel::Anonymous) => {
            Cell::from("anonymous").style(Style::new().fg(Color::Yellow))
        }
        Some(AnonymityLevel::Transparent) => {
            Cell::from("transparent").style(Style::new().fg(Color::Gray))
        }
        Some(AnonymityLevel::Unknown) => {
            Cell::from("unknown").style(Style::new().fg(Color::DarkGray))
        }
        None => Cell::from("-").style(Style::new().fg(Color::DarkGray)),
    };

    let detail = match &result.error {
        Some(error) => shorten(error, 36),
        None => format!("HTTP {}", result.status_code.unwrap_or_default()),
    };

    Row::new(vec![
        Cell::from(endpoint),
        Cell::from(result.proxy.protocol.as_str()),
        status,
        Cell::from(format!("{:>6} ms", result.latency_ms())),
        anonymity,
        Cell::from(result.exit_ip.clone().unwrap_or_else(|| "-".into())),
        Cell::from(
            result
                .geo
                .as_ref()
                .map(|geo| geo.label())
                .unwrap_or_else(|| "-".into()),
        ),
        Cell::from(detail),
    ])
}

fn draw_details(frame: &mut Frame, app: &Dashboard, view: &View, area: Rect) {
    let block = Block::bordered().title(" details ");

    let lines: Vec<Line> = match app.selected() {
        Some(result) => {
            let endpoint = if view.show_credentials {
                result.proxy.to_url()
            } else {
                result.proxy.redacted()
            };
            let state = if result.alive { "alive" } else { "dead" };
            let anonymity = result
                .anonymity
                .map(|level| level.as_str())
                .unwrap_or("unknown");
            let exit = result.exit_ip.as_deref().unwrap_or("-");

            let summary = Line::from(vec![
                Span::styled(endpoint, Style::new().add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!(
                        " · {state} · {} ms · {anonymity} · exit {exit}",
                        result.latency_ms()
                    ),
                    Style::new().fg(Color::Gray),
                ),
            ]);

            let detail = match (&result.error, result.leaks.is_empty()) {
                (Some(error), _) => {
                    Line::from(Span::styled(error.clone(), Style::new().fg(Color::Red)))
                }
                (None, false) => Line::from(format!("leaks: {}", result.leaks.join(" | "))),
                (None, true) => Line::from(Span::styled(
                    "no forwarding headers visible",
                    Style::new().fg(Color::DarkGray),
                )),
            };

            vec![summary, detail]
        }
        None => vec![Line::from(Span::styled(
            "nothing selected",
            Style::new().fg(Color::DarkGray),
        ))],
    };

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_footer(frame: &mut Frame, app: &Dashboard, area: Rect) {
    match &app.notice {
        Some(notice) => frame.render_widget(
            Paragraph::new(format!(" {notice}")).style(Style::new().fg(Color::Yellow)),
            area,
        ),
        None => frame.render_widget(
            Paragraph::new(HINTS).style(Style::new().fg(Color::DarkGray)),
            area,
        ),
    }
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let popup = centered(area, 64, 74);
    frame.render_widget(Clear, popup);

    let mut lines = vec![
        Line::from(Span::styled(
            format!(" proxyforge {}", env!("CARGO_PKG_VERSION")),
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for (key, description) in HELP {
        lines.push(Line::from(vec![
            Span::styled(format!("{key:<11}"), Style::new().fg(Color::Yellow)),
            Span::raw(*description),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "checks keep running while you browse",
        Style::new().fg(Color::DarkGray),
    )));

    let paragraph = Paragraph::new(Text::from(lines))
        .block(Block::bordered().title(" help "))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup);
}

fn centered(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1])[1]
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut shortened: String = text.chars().take(max.saturating_sub(1)).collect();
    shortened.push('…');
    shortened
}

fn human_duration(duration: std::time::Duration) -> String {
    let millis = duration.as_millis();
    if millis < 1000 {
        format!("{millis} ms")
    } else {
        format!("{:.1} s", duration.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Sort;
    use crate::proxy::{Protocol, Proxy};
    use std::time::Duration;

    fn alive(port: u16, latency_ms: u64) -> CheckResult {
        CheckResult::alive(
            Proxy::new(Protocol::Http, "10.0.0.1", port),
            Duration::from_millis(latency_ms),
            Some(200),
        )
    }

    fn dead(port: u16) -> CheckResult {
        CheckResult::dead(
            Proxy::new(Protocol::Socks5, "10.0.0.2", port),
            Duration::from_millis(500),
            "cannot connect to proxy: refused",
        )
    }

    fn render(app: &mut Dashboard, width: u16, height: u16) -> String {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let view = View {
            judge: "httpbin.org/get",
            workers: 100,
            show_credentials: false,
            export_path: "proxyforge-export.txt".into(),
        };
        let borrowed = &mut *app;
        terminal.draw(|frame| draw(frame, borrowed, &view)).unwrap();

        let buffer = terminal.backend().buffer();
        let mut screen = String::new();
        for y in 0..height {
            for x in 0..width {
                let index = (y * width + x) as usize;
                screen.push_str(buffer.content[index].symbol());
            }
            screen.push('\n');
        }
        screen
    }

    #[test]
    fn renders_results_header_and_details() {
        let mut app = Dashboard::new(2, Sort::Latency);
        app.push(alive(8080, 120));
        app.push(dead(1080));

        let screen = render(&mut app, 110, 24);

        assert!(screen.contains("proxyforge"));
        assert!(screen.contains("2/2 ·"));
        assert!(screen.contains("results · filter: all · sort: latency"));
        assert!(screen.contains("http://10.0.0.1:8080"));
        assert!(screen.contains("alive"));
        assert!(screen.contains("dead"));
        assert!(screen.contains("elite") || screen.contains("unknown"));
        assert!(screen.contains("q quit"));

        // The details pane shows the selected row; move to the dead one.
        app.select_last();
        let screen = render(&mut app, 110, 24);
        assert!(screen.contains("cannot connect to proxy"));
    }

    #[test]
    fn empty_state_waits_for_results() {
        let mut app = Dashboard::new(3, Sort::Latency);
        let screen = render(&mut app, 100, 20);
        assert!(screen.contains("waiting for results"));
        assert!(screen.contains("0/3 ·"));
    }

    #[test]
    fn filtered_empty_state_is_explained() {
        let mut app = Dashboard::new(1, Sort::Latency);
        app.push(alive(1, 10));
        app.cycle_filter(); // alive only, row stays
        app.cycle_filter(); // dead only, nothing left
        let screen = render(&mut app, 100, 20);
        assert!(screen.contains("no proxy matches the current filter"));
    }

    #[test]
    fn help_overlay_lists_keys() {
        let mut app = Dashboard::new(0, Sort::Latency);
        app.toggle_help();
        let screen = render(&mut app, 100, 30);
        assert!(screen.contains("help"));
        assert!(screen.contains("quit the dashboard"));
        assert!(screen.contains("cycle filter"));
    }

    #[test]
    fn shorten_keeps_short_text_and_truncates_long_text() {
        assert_eq!(shorten("short", 10), "short");
        assert_eq!(shorten("abcdefghij", 5), "abcd…");
    }
}
