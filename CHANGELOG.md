# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] - 2026-11-20

### Added

- `--save FILE` writes only the alive proxies after a run, with the detected
  scheme and credentials included; `--save-format` picks endpoints, JSON or
  CSV and `--save-append` extends an existing file. Interrupting with `Ctrl-C`
  still saves what was checked, and the dashboard `e` key writes to the same
  file.
- Settings panel in the dashboard: `S` edits the judge URL, timeout,
  concurrency, retries and protocol detection live; Enter restarts the batch
  with the new values, Escape cancels and invalid input is reported without
  changing anything.
- `R` re-checks every dead endpoint with the current settings.
- Modernised dashboard: rounded borders, status pills, latency on a colour
  scale, country flags next to the Geo column and bracketed key hints.

## [0.2.0] - 2026-11-19

### Added

- `--tui` interactive dashboard: live progress, filters, sorting, cursor
  navigation, `e` export, `r` re-check, mouse support and a help overlay.
- `--serve <BIND>`: after a check, the alive pool is served as a local
  rotating proxy. `--rotate N`, `--rotate-random` and `--rotate-on-error`
  control the rotation.
- `--geo-db <FILE>`: offline GeoIP enrichment (MaxMind mmdb, GeoLite2 Country
  and ASN work out of the box) with `--country` and `--exclude-country`
  filters and a `Geo` column in the reports.
- `--input` now accepts URLs and can be repeated; remote sources are scraped
  out of raw text, HTML and JSON.
- `--detect`: probes schemesless `host:port` entries for HTTP, SOCKS5 and
  SOCKS4 support and reports the protocol that answered.
- `--judge` accepts multiple URLs and rotates attempts through them;
  `--expect TEXT` verifies the judge response.
- `proxyforge completions <SHELL>` prints bash, zsh, fish, etc. completions.

## [0.1.0] - 2026-11-17

### Added

- Concurrent checking of HTTP, HTTPS, SOCKS4/4a and SOCKS5/SOCKS5h proxies
  with username/password support.
- Anonymity classification (transparent, anonymous, elite) from the judge's
  view of the request.
- Table, JSON, JSONL and CSV reports with filters, sorting and statistics.
- `Ctrl-C` handling that still reports partial results.

[Unreleased]: https://github.com/Shidori5417/proxyforge/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/Shidori5417/proxyforge/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Shidori5417/proxyforge/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Shidori5417/proxyforge/releases/tag/v0.1.0
