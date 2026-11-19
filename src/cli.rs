//! Command line interface definition.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};

use crate::anonymity::AnonymityLevel;
use crate::proxy::Protocol;

const EXAMPLES: &str = "\
EXAMPLES:
  proxyforge 1.2.3.4:8080 socks5://5.6.7.8:1080
  proxyforge --input proxies.txt --concurrency 200
  cat proxies.txt | proxyforge --input - --format json --output alive.json
  proxyforge -i proxies.txt --alive-only --max-latency 1500 -f csv";

#[derive(Debug, Clone, Parser)]
#[command(
    name = "proxyforge",
    version,
    about = "Fast, concurrent proxy checker for HTTP, HTTPS, SOCKS4 and SOCKS5",
    after_help = EXAMPLES
)]
pub struct Cli {
    /// Proxy endpoints to check (host:port or scheme://user:pass@host:port)
    #[arg(value_name = "PROXY")]
    pub proxies: Vec<String>,

    /// Read proxies from files or URLs; `-` reads stdin (repeatable)
    #[arg(short, long, value_name = "FILE_OR_URL")]
    pub input: Vec<String>,

    /// URL(s) requested through every proxy; attempts rotate through them
    #[arg(
        short,
        long,
        value_name = "URL",
        default_values_t = vec![String::from("https://httpbin.org/get")]
    )]
    pub judge: Vec<String>,

    /// Require this text in the judge response, otherwise the proxy is dead
    #[arg(long, value_name = "TEXT")]
    pub expect: Option<String>,

    /// Probe schemesless endpoints for HTTP, SOCKS5 and SOCKS4 support
    #[arg(long)]
    pub detect: bool,

    /// Do not classify proxies by anonymity
    #[arg(long)]
    pub no_anonymity: bool,

    /// Offline GeoIP database(s) in MaxMind mmdb format, repeatable
    #[arg(long, value_name = "FILE")]
    pub geo_db: Vec<String>,

    /// Only report proxies exiting in these countries (comma separated ISO codes)
    #[arg(long, value_delimiter = ',', value_name = "CODES")]
    pub country: Vec<String>,

    /// Drop proxies exiting in these countries
    #[arg(long, value_delimiter = ',', value_name = "CODES")]
    pub exclude_country: Vec<String>,

    /// URL used once, without a proxy, to learn the local public IP
    #[arg(long, value_name = "URL", default_value = "https://api.ipify.org")]
    pub local_ip_url: String,

    /// Only report proxies at or above this anonymity level
    #[arg(long, value_enum, value_name = "LEVEL")]
    pub min_anonymity: Option<AnonymityLevel>,

    /// Number of proxies checked in parallel
    #[arg(
        short = 'c',
        long,
        value_name = "N",
        default_value_t = 100,
        value_parser = clap::value_parser!(u16).range(1..)
    )]
    pub concurrency: u16,

    /// Per-proxy timeout in seconds
    #[arg(
        short = 't',
        long,
        value_name = "SECS",
        default_value_t = 8,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub timeout: u64,

    /// Extra attempts after the first failure
    #[arg(short = 'r', long, value_name = "N", default_value_t = 0)]
    pub retries: u32,

    /// Report format
    #[arg(short = 'f', long, value_enum, default_value_t = Format::Table)]
    pub format: Format,

    /// Write the report to a file instead of stdout
    #[arg(short = 'o', long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Only report proxies that responded
    #[arg(long)]
    pub alive_only: bool,

    /// Drop proxies slower than this many milliseconds
    #[arg(long, value_name = "MS")]
    pub max_latency: Option<u64>,

    /// Only check these protocols (comma separated)
    #[arg(long, value_enum, value_delimiter = ',', value_name = "LIST")]
    pub protocols: Vec<Protocol>,

    /// Order of the reported entries
    #[arg(long, value_enum, default_value_t = Sort::Latency)]
    pub sort: Sort,

    /// Show proxy credentials instead of redacting them
    #[arg(long)]
    pub show_credentials: bool,

    /// Open the interactive dashboard instead of printing a report
    #[arg(long)]
    pub tui: bool,

    /// After the check, serve the alive pool as a local rotating proxy
    #[arg(long, value_name = "BIND")]
    pub serve: Option<String>,

    /// Rotate to the next pool member every N requests (with --serve)
    #[arg(long, value_name = "N", default_value_t = 1)]
    pub rotate: usize,

    /// Pick pool members randomly instead of in order
    #[arg(long)]
    pub rotate_random: bool,

    /// On failure, try the next pool member
    #[arg(long)]
    pub rotate_on_error: bool,

    /// Print one endpoint per line, without table borders or summary
    #[arg(short, long)]
    pub quiet: bool,

    /// Do not print the summary block
    #[arg(long)]
    pub no_summary: bool,

    /// Do not render a progress bar
    #[arg(long)]
    pub no_progress: bool,

    /// Disable ANSI colours in the table
    #[arg(long)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Print shell completions for the given shell
    Completions { shell: clap_complete::Shell },
}

impl Cli {
    pub fn timeout_duration(&self) -> Duration {
        Duration::from_secs(self.timeout)
    }
}

/// Report format written to stdout or the output file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Human readable table
    Table,
    /// JSON array of result objects
    Json,
    /// One JSON object per line
    Jsonl,
    /// Comma separated values
    Csv,
}

impl Sort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Sort::Latency => "latency",
            Sort::Proxy => "proxy",
            Sort::Status => "status",
        }
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Format::Table => "table",
            Format::Json => "json",
            Format::Jsonl => "jsonl",
            Format::Csv => "csv",
        })
    }
}

/// Order in which results are reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Sort {
    /// Fastest first, dead proxies last
    Latency,
    /// Alphabetical by endpoint
    Proxy,
    /// Alive first, keeping the input order
    Status,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completions_subcommand_parses() {
        let cli = Cli::parse_from(["proxyforge", "completions", "fish"]);
        assert!(matches!(
            cli.command,
            Some(Command::Completions {
                shell: clap_complete::Shell::Fish
            })
        ));
    }

    #[test]
    fn no_subcommand_still_parses_endpoints() {
        let cli = Cli::parse_from(["proxyforge", "1.2.3.4:8080"]);
        assert!(cli.command.is_none());
        assert_eq!(cli.proxies, vec!["1.2.3.4:8080"]);
    }
}
