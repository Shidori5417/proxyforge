# proxyforge

Fast, concurrent proxy checker for **HTTP**, **HTTPS**, **SOCKS4** and **SOCKS5**
proxies, written in Rust.

[![CI](https://github.com/Shidori5417/proxyforge/actions/workflows/ci.yml/badge.svg)](https://github.com/Shidori5417/proxyforge/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Türkçe dokümantasyon: [README.tr.md](README.tr.md)

![proxyforge interactive dashboard](docs/tui.png)

## Features

- Checks HTTP, HTTPS (via `CONNECT`), SOCKS4/4a, SOCKS5 and SOCKS5h proxies.
- Interactive terminal dashboard (`--tui`) with live progress, filters, sorting, export and mouse support.
- Pulls proxy lists straight from URLs (`--input URL`) out of raw text, HTML or JSON.
- Protocol auto-detection (`--detect`) for lists that carry no scheme.
- Offline GeoIP enrichment (`--geo-db`) with country and ASN filters.
- Multiple judges with rotation and response verification (`--judge`, `--expect`).
- Rotating proxy server (`--serve`) that forwards traffic through the alive pool.
- Saves the survivors with `--save`, as endpoints, JSON or CSV.
- Settings editable live in the dashboard (`S`), re-running the batch on Enter.
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

Prebuilt binaries for Linux, macOS and Windows are attached to every
[release](https://github.com/Shidori5417/proxyforge/releases).

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

Docker:

```bash
docker build -t proxyforge https://github.com/Shidori5417/proxyforge.git
docker run --rm -i proxyforge --input - --quiet < proxies.txt
```

Shell completions:

```bash
proxyforge completions bash      # also zsh, fish, elvish, powershell
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

# Pull lists straight from the web, keep only Turkish exits, save them
proxyforge -i https://example.com/proxies.txt --geo-db GeoLite2-Country.mmdb \
  --geo-db GeoLite2-ASN.mmdb --country tr --save tr.txt

# Check a list with no schemes, then serve the survivors as a local rotator
proxyforge -i plain.txt --detect --serve 127.0.0.1:8080 --rotate-on-error
curl -x http://127.0.0.1:8080 https://httpbin.org/get
```

## Input formats

One endpoint per line in a file or URL, or one argument per endpoint. Sources given as
URLs are scraped with a forgiving pattern, so raw HTML and JSON work too:

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
| `-i, --input <FILE_OR_URL>` | Read endpoints from a file, a URL or `-` (stdin); repeatable |
| `-j, --judge <URL>` | Judge URL requested through every proxy, repeatable |
| `-c, --concurrency <N>` | Proxies checked in parallel (default `100`) |
| `-t, --timeout <SECS>` | Per-proxy timeout (default `8`) |
| `-r, --retries <N>` | Extra attempts after a failure (default `0`) |
| `-f, --format <FORMAT>` | `table`, `json`, `jsonl` or `csv` (default `table`) |
| `-o, --output <FILE>` | Write the report to a file |
| `--alive-only` | Report only proxies that responded |
| `--max-latency <MS>` | Drop proxies slower than this |
| `--min-anonymity <LEVEL>` | `transparent`, `anonymous` or `elite` |
| `--expect <TEXT>` | Mark a proxy dead unless the judge response contains this text |
| `--detect` | Probe schemesless endpoints for HTTP, SOCKS5 and SOCKS4 |
| `--protocols <LIST>` | Only check these protocols, comma separated |
| `--sort <ORDER>` | `latency` (default), `proxy` or `status` |
| `--save <FILE>` | Write only the alive proxies to a file after the run |
| `--save-append` | Append to the save file instead of overwriting |
| `--save-format <FMT>` | `endpoints` (default), `json` or `csv` |
| `--geo-db <FILE>` | Offline GeoIP database in MaxMind mmdb format, repeatable |
| `--country <CODES>` | Only report proxies exiting in these countries |
| `--exclude-country <CODES>` | Drop proxies exiting in these countries |
| `--no-anonymity` | Skip anonymity detection (saves one request) |
| `--local-ip-url <URL>` | URL used once to learn the local public IP |
| `--show-credentials` | Print credentials instead of `***:***` |
| `--tui` | Open the interactive dashboard |
| `-q, --quiet` | Print only alive endpoints, no table or summary |
| `--no-summary` | Hide the summary block |
| `--no-progress` | Hide the progress bar |
| `--no-color` | Disable ANSI colours |
| `--serve <BIND>` | Serve the alive pool as a local rotating proxy |
| `--rotate <N>` | Rotate every N requests (default `1`) |
| `--rotate-random` | Pick pool members randomly |
| `--rotate-on-error` | Retry with the next pool member on failure |

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

![proxyforge checking a mixed batch of HTTP, SOCKS4 and SOCKS5 proxies](docs/demo.png)

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

## Saving the survivors

`--save` writes only the proxies that answered, with their detected scheme and
credentials included, so the file can be fed straight back in:

```bash
proxyforge -i proxies.txt --detect --save alive.txt
proxyforge -i alive.txt --alive-only --country tr --save-append tr.txt
```

`--save-format json` (or `csv`) keeps every field instead. Interrupting with
`Ctrl-C` still saves everything checked so far, and in the dashboard the `e`
key writes to the same file.

## Interactive dashboard

`--tui` opens the live dashboard shown at the top of this page:

| Key | Action |
| --- | --- |
| `q` / `Esc` | Quit (the first `Esc` closes the help overlay) |
| `↑` `↓` / `j` `k` | Move the cursor |
| `PgUp` / `PgDn` | Move one page |
| `g` / `G` | Jump to the first / last proxy |
| `f` | Cycle the filter: all → alive → dead |
| `s` | Cycle the sort order: latency → proxy → status |
| `e` | Export the visible alive endpoints to the `--save` file |
| `r` | Re-check the selected proxy |
| `R` | Re-check every dead endpoint |
| `S` | Open the settings panel: judge, timeout, concurrency, retries, detection |
| wheel / click | Scroll the results / select a row |
| `?` | Show or hide the help overlay |

Inside the settings panel, `↑↓` pick a field, `◀▶` or digits edit it, `Enter`
restarts the whole batch with the new values and `Esc` cancels. Invalid input
is reported in the footer and changes nothing.

The details pane follows the cursor and shows the full error, the leaked
forwarding headers and the exit IP. Quitting cancels the remaining checks and
restores the terminal. Report options such as `--format` and `--output` only
apply to the non-interactive mode.

## GeoIP enrichment

`--geo-db` loads MaxMind format (`.mmdb`) databases and enriches every exit IP
with its country, ISO code, ASN and organisation. GeoLite2 Country and ASN work
out of the box; free alternatives such as the DB-IP Lite databases need no
registration:

```bash
curl -O https://download.db-ip.com/free/dbip-country-lite-2026-11.mmdb.gz
curl -O https://download.db-ip.com/free/dbip-asn-lite-2026-11.mmdb.gz
gunzip dbip-*.mmdb.gz

proxyforge -i proxies.txt --geo-db dbip-country-lite-2026-11.mmdb \
  --geo-db dbip-asn-lite-2026-11.mmdb
```

Reports gain a `Geo` column (`DE · AS3320`) and JSON/CSV gain `country`,
`country_code`, `asn` and `as_org` fields. `--country TR,DE` and
`--exclude-country CN` filter by ISO code; without a database loaded those
filters keep nothing alive.

## Rotating proxy server

`--serve` turns the survivors of a check into a local HTTP proxy that forwards
every request or `CONNECT` tunnel through the pool:

```bash
proxyforge -i proxies.txt --alive-only --serve 127.0.0.1:8080 \
  --rotate 5 --rotate-on-error

curl -x http://127.0.0.1:8080 https://httpbin.org/get
```

Rotation walks the pool in order every `--rotate N` requests, or picks randomly
(a fresh random member every N requests) with `--rotate-random`. With
`--rotate-on-error` a failing pool member is skipped and the next one retries
the request. `Ctrl-C` stops the server.

What counts as one unit for `--rotate`:

- **Plain HTTP requests** are counted per request. Each one is forwarded with
  `Connection: close` (the client's `Connection`, `Keep-Alive` and
  `Proxy-Connection` headers are dropped), the response is streamed back
  respecting `Content-Length` / chunked framing, and the client connection is
  then closed. Keep-alive clients just reconnect, so every request gets its own
  pool pick.
- **`CONNECT` tunnels** are opaque byte streams and are counted per tunnel: all
  traffic inside one tunnel (for example every request of an HTTPS session)
  uses the same pool member.

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
├── checker.rs    # worker pool, retries, latency, protocol detection
├── anonymity.rs  # judge payload analysis
├── geo.rs        # offline GeoIP lookups
├── serve.rs      # rotating local proxy server
├── output.rs     # filtering, sorting, table/JSON/CSV
├── tui/          # dashboard state, rendering and event loop
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
