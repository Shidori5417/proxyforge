//! Proxy list loading from arguments, files and stdin.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read};
use std::str::FromStr;

use anyhow::{Context, Result, bail};

use crate::cli::Cli;
use crate::proxy::{ParseProxyError, Proxy};

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

/// Reads the proxy list from positional arguments and the optional input file.
pub fn load(cli: &Cli) -> Result<LoadOutcome> {
    let mut outcome = LoadOutcome::default();
    let mut seen = HashSet::new();

    for raw in &cli.proxies {
        add_entry(&mut outcome, &mut seen, raw, None);
    }

    if let Some(path) = &cli.input {
        let content = if path.as_os_str() == "-" {
            read_stdin()?
        } else {
            fs::read_to_string(path)
                .with_context(|| format!("cannot read proxy file `{}`", path.display()))?
        };

        for (index, line) in content.lines().enumerate() {
            let line_number = index + 1;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            add_entry(&mut outcome, &mut seen, trimmed, Some(line_number));
        }
    }

    if outcome.proxies.is_empty() && outcome.invalid.is_empty() {
        bail!(
            "no proxies given\n\nPass endpoints directly, use --input FILE, or pipe a list:\n  \
             proxyforge 1.2.3.4:8080\n  proxyforge -i proxies.txt\n  cat proxies.txt | proxyforge -i -"
        );
    }

    Ok(outcome)
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

    fn cli_with_input(path: &std::path::Path) -> Cli {
        Cli::parse_from(["proxyforge", "--input", path.to_str().unwrap()])
    }

    #[test]
    fn loads_file_skipping_comments_and_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proxies.txt");
        fs::write(
            &path,
            "# comment\n\n1.2.3.4:8080\n1.2.3.4:8080\nsocks5://5.6.7.8:1080\nbad entry\n",
        )
        .unwrap();

        let outcome = load(&cli_with_input(&path)).unwrap();
        assert_eq!(outcome.proxies.len(), 2);
        assert_eq!(outcome.duplicates, 1);
        assert_eq!(outcome.invalid.len(), 1);
        assert_eq!(outcome.invalid[0].line, Some(6));
    }

    #[test]
    fn combines_arguments_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proxies.txt");
        fs::write(&path, "5.6.7.8:1080\n").unwrap();

        let cli = Cli::parse_from([
            "proxyforge",
            "1.2.3.4:8080",
            "--input",
            path.to_str().unwrap(),
        ]);
        let outcome = load(&cli).unwrap();
        assert_eq!(outcome.proxies.len(), 2);
    }

    #[test]
    fn empty_input_is_an_error() {
        let cli = Cli::parse_from(["proxyforge"]);
        assert!(load(&cli).is_err());
    }
}
