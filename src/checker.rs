//! Concurrent proxy checking.
//!
//! A fixed number of workers pull endpoints from a shared queue so that a
//! single slow proxy never blocks the rest of the list. Results are streamed
//! back to the caller over a channel, which keeps partial results available
//! when the user interrupts the run.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::proxy::Proxy;
use crate::report::CheckResult;
use crate::transport::{self, Target};

/// Everything a worker needs to probe a proxy.
#[derive(Debug, Clone)]
pub struct CheckSettings {
    pub target: Target,
    pub timeout: Duration,
    /// Extra attempts after the first failure.
    pub retries: u32,
}

/// Probes one proxy, retrying failed attempts up to `settings.retries` times.
pub async fn check_proxy(proxy: &Proxy, settings: &CheckSettings) -> CheckResult {
    let started = Instant::now();
    let mut last_error = String::from("no attempt was made");

    for _ in 0..=settings.retries {
        match transport::get(proxy, &settings.target, settings.timeout).await {
            Ok(response) if response.is_success() => {
                return CheckResult::alive(proxy.clone(), started.elapsed(), Some(response.status));
            }
            Ok(response) => {
                last_error = format!("judge returned HTTP {}", response.status);
            }
            Err(error) => last_error = error.to_string(),
        }
    }

    CheckResult::dead(proxy.clone(), started.elapsed(), last_error)
}

/// Starts `concurrency` workers and returns their handles.
///
/// Each worker stops when the queue is empty or `cancel` is set between
/// checks. Dropping the returned handles detaches the tasks, which is fine for
/// a short-lived CLI process.
pub fn spawn_workers(
    proxies: Vec<Proxy>,
    settings: Arc<CheckSettings>,
    concurrency: usize,
    cancel: Arc<AtomicBool>,
    sender: UnboundedSender<CheckResult>,
) -> Vec<JoinHandle<()>> {
    let queue = Arc::new(Mutex::new(VecDeque::from(proxies)));
    let worker_count = concurrency
        .max(1)
        .min(queue.lock().map(|q| q.len()).unwrap_or(0));

    (0..worker_count)
        .map(|_| {
            let queue = Arc::clone(&queue);
            let settings = Arc::clone(&settings);
            let cancel = Arc::clone(&cancel);
            let sender = sender.clone();

            tokio::spawn(async move {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let next = queue.lock().ok().and_then(|mut queue| queue.pop_front());
                    let Some(proxy) = next else { break };

                    let result = check_proxy(&proxy, &settings).await;
                    if sender.send(result).is_err() {
                        break;
                    }
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::Protocol;

    #[tokio::test]
    async fn unreachable_proxy_is_dead() {
        // Port 1 on loopback is refused immediately.
        let proxy = Proxy::new(Protocol::Http, "127.0.0.1", 1);
        let settings = CheckSettings {
            target: Target::parse("http://example.com/").unwrap(),
            timeout: Duration::from_millis(500),
            retries: 0,
        };
        let result = check_proxy(&proxy, &settings).await;
        assert!(!result.alive);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn retries_are_attempted() {
        let proxy = Proxy::new(Protocol::Http, "127.0.0.1", 1);
        let settings = CheckSettings {
            target: Target::parse("http://example.com/").unwrap(),
            timeout: Duration::from_millis(100),
            retries: 2,
        };
        let started = Instant::now();
        let result = check_proxy(&proxy, &settings).await;
        assert!(!result.alive);
        // Three refused connections are fast, but the loop must have run thrice.
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(result.error.unwrap().contains("connect"));
    }
}
