//! Proxy list loading from arguments, files, stdin and remote URLs.
//!
//! Local files are parsed line by line so comments keep working. Remote
//! sources are scraped with a forgiving pattern that pulls endpoints out of
//! raw text, HTML or JSON, because public proxy lists rarely come as clean
//! line-per-endpoint files.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read};
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use regex::Regex;

use crate::cli::Cli;
use crate::proxy::{ParseProxyError, Proxy};
use crate::transport::{Target, get_direct};

/// Timeout for fetching a remote proxy list.
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// An entry that could not be turned into a [`Proxy`].
#[derive(Debug)]
pub struct InvalidEntry {
    /// 1-based line number inside the input file, when known.
    pub line: Option<usize>,
    pub input: String,
    pub error: ParseProxyError,
}

/// Everything gathered from the command line.
#[derive(Debug, Default)]
pub struct LoadOutcome {
    pub proxies: Vec<Proxy>,
    pub duplicates: usize,
    pub invalid: Vec<InvalidEntry>,
    /// Endpoints scraped from remote sources.
    pub scraped: usize,
}

impl LoadOutcome {
    /// Human readable warnings for the entries that were skipped.
    pub fn warnings(&self) -> Vec<String> {
        self.invalid
            .iter()
            .map(|entry| match entry.line {
                Some(line) => format!("line {line}: skipped `{}`: {}", entry.input, entry.error),
                None => format!("skipped `{}`: {}", entry.input, entry.error),
            })
            .collect()
    }
}

/// Reads the proxy list from positional arguments and every `--input` source.
pub async fn load(cli: &Cli) -> Result<LoadOutcome> {
    let mut outcome = LoadOutcome::default();
    let mut seen = HashSet::new();

    for raw in &cli.proxies {
        add_entry(&mut outcome, &mut seen, raw, None);
    }

    for source in &cli.input {
        if is_url(source) {
            fetch_list(source, &mut outcome, &mut seen).await?;
        } else if source == "-" {
            let content = read_stdin()?;
            add_lines(&mut outcome, &mut seen, &content, Some(0));
        } else {
            let content = fs::read_to_string(source)
                .with_context(|| format!("cannot read proxy file `{source}`"))?;
            add_lines(&mut outcome, &mut seen, &content, Some(0));
        }
    }

    if outcome.proxies.is_empty() && outcome.invalid.is_empty() {
        bail!(
            "no proxies given\n\nPass endpoints directly, use --input FILE or a URL, or pipe a list:\n  \
             proxyforge 1.2.3.4:8080\n  proxyforge -i proxies.txt\n  proxyforge -i https://example.com/list\n  \
             cat proxies.txt | proxyforge -i -"
        );
    }

    Ok(outcome)
}

fn is_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

fn add_lines(
    outcome: &mut LoadOutcome,
    seen: &mut HashSet<String>,
    content: &str,
    origin: Option<usize>,
) {
    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let line_number = origin.map(|start| start + index + 1);
        add_entry(outcome, seen, trimmed, line_number);
    }
}

fn add_entry(
    outcome: &mut LoadOutcome,
    seen: &mut HashSet<String>,
    raw: &str,
    line: Option<usize>,
) {
    match Proxy::from_str(raw) {
        Ok(proxy) => {
            if seen.insert(proxy.key()) {
                outcome.proxies.push(proxy);
            } else {
                outcome.duplicates += 1;
            }
        }
        Err(error) => outcome.invalid.push(InvalidEntry {
            line,
            input: raw.to_string(),
            error,
        }),
    }
}

async fn fetch_list(
    url: &str,
    outcome: &mut LoadOutcome,
    seen: &mut HashSet<String>,
) -> Result<()> {
    let target = Target::parse(url).with_context(|| format!("invalid source URL `{url}`"))?;
    let response = get_direct(&target, FETCH_TIMEOUT)
        .await
        .with_context(|| format!("cannot fetch `{url}`"))?;
    if !response.is_success() {
        bail!("cannot fetch `{url}`: HTTP {}", response.status);
    }

    let endpoints = extract_endpoints(&response.text());
    if endpoints.is_empty() {
        bail!("no proxy endpoints found in `{url}`");
    }

    // Scraped matches are best effort: anything that does not parse cleanly
    // is dropped silently instead of polluting the warnings.
    for endpoint in endpoints {
        if let Ok(proxy) = Proxy::from_str(&endpoint)
            && seen.insert(proxy.key())
        {
            outcome.proxies.push(proxy);
            outcome.scraped += 1;
        }
    }

    if outcome.scraped == 0 {
        bail!("every endpoint scraped from `{url}` was a duplicate");
    }
    Ok(())
}

/// Pulls proxy endpoints out of arbitrary text: plain lists, HTML pages and
/// JSON blobs all work.
pub fn extract_endpoints(text: &str) -> Vec<String> {
    static SCHEMED: OnceLock<Regex> = OnceLock::new();
    static BARE: OnceLock<Regex> = OnceLock::new();

    let schemed = SCHEMED.get_or_init(|| {
        Regex::new(concat!(
            r#"(?i)\b(?:https?|socks4a?|socks5h?)://"#,
            r#"(?:[^\s@/"']+:[^\s@/"']*@|[^\s@/"']+@)?"#,
            r#"(?:\[[0-9A-Fa-f:]+\]|[0-9A-Za-z][0-9A-Za-z._-]*)"#,
            r#"(?::[0-9]{1,5})?"#,
        ))
        .expect("scheme pattern is valid")
    });
    let bare = BARE.get_or_init(|| {
        Regex::new(r"\b(?:[0-9]{1,3}\.){3}[0-9]{1,3}:[0-9]{2,5}\b|\[[0-9A-Fa-f:]+\]:[0-9]{1,5}\b")
            .expect("bare pattern is valid")
    });

    let schemed_matches: Vec<(usize, usize)> = schemed
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect();
    let mut endpoints: Vec<String> = schemed_matches
        .iter()
        .map(|(start, end)| text[*start..*end].to_string())
        .collect();

    // Blank out the schemed spans so the bare pattern does not pick up the
    // `host:port` part of an already matched URL again.
    let masked = mask_ranges(text, &schemed_matches);
    endpoints.extend(bare.find_iter(&masked).map(|m| m.as_str().to_string()));
    endpoints
}

