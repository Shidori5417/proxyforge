//! Proxy endpoint parsing and representation.
//!
//! Accepted input formats (whitespace around the entry is ignored):
//!
//! ```text
//! host:port
//! scheme://host:port
//! user:pass@host:port
//! scheme://user:pass@host:port
//! ```
//!
//! Supported schemes are `http`, `https`, `socks4`, `socks4a`, `socks5`,
//! `socks5h` and the `socks` alias for `socks5`. When the scheme is omitted
//! the endpoint is treated as an HTTP proxy.

use std::fmt;

use clap::ValueEnum;
use thiserror::Error;

/// Port assumed for `http://` proxies that do not declare one.
pub const DEFAULT_HTTP_PORT: u16 = 8080;
/// Port assumed for `https://` proxies that do not declare one.
pub const DEFAULT_HTTPS_PORT: u16 = 443;
/// Port assumed for SOCKS proxies that do not declare one.
pub const DEFAULT_SOCKS_PORT: u16 = 1080;

/// Transport protocol spoken by a proxy endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, ValueEnum)]
pub enum Protocol {
    Http,
    Https,
    Socks4,
    Socks5,
    /// SOCKS5 with local DNS resolution.
    Socks5h,
}

impl Protocol {
    pub const fn as_str(self) -> &'static str {
        match self {
            Protocol::Http => "http",
            Protocol::Https => "https",
            Protocol::Socks4 => "socks4",
            Protocol::Socks5 => "socks5",
            Protocol::Socks5h => "socks5h",
        }
    }

    /// Port used when the endpoint does not specify one.
    pub const fn default_port(self) -> u16 {
        match self {
            Protocol::Http => DEFAULT_HTTP_PORT,
            Protocol::Https => DEFAULT_HTTPS_PORT,
            Protocol::Socks4 | Protocol::Socks5 | Protocol::Socks5h => DEFAULT_SOCKS_PORT,
        }
    }

    pub const fn is_socks(self) -> bool {
        matches!(
            self,
            Protocol::Socks4 | Protocol::Socks5 | Protocol::Socks5h
        )
    }

    /// Whether a TLS handshake with the proxy itself is required.
    pub const fn is_tls(self) -> bool {
        matches!(self, Protocol::Https)
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single proxy endpoint, optionally carrying credentials.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Proxy {
    pub protocol: Protocol,
    /// Hostname or IP literal. IPv6 addresses are stored without brackets.
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    /// True when the input carried no scheme and the protocol was assumed.
    pub inferred: bool,
}

impl Proxy {
    pub fn new(protocol: Protocol, host: impl Into<String>, port: u16) -> Self {
        Self {
            protocol,
            host: host.into(),
            port,
            username: None,
            password: None,
            inferred: false,
        }
    }

    /// Returns a copy speaking a different protocol.
    pub fn with_protocol(mut self, protocol: Protocol) -> Self {
        self.protocol = protocol;
        self.inferred = false;
        self
    }

    pub fn with_credentials(
        mut self,
        username: impl Into<String>,
        password: Option<String>,
    ) -> Self {
        self.username = Some(username.into());
        self.password = password;
        self
    }

    pub fn has_credentials(&self) -> bool {
        self.username.is_some()
    }

    /// `host:port`, with brackets around IPv6 literals.
    pub fn authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// Full endpoint including credentials, safe for internal use only.
    pub fn to_url(&self) -> String {
        match (&self.username, &self.password) {
            (Some(user), Some(pass)) => {
                format!("{}://{}:{}@{}", self.protocol, user, pass, self.authority())
            }
            (Some(user), None) => {
                format!("{}://{}@{}", self.protocol, user, self.authority())
            }
            _ => format!("{}://{}", self.protocol, self.authority()),
        }
    }

    /// Printable endpoint that never leaks the credentials.
    pub fn redacted(&self) -> String {
        if self.has_credentials() {
            format!("{}://***:***@{}", self.protocol, self.authority())
        } else {
            format!("{}://{}", self.protocol, self.authority())
        }
    }

    /// Identity used for de-duplicating the input list.
    pub fn key(&self) -> String {
        let creds = match (&self.username, &self.password) {
            (Some(user), Some(pass)) => format!("{user}:{pass}@"),
            (Some(user), None) => format!("{user}@"),
            _ => String::new(),
        };
        format!("{}://{creds}{}", self.protocol, self.authority())
    }
}

impl fmt::Display for Proxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted())
    }
}

/// Reasons an entry in the proxy list could not be parsed.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseProxyError {
    #[error("empty proxy entry")]
    Empty,
    #[error("unsupported proxy scheme `{0}`")]
    UnsupportedScheme(String),
    #[error("missing proxy host")]
    MissingHost,
    #[error("invalid proxy port `{0}`")]
    InvalidPort(String),
    #[error("proxy host contains invalid characters")]
    InvalidHost,
    #[error("proxy credentials are incomplete")]
    InvalidCredentials,
}

