//! Anonymity classification based on what the judge sees.
//!
//! A proxy is considered:
//!
//! * **transparent** when the client's own public IP leaks through the proxy,
//!   either as the observed origin or inside a forwarding header,
//! * **anonymous** when forwarding headers are present but the client IP is not,
//! * **elite** when no forwarding header or client IP is visible at all.
//!
//! Classification requires a judge that echoes request headers, such as
//! `https://httpbin.org/get`. Judges that only report the exit IP leave the
//! level unknown.

use std::net::IpAddr;

use clap::ValueEnum;
use serde_json::Value;

use crate::transport::Response;

/// Headers that reveal a proxy in the request path.
const LEAK_HEADERS: &[&str] = &[
    "forwarded",
    "via",
    "proxy-connection",
    "proxy-agent",
    "client-ip",
    "x-client-ip",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-proxy-id",
    "x-real-ip",
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum AnonymityLevel {
    /// The client IP is visible to the judge.
    Transparent,
    /// The proxy hides the client IP but announces itself.
    Anonymous,
    /// Neither the client IP nor the proxy is visible.
    Elite,
    /// The judge did not reveal enough information.
    #[default]
    Unknown,
}

impl AnonymityLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            AnonymityLevel::Transparent => "transparent",
            AnonymityLevel::Anonymous => "anonymous",
            AnonymityLevel::Elite => "elite",
            AnonymityLevel::Unknown => "unknown",
        }
    }

    /// Position in the transparent < anonymous < elite ladder.
    /// Unknown levels have no rank and never satisfy a minimum.
    pub const fn rank(self) -> Option<u8> {
        match self {
            AnonymityLevel::Transparent => Some(0),
            AnonymityLevel::Anonymous => Some(1),
            AnonymityLevel::Elite => Some(2),
            AnonymityLevel::Unknown => None,
        }
    }

    /// Whether this level satisfies `minimum`.
    pub fn satisfies(self, minimum: AnonymityLevel) -> bool {
        match (self.rank(), minimum.rank()) {
            (Some(actual), Some(required)) => actual >= required,
            _ => false,
        }
    }
}

impl std::fmt::Display for AnonymityLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the judge told us about a proxied request.
#[derive(Debug, Clone, Default)]
pub struct Anonymity {
    pub level: AnonymityLevel,
    pub exit_ip: Option<String>,
    /// Forwarding headers that were visible to the judge.
    pub leaks: Vec<String>,
}

/// Classifies a judge response. `local_ip` is the machine's own public IP,
/// resolved without a proxy, and is used to detect transparent proxies.
pub fn analyze(response: &Response, local_ip: Option<&str>) -> Anonymity {
    let payload = Payload::parse(&response.text());

    let mut leaks = Vec::new();
    for (name, value) in &payload.headers {
        if LEAK_HEADERS.contains(&name.as_str()) && !value.trim().is_empty() {
            leaks.push(format!("{name}: {value}"));
        }
    }

    let local_leak = local_ip.is_some_and(|local| {
        payload
            .headers
            .iter()
            .any(|(_, value)| contains_ip(value, local))
            || payload.exit_ip.as_deref() == Some(local)
    });

    let level = if !payload.observed_headers {
        // Without echoed headers only the exit IP is available.
        if payload.exit_ip.is_some() && !local_leak {
            AnonymityLevel::Unknown
        } else if local_leak {
            AnonymityLevel::Transparent
        } else {
            AnonymityLevel::Unknown
        }
    } else if local_leak {
        AnonymityLevel::Transparent
    } else if leaks.is_empty() {
        AnonymityLevel::Elite
    } else {
        AnonymityLevel::Anonymous
    };

    Anonymity {
        level,
        exit_ip: payload.exit_ip,
        leaks,
    }
}

/// Extracts an IP from a judge body that is either JSON or plain text.
pub fn exit_ip_from_text(text: &str) -> Option<String> {
    Payload::parse(text).exit_ip
}

/// Whether `text` contains `ip` as a whole token.
fn contains_ip(text: &str, ip: &str) -> bool {
    text.split(|c: char| !(c.is_ascii_hexdigit() || c == '.' || c == ':'))
        .any(|token| token.trim_matches(':') == ip)
}

#[derive(Debug, Default)]
struct Payload {
    exit_ip: Option<String>,
    headers: Vec<(String, String)>,
    /// True when the judge actually echoed request headers.
    observed_headers: bool,
}

impl Payload {
    fn parse(text: &str) -> Self {
        let trimmed = text.trim();
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            return Self {
                exit_ip: plain_ip(trimmed),
                ..Self::default()
            };
        };

        let exit_ip = origin_ip(&value);
        let mut payload = Self {
            exit_ip,
            ..Self::default()
        };

        if let Some(headers) = value.get("headers").and_then(Value::as_object) {
            payload.observed_headers = true;
            for (name, value) in headers {
                if let Some(value) = value.as_str() {
                    payload
                        .headers
                        .push((name.to_ascii_lowercase(), value.to_string()));
                }
            }
        }