/// Replaces the given byte ranges with spaces, keeping the string length.
fn mask_ranges(text: &str, ranges: &[(usize, usize)]) -> String {
    let mut masked = text.as_bytes().to_vec();
    for (start, end) in ranges {
        for byte in &mut masked[*start..*end] {
            *byte = b' ';
        }
    }
    String::from_utf8_lossy(&masked).into_owned()
}

fn read_stdin() -> Result<String> {
    let mut buffer = String::new();
    io::stdin()
        .read_to_string(&mut buffer)
        .context("cannot read proxies from stdin")?;
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn cli_with(args: &[&str]) -> Cli {
        let mut argv = vec!["proxyforge"];
        argv.extend_from_slice(args);
        Cli::parse_from(argv)
    }

    #[tokio::test]
    async fn loads_file_skipping_comments_and_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proxies.txt");
        fs::write(
            &path,
            "# comment\n\n1.2.3.4:8080\n1.2.3.4:8080\nsocks5://5.6.7.8:1080\nbad entry\n",
        )
        .unwrap();

        let outcome = load(&cli_with(&["--input", path.to_str().unwrap()]))
            .await
            .unwrap();
        assert_eq!(outcome.proxies.len(), 2);
        assert_eq!(outcome.duplicates, 1);
        assert_eq!(outcome.invalid.len(), 1);
        assert_eq!(outcome.invalid[0].line, Some(6));
    }

    #[tokio::test]
    async fn combines_arguments_and_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proxies.txt");
        fs::write(&path, "5.6.7.8:1080\n").unwrap();

        let outcome = load(&cli_with(&[
            "1.2.3.4:8080",
            "--input",
            path.to_str().unwrap(),
        ]))
        .await
        .unwrap();
        assert_eq!(outcome.proxies.len(), 2);
    }

    #[tokio::test]
    async fn accepts_multiple_input_sources() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("one.txt");
        let second = dir.path().join("two.txt");
        fs::write(&first, "1.2.3.4:80\n").unwrap();
        fs::write(&second, "5.6.7.8:1080\n1.2.3.4:80\n").unwrap();

        let outcome = load(&cli_with(&[
            "--input",
            first.to_str().unwrap(),
            "--input",
            second.to_str().unwrap(),
        ]))
        .await
        .unwrap();
        assert_eq!(outcome.proxies.len(), 2);
        assert_eq!(outcome.duplicates, 1);
    }

    #[tokio::test]
    async fn empty_input_is_an_error() {
        assert!(load(&cli_with(&[])).await.is_err());
    }

    #[test]
    fn extraction_does_not_double_count_urls() {
        let found = extract_endpoints("http://user:pass@127.0.0.1:3128 and 1.2.3.4:8080");
        assert_eq!(
            found,
            vec!["http://user:pass@127.0.0.1:3128", "1.2.3.4:8080"]
        );
    }

    #[test]
    fn extracts_endpoints_from_plain_lists() {
        let found = extract_endpoints("1.2.3.4:8080\r\n5.6.7.8:3128\n9.9.9.9:80");
        assert_eq!(found, vec!["1.2.3.4:8080", "5.6.7.8:3128", "9.9.9.9:80"]);
    }

    #[test]
    fn extracts_endpoints_from_html() {
        let html = r#"<td>socks5://user:pass@10.0.0.1:1080</td><td>1.2.3.4:8080</td>"#;
        let found = extract_endpoints(html);
        assert!(found.contains(&"socks5://user:pass@10.0.0.1:1080".to_string()));
        assert!(found.contains(&"1.2.3.4:8080".to_string()));
    }

    #[test]
    fn extracts_endpoints_from_json() {
        let json = r#"{"proxies":[{"ip":"1.2.3.4","port":8080,"url":"http://5.6.7.8:3128"}]}"#;
        let found = extract_endpoints(json);
        assert!(found.contains(&"http://5.6.7.8:3128".to_string()));
    }

    #[test]
    fn extracts_ipv6_endpoints() {
        let found = extract_endpoints("gateway [2001:db8::1]:8080 online");
        assert!(found.contains(&"[2001:db8::1]:8080".to_string()));
    }

    #[test]
    fn does_not_scrape_bare_addresses_inside_schemed_urls() {
        let found = extract_endpoints("http://user:pass@1.2.3.4:8080 and 5.6.7.8:3128");
        assert_eq!(found.len(), 2);
        assert!(found.contains(&"http://user:pass@1.2.3.4:8080".to_string()));
        assert!(found.contains(&"5.6.7.8:3128".to_string()));
    }

    #[test]
    fn extraction_ignores_non_proxies() {
        let found = extract_endpoints("version 1.2.3.4 built at 10:20:30 no proxies here");
        assert!(found.is_empty());
    }
}
