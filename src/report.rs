//! Types describing the outcome of checking a single proxy.

use std::time::Duration;

use crate::anonymity::AnonymityLevel;
use crate::geo::GeoInfo;
use crate::proxy::Proxy;

/// Result of probing one proxy endpoint.
#[derive(Debug, Clone)]
pub struct CheckResult {
    pub proxy: Proxy,
    pub alive: bool,
    /// Wall-clock time spent on the check, including retries.
    pub latency: Duration,
    /// HTTP status returned by the judge, when a response was received.
    pub status_code: Option<u16>,
    /// Anonymity level, when the judge echoed enough information.
    pub anonymity: Option<AnonymityLevel>,
    /// Public IP the judge observed for this proxy.
    pub exit_ip: Option<String>,
    /// Forwarding headers the judge could see.
    pub leaks: Vec<String>,
    /// Country and ASN of the exit IP, when a GeoIP database was given.
    pub geo: Option<GeoInfo>,
    pub error: Option<String>,
}

impl CheckResult {
    pub fn alive(proxy: Proxy, latency: Duration, status_code: Option<u16>) -> Self {
        Self {
            proxy,
            alive: true,
            latency,
            status_code,
            anonymity: None,
            exit_ip: None,
            leaks: Vec::new(),
            geo: None,
            error: None,
        }
    }

    pub fn dead(proxy: Proxy, latency: Duration, error: impl Into<String>) -> Self {
        Self {
            proxy,
            alive: false,
            latency,
            status_code: None,
            anonymity: None,
            exit_ip: None,
            leaks: Vec::new(),
            geo: None,
            error: Some(error.into()),
        }
    }

    pub fn latency_ms(&self) -> u128 {
        self.latency.as_millis()
    }
}