        payload
    }
}

fn origin_ip(value: &Value) -> Option<String> {
    for key in ["origin", "ip"] {
        match value.get(key) {
            Some(Value::String(text)) => {
                if let Some(ip) = first_ip(text) {
                    return Some(ip);
                }
            }
            Some(Value::Array(items)) => {
                if let Some(Value::String(text)) = items.first()
                    && let Some(ip) = first_ip(text)
                {
                    return Some(ip);
                }
            }
            _ => {}
        }
    }
    None
}

fn first_ip(text: &str) -> Option<String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .find_map(|token| token.trim().parse::<IpAddr>().ok().map(|ip| ip.to_string()))
}

fn plain_ip(text: &str) -> Option<String> {
    text.lines()
        .next()
        .and_then(|line| line.trim().parse::<IpAddr>().ok())
        .map(|ip| ip.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::Response;

    fn response(body: &str) -> Response {
        Response {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn elite_when_no_leaks_are_visible() {
        let body = r#"{"origin":"203.0.113.9","headers":{"Host":"httpbin.org","User-Agent":"x"}}"#;
        let result = analyze(&response(body), Some("198.51.100.1"));
        assert_eq!(result.level, AnonymityLevel::Elite);
        assert_eq!(result.exit_ip.as_deref(), Some("203.0.113.9"));
        assert!(result.leaks.is_empty());
    }

    #[test]
    fn anonymous_when_forwarding_headers_exist() {
        let body = r#"{"origin":"203.0.113.9","headers":{"X-Forwarded-For":"203.0.113.9"}}"#;
        let result = analyze(&response(body), Some("198.51.100.1"));
        assert_eq!(result.level, AnonymityLevel::Anonymous);
        assert_eq!(result.leaks.len(), 1);
    }

    #[test]
    fn transparent_when_origin_is_the_client() {
        let body = r#"{"origin":"198.51.100.1","headers":{"Via":"1.1 proxy"}}"#;
        let result = analyze(&response(body), Some("198.51.100.1"));
        assert_eq!(result.level, AnonymityLevel::Transparent);
    }

    #[test]
    fn transparent_when_client_ip_leaks_in_a_header() {
        let body =
            r#"{"origin":"203.0.113.9","headers":{"X-Forwarded-For":"198.51.100.1, 203.0.113.9"}}"#;
        let result = analyze(&response(body), Some("198.51.100.1"));
        assert_eq!(result.level, AnonymityLevel::Transparent);
        assert_eq!(result.leaks.len(), 1);
    }

    #[test]
    fn client_ip_matches_only_whole_tokens() {
        let body = r#"{"origin":"203.0.113.9","headers":{"X-Forwarded-For":"1198.51.100.10"}}"#;
        let result = analyze(&response(body), Some("198.51.100.1"));
        assert_eq!(result.level, AnonymityLevel::Anonymous);
    }

    #[test]
    fn unknown_without_header_echo() {
        let result = analyze(&response("203.0.113.9"), Some("198.51.100.1"));
        assert_eq!(result.level, AnonymityLevel::Unknown);
        assert_eq!(result.exit_ip.as_deref(), Some("203.0.113.9"));
    }

    #[test]
    fn unknown_for_ipify_style_judges() {
        let result = analyze(&response(r#"{"ip":"203.0.113.9"}"#), Some("198.51.100.1"));
        assert_eq!(result.level, AnonymityLevel::Unknown);
        assert_eq!(result.exit_ip.as_deref(), Some("203.0.113.9"));
    }

    #[test]
    fn unknown_for_non_json_bodies() {
        let result = analyze(&response("<html>nope</html>"), None);
        assert_eq!(result.level, AnonymityLevel::Unknown);
        assert_eq!(result.exit_ip, None);
    }

    #[test]
    fn picks_the_first_origin_address() {
        let body = r#"{"origin":"203.0.113.9, 198.51.100.1","headers":{}}"#;
        let result = analyze(&response(body), None);
        assert_eq!(result.exit_ip.as_deref(), Some("203.0.113.9"));
    }

    #[test]
    fn extracts_plain_text_and_json_ips() {
        assert_eq!(
            exit_ip_from_text("203.0.113.9\n").as_deref(),
            Some("203.0.113.9")
        );
        assert_eq!(
            exit_ip_from_text(r#"{"ip":"2001:db8::1"}"#).as_deref(),
            Some("2001:db8::1")
        );
        assert_eq!(exit_ip_from_text("not an ip"), None);
    }

    #[test]
    fn levels_are_ordered() {
        assert!(AnonymityLevel::Elite.satisfies(AnonymityLevel::Anonymous));
        assert!(AnonymityLevel::Anonymous.satisfies(AnonymityLevel::Transparent));
        assert!(!AnonymityLevel::Transparent.satisfies(AnonymityLevel::Anonymous));
        assert!(!AnonymityLevel::Unknown.satisfies(AnonymityLevel::Transparent));
    }
}