impl std::str::FromStr for Proxy {
    type Err = ParseProxyError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let cleaned = raw.trim().trim_end_matches([',', ';', ' ']);
        if cleaned.is_empty() {
            return Err(ParseProxyError::Empty);
        }

        // Split an optional scheme prefix.
        let (protocol, rest, inferred) = match cleaned.split_once("://") {
            Some((scheme, rest)) => (parse_scheme(scheme)?, rest, false),
            None => (Protocol::Http, cleaned, true),
        };

        // Drop any URL path, query or fragment pasted by accident.
        let rest = rest
            .split(['/', '?', '#'])
            .next()
            .unwrap_or(rest)
            .trim_end_matches('/');

        // Credentials are separated by the last `@`, so passwords may contain one.
        let (credentials, authority) = match rest.rsplit_once('@') {
            Some((creds, host)) => (Some(creds), host),
            None => (None, rest),
        };

        let (host, port) = split_authority(authority, protocol)?;
        if host.is_empty() {
            return Err(ParseProxyError::MissingHost);
        }
        if !is_valid_host(&host) {
            return Err(ParseProxyError::InvalidHost);
        }

        let mut proxy = Proxy::new(protocol, host, port);
        proxy.inferred = inferred;
        if let Some(creds) = credentials {
            if creds.is_empty() {
                return Err(ParseProxyError::InvalidCredentials);
            }
            let (user, pass) = match creds.split_once(':') {
                Some((user, pass)) => (user, Some(pass)),
                None => (creds, None),
            };
            let user = percent_decode(user);
            if user.is_empty() {
                return Err(ParseProxyError::InvalidCredentials);
            }
            proxy.username = Some(user);
            proxy.password = pass.map(percent_decode);
        }

        Ok(proxy)
    }
}

fn parse_scheme(scheme: &str) -> Result<Protocol, ParseProxyError> {
    let lowered = scheme.trim().to_ascii_lowercase();
    match lowered.as_str() {
        "http" => Ok(Protocol::Http),
        "https" => Ok(Protocol::Https),
        "socks" | "socks5" => Ok(Protocol::Socks5),
        "socks5h" => Ok(Protocol::Socks5h),
        "socks4" | "socks4a" => Ok(Protocol::Socks4),
        other => Err(ParseProxyError::UnsupportedScheme(other.to_string())),
    }
}

/// Splits `host:port`, honouring `[ipv6]:port` syntax.
fn split_authority(authority: &str, protocol: Protocol) -> Result<(String, u16), ParseProxyError> {
    if let Some(inner) = authority.strip_prefix('[') {
        let Some((host, after)) = inner.split_once(']') else {
            return Err(ParseProxyError::InvalidHost);
        };
        let port = match after.strip_prefix(':') {
            Some(port) if !port.is_empty() => parse_port(port)?,
            Some(_) => return Err(ParseProxyError::InvalidPort(String::new())),
            None => protocol.default_port(),
        };
        return Ok((host.to_string(), port));
    }

    match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() => Ok((host.to_string(), parse_port(port)?)),
        Some((host, _)) => Ok((host.to_string(), protocol.default_port())),
        None => Ok((authority.to_string(), protocol.default_port())),
    }
}

fn parse_port(raw: &str) -> Result<u16, ParseProxyError> {
    match raw.trim().parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(ParseProxyError::InvalidPort(raw.to_string())),
    }
}

fn is_valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':'))
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !host.contains("..")
}

