//! Report filtering, sorting and rendering.

use std::io::IsTerminal;
use std::time::Duration;

use anyhow::{Context, Result};
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Cell, CellAlignment, Color, ContentArrangement, Table};
use serde::Serialize;

use crate::anonymity::AnonymityLevel;
use crate::cli::{Cli, Format, Sort};
use crate::report::CheckResult;

/// A finished run: the visible results plus aggregate statistics.
#[derive(Debug)]
pub struct Report {
    /// Results after filtering and sorting.
    pub results: Vec<CheckResult>,
    pub stats: Stats,
}

impl Report {
    pub fn build(results: Vec<CheckResult>, cli: &Cli, elapsed: Duration) -> Self {
        let stats = Stats::new(&results, elapsed);

        let mut visible: Vec<CheckResult> = results
            .into_iter()
            .filter(|result| keep(result, cli))
            .collect();

        match cli.sort {
            Sort::Latency => visible.sort_by_key(|result| (!result.alive, result.latency_ms())),
            Sort::Proxy => visible.sort_by_key(|result| result.proxy.redacted()),
            Sort::Status => visible.sort_by_key(|result| !result.alive),
        }

        Report {
            results: visible,
            stats,
        }
    }

    /// Renders the report in the requested format.
    pub fn render(&self, cli: &Cli) -> Result<String> {
        if cli.format == Format::Table && cli.quiet {
            return Ok(self.render_plain(cli));
        }

        match cli.format {
            Format::Table => Ok(self.render_table(cli)),
            Format::Json => {
                let rows = self.rows(cli);
                let mut output =
                    serde_json::to_string_pretty(&rows).context("cannot encode JSON report")?;
                output.push('\n');
                Ok(output)
            }
            Format::Jsonl => {
                let mut output = String::new();
                for row in self.rows(cli) {
                    output.push_str(
                        &serde_json::to_string(&row).context("cannot encode JSON report")?,
                    );
                    output.push('\n');
                }
                Ok(output)
            }
            Format::Csv => self.render_csv(cli),
        }
    }

    fn rows(&self, cli: &Cli) -> Vec<Row> {
        self.results
            .iter()
            .map(|result| Row::new(result, cli.show_credentials))
            .collect()
    }

    fn render_plain(&self, cli: &Cli) -> String {
        let mut output = String::new();
        for result in &self.results {
            if !result.alive {
                continue;
            }
            if cli.show_credentials {
                output.push_str(&result.proxy.to_url());
            } else {
                output.push_str(&result.proxy.redacted());
            }
            output.push('\n');
        }
        output
    }

    fn render_table(&self, cli: &Cli) -> String {
        let mut table = Table::new();
        // Colours only make sense on an interactive terminal and never inside
        // a file or a pipe.
        let color = !cli.no_color
            && cli.output.is_none()
            && std::env::var_os("NO_COLOR").is_none()
            && std::io::stdout().is_terminal();

        table.load_style(UTF8_FULL.with_rounded_corners());
        table.set_content_arrangement(ContentArrangement::Dynamic);
        if color {
            table.enforce_styling();
        } else {
            table.force_no_tty();
        }

        table.set_header(vec![
            "Proxy",
            "Proto",
            "Status",
            "Latency",
            "Anonymity",
            "Exit IP",
            "Detail",
        ]);

        for result in &self.results {
            let endpoint = if cli.show_credentials {
                result.proxy.to_url()
            } else {
                result.proxy.redacted()
            };

            let status = if result.alive {
                styled("alive", Color::Green, color)
            } else {
                styled("dead", Color::Red, color)
            };

            let anonymity = match result.anonymity {
                Some(level) => match level {
                    AnonymityLevel::Elite => styled(level.as_str(), Color::Green, color),
                    AnonymityLevel::Anonymous => styled(level.as_str(), Color::Yellow, color),
                    _ => styled(level.as_str(), Color::DarkGrey, color),
                },
                None => Cell::new("-"),
            };

            let detail = match &result.error {
                Some(error) => truncate(error, 48),
                None => format!("HTTP {}", result.status_code.unwrap_or_default()),
            };

            table.add_row(vec![
                Cell::new(endpoint),
                Cell::new(result.proxy.protocol.as_str()),
                status,
                right(format!("{} ms", result.latency_ms())),
                anonymity,
                Cell::new(result.exit_ip.as_deref().unwrap_or("-")),
                Cell::new(detail),
            ]);
        }

        let mut output = table.to_string();
        output.push('\n');
        output
    }

    fn render_csv(&self, cli: &Cli) -> Result<String> {
        let mut writer = csv::Writer::from_writer(Vec::new());
        for row in self.rows(cli) {
            writer.serialize(row).context("cannot encode CSV report")?;
        }
        let bytes = writer.into_inner().context("cannot encode CSV report")?;
        String::from_utf8(bytes).context("CSV report is not valid UTF-8")
    }
}

