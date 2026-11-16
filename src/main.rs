mod checker;
mod cli;
mod input;
mod proxy;
mod report;
mod transport;

use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use tokio::sync::mpsc;

use crate::checker::CheckSettings;
use crate::cli::Cli;
use crate::proxy::Proxy;
use crate::report::CheckResult;
use crate::transport::Target;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    run(cli).await
}

async fn run(cli: Cli) -> Result<()> {
    let loaded = input::load(&cli)?;
    for warning in loaded.warnings() {
        eprintln!("warning: {warning}");
    }

    let mut proxies = loaded.proxies;
    if !cli.protocols.is_empty() {
        proxies.retain(|proxy| cli.protocols.contains(&proxy.protocol));
    }
    if proxies.is_empty() {
        bail!(
            "nothing to check: {} entries were skipped and no proxy matched the protocol filter",
            loaded.invalid.len()
        );
    }

    let target =
        Target::parse(&cli.judge).with_context(|| format!("invalid judge URL `{}`", cli.judge))?;
    let settings = Arc::new(CheckSettings {
        target,
        timeout: cli.timeout_duration(),
        retries: cli.retries,
    });

    let total = proxies.len();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let _workers = checker::spawn_workers(
        proxies,
        settings,
        cli.concurrency as usize,
        Arc::clone(&cancel),
        sender,
    );

    let progress = build_progress(&cli, total);
    let mut results: Vec<CheckResult> = Vec::with_capacity(total);
    let mut interrupted = false;

    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                let _ = signal;
                interrupted = true;
                cancel.store(true, Ordering::Relaxed);
                break;
            }
            received = receiver.recv() => match received {
                Some(result) => {
                    if let Some(bar) = &progress {
                        bar.set_message(result.proxy.authority());
                        bar.inc(1);
                    }
                    results.push(result);
                }
                None => break,
            }
        }
    }

    if let Some(bar) = progress {
        bar.finish_and_clear();
    }

    print_results(&cli, &results);

    let alive = results.iter().filter(|result| result.alive).count();
    if !cli.quiet && !cli.no_summary {
        eprintln!(
            "\n{} checked, {alive} alive, {} dead",
            results.len(),
            results.len() - alive
        );
    }

    if interrupted {
        eprintln!("interrupted: reporting partial results");
        std::process::exit(130);
    }

    Ok(())
}

fn build_progress(cli: &Cli, total: usize) -> Option<ProgressBar> {
    if cli.no_progress || cli.quiet || !std::io::stderr().is_terminal() {
        return None;
    }

    let template = if cli.no_color {
        "{spinner} [{elapsed_precise}] [{bar:40}] {pos}/{len} {msg}"
    } else {
        "{spinner:.cyan} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}"
    };
    let style = ProgressStyle::with_template(template)
        .expect("progress template is valid")
        .progress_chars("=> ");

    let bar = ProgressBar::with_draw_target(Some(total as u64), ProgressDrawTarget::stderr());
    bar.set_style(style);
    bar.enable_steady_tick(Duration::from_millis(120));
    Some(bar)
}

fn print_results(cli: &Cli, results: &[CheckResult]) {
    let endpoint = |proxy: &Proxy| {
        if cli.show_credentials {
            proxy.to_url()
        } else {
            proxy.redacted()
        }
    };

    for result in results {
        if cli.alive_only && !result.alive {
            continue;
        }
        if cli.quiet {
            if result.alive {
                println!("{}", endpoint(&result.proxy));
            }
            continue;
        }

        match &result.error {
            None => println!(
                "alive  {:<44} {:>6} ms  HTTP {}",
                endpoint(&result.proxy),
                result.latency_ms(),
                result.status_code.unwrap_or_default()
            ),
            Some(error) => println!(
                "dead   {:<44} {:>6} ms  {error}",
                endpoint(&result.proxy),
                result.latency_ms()
            ),
        }
    }
}