/// Decodes `%XX` escapes; malformed sequences are left untouched.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn parse(raw: &str) -> Proxy {
        Proxy::from_str(raw).expect("proxy should parse")
    }

    #[test]
    fn parses_bare_host_and_port() {
        let proxy = parse("203.0.113.7:8080");
        assert_eq!(proxy.protocol, Protocol::Http);
        assert_eq!(proxy.host, "203.0.113.7");
        assert_eq!(proxy.port, 8080);
        assert!(!proxy.has_credentials());
    }

    #[test]
    fn parses_explicit_schemes() {
        for (raw, expected) in [
            ("http://1.2.3.4:80", Protocol::Http),
            ("https://1.2.3.4:443", Protocol::Https),
            ("socks4://1.2.3.4:1080", Protocol::Socks4),
            ("socks4a://1.2.3.4:1080", Protocol::Socks4),
            ("socks5://1.2.3.4:1080", Protocol::Socks5),
            ("socks://1.2.3.4:1080", Protocol::Socks5),
            ("socks5h://1.2.3.4:1080", Protocol::Socks5h),
        ] {
            assert_eq!(parse(raw).protocol, expected, "input: {raw}");
        }
    }

    #[test]
    fn scheme_is_case_insensitive() {
        assert_eq!(parse("SOCKS5://1.2.3.4:1080").protocol, Protocol::Socks5);
    }

    #[test]
    fn applies_default_ports() {
        assert_eq!(parse("http://example.com").port, DEFAULT_HTTP_PORT);
        assert_eq!(parse("https://example.com").port, DEFAULT_HTTPS_PORT);
        assert_eq!(parse("socks5://example.com").port, DEFAULT_SOCKS_PORT);
    }

    #[test]
    fn parses_credentials() {
        let proxy = parse("socks5://alice:s3cret@10.0.0.1:1080");
        assert_eq!(proxy.username.as_deref(), Some("alice"));
        assert_eq!(proxy.password.as_deref(), Some("s3cret"));
        assert!(proxy.has_credentials());
    }

    #[test]
    fn password_may_contain_at_and_colon() {
        let proxy = parse("http://bob:p@ss:w0rd@10.0.0.1:3128");
        assert_eq!(proxy.username.as_deref(), Some("bob"));
        assert_eq!(proxy.password.as_deref(), Some("p@ss:w0rd"));
    }

    #[test]
    fn username_without_password() {
        let proxy = parse("http://bob@10.0.0.1:3128");
        assert_eq!(proxy.username.as_deref(), Some("bob"));
        assert_eq!(proxy.password, None);
    }

    #[test]
    fn decodes_percent_escapes_in_credentials() {
        let proxy = parse("http://bob%40example.com:p%40ss@10.0.0.1:3128");
        assert_eq!(proxy.username.as_deref(), Some("bob@example.com"));
        assert_eq!(proxy.password.as_deref(), Some("p@ss"));
    }

    #[test]
    fn parses_ipv6_literals() {
        let proxy = parse("[2001:db8::1]:8080");
        assert_eq!(proxy.host, "2001:db8::1");
        assert_eq!(proxy.port, 8080);
        assert_eq!(proxy.authority(), "[2001:db8::1]:8080");

        let with_creds = parse("socks5://u:p@[::1]:1080");
        assert_eq!(with_creds.host, "::1");
        assert_eq!(with_creds.port, 1080);
        assert_eq!(with_creds.username.as_deref(), Some("u"));
    }

    #[test]
    fn trims_noise_around_entries() {
        let proxy = parse("  1.2.3.4:8080,  ");
        assert_eq!(proxy.host, "1.2.3.4");
        assert_eq!(proxy.port, 8080);
    }

    #[test]
    fn drops_url_path() {
        let proxy = parse("http://1.2.3.4:8080/some/path?q=1");
        assert_eq!(proxy.host, "1.2.3.4");
        assert_eq!(proxy.port, 8080);
    }

    #[test]
    fn bare_endpoints_are_marked_as_inferred() {
        assert!(parse("1.2.3.4:8080").inferred);
        assert!(!parse("http://1.2.3.4:8080").inferred);
        assert!(!parse("socks5://1.2.3.4:1080").inferred);
    }

    #[test]
    fn with_protocol_switches_and_marks_as_explicit() {
        let proxy = parse("1.2.3.4:8080").with_protocol(Protocol::Socks5);
        assert_eq!(proxy.protocol, Protocol::Socks5);
        assert!(!proxy.inferred);
    }

    #[test]
    fn rejects_invalid_input() {
        for raw in ["", "   ", "ftp://1.2.3.4:21", "1.2.3.4:0", "1.2.3.4:99999"] {
            assert!(Proxy::from_str(raw).is_err(), "should reject: {raw:?}");
        }
    }

    #[test]
    fn rejects_hosts_with_invalid_characters() {
        assert!(Proxy::from_str("ht tp://1.2.3.4:8080").is_err());
        assert!(Proxy::from_str("proxy .example.com:8080").is_err());
    }

    #[test]
    fn redacts_credentials() {
        let proxy = parse("socks5://alice:s3cret@10.0.0.1:1080");
        assert_eq!(proxy.redacted(), "socks5://***:***@10.0.0.1:1080");
        assert_eq!(proxy.to_string(), "socks5://***:***@10.0.0.1:1080");
        assert_eq!(proxy.to_url(), "socks5://alice:s3cret@10.0.0.1:1080");
    }

    #[test]
    fn key_includes_credentials_and_scheme() {
        let a = parse("socks5://alice:one@10.0.0.1:1080");
        let b = parse("socks5://alice:two@10.0.0.1:1080");
        let c = parse("socks5h://alice:one@10.0.0.1:1080");
        assert_ne!(a.key(), b.key());
        assert_ne!(a.key(), c.key());
    }
}