fn keep(result: &CheckResult, cli: &Cli) -> bool {
    if cli.alive_only && !result.alive {
        return false;
    }
    if let Some(max) = cli.max_latency
        && result.alive
        && result.latency_ms() > max as u128
    {
        return false;
    }
    if let Some(minimum) = cli.min_anonymity
        && !result
            .anonymity
            .is_some_and(|level| level.satisfies(minimum))
    {
        return false;
    }
    true
}

fn styled(text: &str, color: Color, plain: bool) -> Cell {
    if plain {
        Cell::new(text)
    } else {
        Cell::new(text).fg(color)
    }
}

fn right(text: String) -> Cell {
    Cell::new(text).set_alignment(CellAlignment::Right)
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

/// One proxy in a machine readable report.
#[derive(Debug, Serialize)]
pub struct Row {
    pub proxy: String,
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub alive: bool,
    pub latency_ms: u64,
    pub status_code: Option<u16>,
    pub anonymity: Option<String>,
    pub exit_ip: Option<String>,
    pub error: Option<String>,
}

impl Row {
    fn new(result: &CheckResult, show_credentials: bool) -> Self {
        Self {
            proxy: if show_credentials {
                result.proxy.to_url()
            } else {
                result.proxy.redacted()
            },
            protocol: result.proxy.protocol.as_str().to_string(),
            host: result.proxy.host.clone(),
            port: result.proxy.port,
            alive: result.alive,
            latency_ms: result.latency_ms() as u64,
            status_code: result.status_code,
            anonymity: result.anonymity.map(|level| level.as_str().to_string()),
            exit_ip: result.exit_ip.clone(),
            error: result.error.clone(),
        }
    }
}

/// Aggregate statistics over every checked proxy.
#[derive(Debug)]
pub struct Stats {
    pub total: usize,
    pub alive: usize,
    pub dead: usize,
    pub avg_latency_ms: Option<u128>,
    pub fastest_ms: Option<u128>,
    pub slowest_ms: Option<u128>,
    pub elite: usize,
    pub anonymous: usize,
    pub transparent: usize,
    pub unknown: usize,
    pub elapsed: Duration,
}

impl Stats {
    fn new(results: &[CheckResult], elapsed: Duration) -> Self {
        let alive_results: Vec<&CheckResult> = results.iter().filter(|r| r.alive).collect();
        let latencies: Vec<u128> = alive_results.iter().map(|r| r.latency_ms()).collect();

        let count = |level: AnonymityLevel| {
            alive_results
                .iter()
                .filter(|result| result.anonymity == Some(level))
                .count()
        };

        Self {
            total: results.len(),
            alive: alive_results.len(),
            dead: results.len() - alive_results.len(),
            avg_latency_ms: if latencies.is_empty() {
                None
            } else {
                Some(latencies.iter().sum::<u128>() / latencies.len() as u128)
            },
            fastest_ms: latencies.iter().min().copied(),
            slowest_ms: latencies.iter().max().copied(),
            elite: count(AnonymityLevel::Elite),
            anonymous: count(AnonymityLevel::Anonymous),
            transparent: count(AnonymityLevel::Transparent),
            unknown: count(AnonymityLevel::Unknown),
            elapsed,
        }
    }

    pub fn render(&self) -> String {
        let share = |part: usize| {
            if self.total == 0 {
                0.0
            } else {
                part as f64 * 100.0 / self.total as f64
            }
        };

        let mut lines = Vec::new();
        lines.push(format!(
            "checked {} proxies in {}",
            self.total,
            human_duration(self.elapsed)
        ));
        lines.push(format!(
            "alive {} ({:.1}%) · dead {} ({:.1}%)",
            self.alive,
            share(self.alive),
            self.dead,
            share(self.dead)
        ));
        if let (Some(avg), Some(fastest), Some(slowest)) =
            (self.avg_latency_ms, self.fastest_ms, self.slowest_ms)
        {
            lines.push(format!(
                "latency avg {avg} ms · fastest {fastest} ms · slowest {slowest} ms"
            ));
        }
        if self.elite + self.anonymous + self.transparent + self.unknown > 0 {
            lines.push(format!(
                "anonymity elite {} · anonymous {} · transparent {} · unknown {}",
                self.elite, self.anonymous, self.transparent, self.unknown
            ));
        }
        lines.join("\n")
    }
}

fn human_duration(duration: Duration) -> String {
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
    use crate::proxy::{Protocol, Proxy};
    use clap::Parser;

    fn cli(args: &[&str]) -> Cli {
        let mut argv = vec!["proxyforge", "1.1.1.1:1"];
        argv.extend_from_slice(args);
        Cli::parse_from(argv)
    }

    fn alive(port: u16, latency_ms: u64, exit_ip: &str) -> CheckResult {
        let mut result = CheckResult::alive(
            Proxy::new(Protocol::Http, "10.0.0.1", port),
            Duration::from_millis(latency_ms),
            Some(200),
        );
        result.exit_ip = Some(exit_ip.to_string());
        result
    }

    fn dead(port: u16) -> CheckResult {
        CheckResult::dead(
            Proxy::new(Protocol::Socks5, "10.0.0.2", port),
            Duration::from_millis(500),
            "cannot connect: refused",
        )
    }

    #[test]
    fn sorts_by_latency_with_dead_last() {
        let results = vec![dead(1), alive(2, 300, "1.1.1.1"), alive(3, 100, "2.2.2.2")];
        let report = Report::build(results, &cli(&[]), Duration::from_secs(1));
        let ports: Vec<u16> = report.results.iter().map(|r| r.proxy.port).collect();
        assert_eq!(ports, vec![3, 2, 1]);
    }

    #[test]
    fn alive_only_filter_drops_dead_entries() {
        let results = vec![dead(1), alive(2, 300, "1.1.1.1")];
        let report = Report::build(results, &cli(&["--alive-only"]), Duration::from_secs(1));
        assert_eq!(report.results.len(), 1);
        assert!(report.results[0].alive);
    }

    #[test]
    fn max_latency_filter_keeps_dead_entries() {
        let results = vec![dead(1), alive(2, 300, "1.1.1.1"), alive(3, 900, "2.2.2.2")];
        let report = Report::build(
            results,
            &cli(&["--max-latency", "500"]),
            Duration::from_secs(1),
        );
        let ports: Vec<u16> = report.results.iter().map(|r| r.proxy.port).collect();
        assert!(ports.contains(&1));
        assert!(ports.contains(&2));
        assert!(!ports.contains(&3));
    }

    #[test]
    fn min_anonymity_filter_excludes_unknown_levels() {
        let mut anonymous = alive(2, 100, "1.1.1.1");
        anonymous.anonymity = Some(AnonymityLevel::Anonymous);
        let mut elite = alive(3, 100, "2.2.2.2");
        elite.anonymity = Some(AnonymityLevel::Elite);

        let report = Report::build(
            vec![dead(1), anonymous, elite],
            &cli(&["--min-anonymity", "anonymous"]),
            Duration::from_secs(1),
        );
        assert_eq!(report.results.len(), 2);
    }

    #[test]
    fn renders_json_rows() {
        let report = Report::build(
            vec![alive(2, 123, "1.1.1.1")],
            &cli(&["--format", "json"]),
            Duration::from_secs(1),
        );
        let text = report.render(&cli(&["--format", "json"])).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value[0]["latency_ms"], 123);
        assert_eq!(value[0]["proxy"], "http://10.0.0.1:2");
        assert_eq!(value[0]["alive"], true);
    }

    #[test]
    fn renders_csv_with_header() {
        let args = cli(&["--format", "csv"]);
        let report = Report::build(
            vec![alive(2, 123, "1.1.1.1")],
            &args,
            Duration::from_secs(1),
        );
        let text = report.render(&args).unwrap();
        assert!(text.starts_with("proxy,protocol,host,port,alive,latency_ms"));
    }

    #[test]
    fn quiet_mode_lists_alive_endpoints_only() {
        let args = cli(&["--quiet"]);
        let report = Report::build(
            vec![dead(1), alive(2, 10, "1.1.1.1")],
            &args,
            Duration::from_secs(1),
        );
        let text = report.render(&args).unwrap();
        assert_eq!(text, "http://10.0.0.1:2\n");
    }

    #[test]
    fn redacts_credentials_by_default() {
        let mut result = CheckResult::alive(
            Proxy::new(Protocol::Socks5, "10.0.0.1", 1080)
                .with_credentials("user", Some("secret".into())),
            Duration::from_millis(10),
            Some(200),
        );
        result.exit_ip = Some("1.1.1.1".into());
        let args = cli(&["--format", "json"]);
        let report = Report::build(vec![result], &args, Duration::from_secs(1));
        let text = report.render(&args).unwrap();
        assert!(!text.contains("secret"));

        let args = cli(&["--format", "json", "--show-credentials"]);
        let report = Report::build(vec![alive(2, 1, "1.1.1.1")], &args, Duration::from_secs(1));
        assert!(report.render(&args).is_ok());
    }

    #[test]
    fn statistics_cover_all_checks() {
        let results = vec![dead(1), alive(2, 100, "1.1.1.1"), alive(3, 300, "2.2.2.2")];
        let report = Report::build(results, &cli(&["--alive-only"]), Duration::from_secs(2));
        assert_eq!(report.stats.total, 3);
        assert_eq!(report.stats.alive, 2);
        assert_eq!(report.stats.dead, 1);
        assert_eq!(report.stats.avg_latency_ms, Some(200));
        assert!(report.stats.render().contains("2.0 s"));
    }

    #[test]
    fn truncates_long_details() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
    }
}
