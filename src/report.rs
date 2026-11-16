//! Types describing the outcome of checking a single proxy.

use std::time::Duration;

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
    pub error: Option<String>,
}

impl CheckResult {
    pub fn alive(proxy: Proxy, latency: Duration, status_code: Option<u16>) -> Self {
        Self {
            proxy,
            alive: true,
            latency,
            status_code,
            error: None,
        }
    }

    pub fn dead(proxy: Proxy, latency: Duration, error: impl Into<String>) -> Self {
        Self {
            proxy,
            alive: false,
            latency,
            status_code: None,
            error: Some(error.into()),
        }
    }

    pub fn latency_ms(&self) -> u128 {
        self.latency.as_millis()
    }
}
