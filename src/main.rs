mod cli;
mod input;
mod proxy;
mod transport;

use anyhow::{Result, bail};
use clap::Parser;

use crate::cli::Cli;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let loaded = input::load(&cli)?;

    for warning in loaded.warnings() {
        eprintln!("warning: {warning}");
    }

    let mut proxies = loaded.proxies;
    if !cli.protocols.is_empty() {
        proxies.retain(|proxy| cli.protocols.contains(&proxy.protocol));
    }
    if proxies.is_empty() {
        bail!("no proxies left after applying the protocol filter");
    }

    if !cli.quiet {
        eprintln!("loaded {} proxies", proxies.len());
    }
    for proxy in &proxies {
        if cli.show_credentials {
            println!("{}", proxy.to_url());
        } else {
            println!("{}", proxy.redacted());
        }
    }

    Ok(())
}
