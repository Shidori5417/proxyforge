use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use tokio::sync::mpsc;

use proxyforge::anonymity;
use proxyforge::checker::{self, CheckSettings};
use proxyforge::cli::{Cli, Command};
use proxyforge::geo::GeoDb;
use proxyforge::input;
use proxyforge::output::{Report, render_alive};
use proxyforge::report::CheckResult;
use proxyforge::serve::{self, ServeConfig};
use proxyforge::transport::{Target, get_direct};
use proxyforge::tui::{self, TuiConfig};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(Command::Completions { shell }) = &cli.command {
        let mut command = <Cli as clap::CommandFactory>::command();
        clap_complete::generate(*shell, &mut command, "proxyforge", &mut std::io::stdout());
        return Ok(());
    }

    run(cli).await
}

async fn run(cli: Cli) -> Result<()> {
    let loaded = input::load(&cli).await?;
    for warning in loaded.warnings() {
        eprintln!("warning: {warning}");
    }

    if cli.tui && cli.serve.is_some() {
        bail!("--tui and --serve cannot be combined");
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

    let mut judges = Vec::with_capacity(cli.judge.len());
    for url in &cli.judge {
        judges.push(Target::parse(url).with_context(|| format!("invalid judge URL `{url}`"))?);
    }

    if cli.tui && cli.serve.is_some() {
        bail!("--tui and --serve cannot be combined: the dashboard cannot host the rotator");
    }

    let local_ip = if cli.no_anonymity {
        None
    } else {
        resolve_local_ip(&cli).await
    };
    if let Some(ip) = &local_ip
        && !cli.quiet
    {
        eprintln!("local IP: {ip}");
    }

    let geo = if cli.geo_db.is_empty() {
        None
    } else {
        let paths: Vec<std::path::PathBuf> =
            cli.geo_db.iter().map(std::path::PathBuf::from).collect();
        Some(Arc::new(
            GeoDb::open(&paths).context("cannot load the GeoIP databases")?,
        ))
    };

    let settings = Arc::new(CheckSettings {
        judges,
        timeout: cli.timeout_duration(),
        retries: cli.retries,
        expect: cli.expect.clone(),
        detect: cli.detect,
        geo,
        anonymity: !cli.no_anonymity,
        local_ip,
    });

    if cli.tui {
        tui::run(
            proxies,
            settings,
            TuiConfig {
                judge: cli
                    .judge
                    .first()
                    .and_then(|url| Target::parse(url).ok())
                    .map(|target| target.host_header())
                    .unwrap_or_else(|| "judge".into()),
                workers: cli.concurrency as usize,
                show_credentials: cli.show_credentials,
                sort: cli.sort,
                export_path: cli
                    .save
                    .clone()
                    .unwrap_or_else(|| "proxyforge-export.txt".into()),
            },
        )
        .await?;
        return Ok(());
    }

    let total = proxies.len();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let cancel = Arc::new(AtomicBool::new(false));

    let started = Instant::now();
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

    // Alive results feed both --save and the pool behind --serve.
    let alive: Vec<CheckResult> = results
        .iter()
        .filter(|result| result.alive)
        .cloned()
        .collect();
    let pool: Vec<proxyforge::proxy::Proxy> =
        alive.iter().map(|result| result.proxy.clone()).collect();
    let pool_size = pool.len();

    let report = Report::build(results, &cli, started.elapsed());
    write_report(&cli, &report.render(&cli)?)?;

    if !cli.quiet && !cli.no_summary {
        eprintln!("\n{}", report.stats.render());
    }

    if cli.save.is_some() {
        save_alive(&cli, &alive)?;
    }

    if interrupted {
        eprintln!("interrupted: reporting partial results");
        std::process::exit(130);
    }

    if let Some(bind) = &cli.serve {
        let config = ServeConfig {
            bind: bind
                .parse()
                .with_context(|| format!("invalid --serve address `{bind}`"))?,
            rotate: cli.rotate.max(1),
            random: cli.rotate_random,
            rotate_on_error: cli.rotate_on_error,
        };

        let (address, server) = serve::start(pool, config, cli.timeout_duration())
            .await
            .context("cannot start the rotating proxy server")?;

        if !cli.quiet {
            eprintln!("\nserving {pool_size} proxies on {address} · Ctrl-C to stop");
        }
        let _ = tokio::signal::ctrl_c().await;
        server.abort();
        if !cli.quiet {
            eprintln!("stopped");
        }
    }

    Ok(())
}

/// Writes the alive endpoints (or rows) to the `--save` file.
fn save_alive(cli: &Cli, alive: &[CheckResult]) -> Result<()> {
    let Some(path) = &cli.save else {
        return Ok(());
    };

    let text = render_alive(alive, cli.save_format);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(!cli.save_append)
        .append(cli.save_append)
        .open(path)
        .with_context(|| format!("cannot write `{}`", path))?;

    use std::io::Write as _;
    file.write_all(text.as_bytes())
        .with_context(|| format!("cannot write `{}`", path))?;

    if !cli.quiet {
        eprintln!("saved {} alive proxies to {}", alive.len(), path);
    }
    Ok(())
}

fn write_report(cli: &Cli, text: &str) -> Result<()> {
    if let Some(path) = &cli.output {
        std::fs::write(path, text)
            .with_context(|| format!("cannot write the report to `{}`", path.display()))?;
        if !cli.quiet {
            eprintln!("report written to {}", path.display());
        }
        return Ok(());
    }

    let mut stdout = std::io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => other.context("cannot write the report to stdout"),
    }
}

async fn resolve_local_ip(cli: &Cli) -> Option<String> {
    let target = Target::parse(&cli.local_ip_url).ok()?;
    match get_direct(&target, Duration::from_secs(5)).await {
        Ok(response) if response.is_success() => anonymity::exit_ip_from_text(&response.text()),
        Ok(response) => {
            if !cli.quiet {
                eprintln!(
                    "warning: cannot determine the local IP: HTTP {}",
                    response.status
                );
            }
            None
        }
        Err(error) => {
            if !cli.quiet {
                eprintln!("warning: cannot determine the local IP: {error}");
            }
            None
        }
    }
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
