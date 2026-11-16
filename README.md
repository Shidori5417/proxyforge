# proxyforge

Fast, concurrent proxy checker for **HTTP**, **HTTPS**, **SOCKS4** and **SOCKS5**
proxies, written in Rust.

[![CI](https://github.com/Shidori5417/proxyforge/actions/workflows/ci.yml/badge.svg)](https://github.com/Shidori5417/proxyforge/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Türkçe dokümantasyon: [README.tr.md](README.tr.md)

![proxyforge checking a mixed batch of HTTP, SOCKS4 and SOCKS5 proxies](docs/demo.png)

## Features

- Checks HTTP, HTTPS (via `CONNECT`), SOCKS4/4a, SOCKS5 and SOCKS5h proxies.
- Username/password authentication for HTTP (`Proxy-Authorization`) and SOCKS5.
- High concurrency with a worker pool — a slow proxy never blocks the line.
- Per-proxy timeout, optional retries and accurate latency measurements.
- Anonymity classification: **transparent**, **anonymous**, **elite** or unknown.
- Reports as a coloured table, JSON, JSONL or CSV.
- Filters for alive-only, maximum latency and minimum anonymity.
- Graceful `Ctrl-C`: partial results are still reported.
- No global HTTP client or connection pooling, so every proxy is measured on
  its own connection.

## Install

Requires a recent stable Rust toolchain (1.85+).

```bash
git clone https://github.com/Shidori5417/proxyforge.git
cd proxyforge
cargo install --path .
```

Or build a release binary:

```bash
cargo build --release
./target/release/proxyforge --help
```

## Quick start

```bash
# Check two endpoints
proxyforge 1.2.3.4:8080 socks5://5.6.7.8:1080

# Check a list with 200 parallel workers
proxyforge --input proxies.txt --concurrency 200

# Keep only fast, elite proxies and export them
proxyforge -i proxies.txt --alive-only --max-latency 800 \
  --min-anonymity elite --format json --output elite.json

# Pipe a list in, keep plain endpoints on stdout
cat proxies.txt | proxyforge -i - --quiet > alive.txt
```

## Input formats

One endpoint per line in a file, or one argument per endpoint:

```text
1.2.3.4:8080                       # HTTP proxy on port 8080
http://1.2.3.4:3128
https://proxy.example.com:443
socks4://1.2.3.4:1080
socks5://1.2.3.4:1080
socks5h://1.2.3.4:1080             # remote DNS resolution
socks5://user:pass@1.2.3.4:1080
http://user:p%40ss@1.2.3.4:8080    # percent-escapes are decoded
[2001:db8::1]:8080                 # bracketed IPv6
```

Empty lines and lines starting with `#` are ignored. Duplicates are dropped and
unparsable lines are reported as warnings.

## Usage

```text
proxyforge [OPTIONS] [PROXY]...
```

| Option | Description |
| --- | --- |
| `-i, --input <FILE>` | Read endpoints from a file; `-` reads stdin |
| `-j, --judge <URL>` | URL requested through every proxy (default `https://httpbin.org/get`) |
| `-c, --concurrency <N>` | Proxies checked in parallel (default `100`) |
| `-t, --timeout <SECS>` | Per-proxy timeout (default `8`) |
| `-r, --retries <N>` | Extra attempts after a failure (default `0`) |
| `-f, --format <FORMAT>` | `table`, `json`, `jsonl` or `csv` (default `table`) |
| `-o, --output <FILE>` | Write the report to a file |
| `--alive-only` | Report only proxies that responded |
| `--max-latency <MS>` | Drop proxies slower than this |
| `--min-anonymity <LEVEL>` | `transparent`, `anonymous` or `elite` |
| `--protocols <LIST>` | Only check these protocols, comma separated |
| `--sort <ORDER>` | `latency` (default), `proxy` or `status` |
| `--no-anonymity` | Skip anonymity detection (saves one request) |
| `--local-ip-url <URL>` | URL used once to learn the local public IP |
| `--show-credentials` | Print credentials instead of `***:***` |
| `-q, --quiet` | Print only alive endpoints, no table or summary |
| `--no-summary` | Hide the summary block |
| `--no-progress` | Hide the progress bar |
| `--no-color` | Disable ANSI colours |

## Output

The table goes to stdout and the summary to stderr, so machine-readable output
stays clean:

```text
╭────────────────────────┬───────┬────────┬─────────┬─────────────┬────────────────┬──────────╮
│ Proxy                  ┆ Proto ┆ Status ┆ Latency ┆ Anonymity   ┆ Exit IP        ┆ Detail   │
╞════════════════════════╪═══════╪════════╪═════════╪═════════════╪════════════════╪══════════╡
│ socks5://1.2.3.4:1080  ┆ socks5┆ alive  ┆  142 ms ┆ elite       ┆ 203.0.113.9    ┆ HTTP 200 │
╰────────────────────────┴───────┴────────┴─────────┴─────────────┴────────────────┴──────────╯

checked 1 proxies in 142 ms
alive 1 (100.0%) · dead 0 (0.0%)
latency avg 142 ms · fastest 142 ms · slowest 142 ms
anonymity elite 1 · anonymous 0 · transparent 0 · unknown 0
```

JSON output is an array of objects:

```json
[
  {
    "proxy": "socks5://***:***@1.2.3.4:1080",
    "protocol": "socks5",
    "host": "1.2.3.4",
    "port": 1080,
    "alive": true,
    "latency_ms": 142,
    "status_code": 200,
    "anonymity": "elite",
    "exit_ip": "203.0.113.9",
    "error": null
  }
]
```

CSV uses the same fields. Use `--show-credentials` to keep usernames and
passwords in the output.

## Anonymity levels

The checker resolves your own public IP once (without a proxy) and then compares
it with what the judge sees through each proxy:

| Level | Meaning |
| --- | --- |
| `elite` | No forwarding header and no client IP visible |
| `anonymous` | Forwarding headers visible (`X-Forwarded-For`, `Via`, …) but the client IP is hidden |
| `transparent` | Your public IP is visible, either as the origin or inside a header |
| `unknown` | The judge did not echo enough information |

Detection needs a judge that echoes the request headers, such as
`https://httpbin.org/get` (the default). Judges that only return an IP, for
example `https://api.ipify.org`, still prove that a proxy works but leave the
level `unknown`. Disable the extra lookup with `--no-anonymity`.

> A proxy running on the same network as the checker can legitimately look
> `transparent`, because its exit IP is your own.

## How it works

```
src/
├── proxy.rs      # endpoint parsing and redaction
├── transport.rs  # HTTP/1.1 client: CONNECT, SOCKS4a, SOCKS5, TLS
├── checker.rs    # worker pool, retries, latency
├── anonymity.rs  # judge payload analysis
├── output.rs     # filtering, sorting, table/JSON/CSV
├── input.rs      # argument, file and stdin loading
└── main.rs       # CLI wiring, progress and Ctrl-C handling
```

The transport is intentionally small: one `GET`, one connection per check, no
pooling and no redirects. That keeps failures attributable to the proxy itself.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Run completed |
| `1` | Invalid arguments, unreadable input or invalid judge URL |
| `130` | Interrupted with `Ctrl-C`; partial results were written |

## Development

```bash
cargo test            # unit and integration tests, fully offline
cargo clippy --all-targets -- -D warnings
cargo fmt
```

The integration suite starts local mock HTTP, SOCKS4 and SOCKS5 proxies and a
mock judge on ephemeral loopback ports.

## License

[MIT](LICENSE)
