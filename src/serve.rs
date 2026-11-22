//! Local rotating proxy server (`--serve`).
//!
//! After a check run, the alive proxies form a pool and the tool listens as an
//! ordinary HTTP proxy itself: every incoming request or `CONNECT` tunnel is
//! forwarded through the next pool member, either in order or at random.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::proxy::Proxy;
use crate::transport::{self, BoxStream, Target};

/// How long a client may take to send its request head.
const CLIENT_HEAD_TIMEOUT: Duration = Duration::from_secs(15);

/// How the pool is served.
#[derive(Debug, Clone)]
pub struct ServeConfig {
    /// `host:port` to listen on.
    pub bind: SocketAddr,
    /// Rotate to the next proxy after this many requests.
    pub rotate: usize,
    /// Pick a random pool member instead of going in order.
    pub random: bool,
    /// On connect failure, rotate to another proxy and retry.
    pub rotate_on_error: bool,
}

struct ServeState {
    pool: Vec<Proxy>,
    /// Requests served so far; drives sequential rotation.
    counter: AtomicUsize,
    config: ServeConfig,
    timeout: Duration,
}

impl ServeState {
    /// The pool member for `attempt` of the current request.
    fn pick_at(&self, attempt: usize) -> &Proxy {
        let served = self.counter.load(Ordering::Relaxed);
        let length = self.pool.len().max(1);
        let index = if self.config.random {
            // Cheap pseudo randomisation without a dependency.
            served.wrapping_add(attempt).wrapping_mul(2654435761) % length
        } else {
            (served / self.config.rotate.max(1) + attempt) % length
        };
        &self.pool[index]
    }
}

/// Binds the address and starts serving. The returned task runs until
/// aborted; callers decide when to stop (Ctrl-C in the CLI, a flag in tests).
pub async fn start(
    pool: Vec<Proxy>,
    config: ServeConfig,
    timeout: Duration,
) -> Result<(SocketAddr, JoinHandle<()>)> {
    if pool.is_empty() {
        bail!("the proxy pool is empty");
    }

    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("cannot bind {}", config.bind))?;
    let address = listener.local_addr()?;

    let state = Arc::new(ServeState {
        pool,
        counter: AtomicUsize::new(0),
        config,
        timeout,
    });

    let task = tokio::spawn(accept_loop(listener, state));

    Ok((address, task))
}

async fn accept_loop(listener: TcpListener, state: Arc<ServeState>) {
    while let Ok((stream, _)) = listener.accept().await {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(error) = handle_client(stream, &state).await {
                let _ = writeln_to_stderr(&error);
            }
        });
    }
}

fn writeln_to_stderr(message: &anyhow::Error) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut stderr = std::io::stderr().lock();
    writeln!(stderr, "proxyforge serve: {message}")
}

async fn handle_client(mut client: TcpStream, state: &ServeState) -> Result<()> {
    state.counter.fetch_add(1, Ordering::Relaxed);

    let mut leftover = Vec::with_capacity(4096);
    // Bound the wait so a silent client cannot hold a task open forever.
    let head = tokio::time::timeout(
        CLIENT_HEAD_TIMEOUT,
        transport::read_head(&mut client, &mut leftover),
    )
    .await
    .context("timed out waiting for the client request")?
    .context("cannot read the client request")?;

    let text = String::from_utf8_lossy(&head);
    let request_line = text.lines().next().unwrap_or_default();
    let mut tokens = request_line.split(' ');
    let method = tokens.next().unwrap_or_default();
    let uri = tokens.next().unwrap_or_default().to_string();

    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = parse_connect_target(&uri)?;
        let mut upstream = connect_upstream(state, &host, port, Tunnel::Yes, "", &[], &[]).await?;
        client
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await?;
        if !leftover.is_empty() {
            upstream
                .write_all(&leftover)
                .await
                .context("cannot forward the tunnel payload upstream")?;
        }
        relay(client, upstream).await;
        return Ok(());
    }

    let target = absolute_target(&uri)?;
    let upstream = connect_upstream(
        state,
        &target.host,
        target.port,
        Tunnel::No,
        &target.path,
        &head,
        &leftover,
    )
    .await?;
    relay(client, upstream).await;
    Ok(())
}

/// Whether the client asked for a raw tunnel (`CONNECT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tunnel {
    Yes,
    No,
}

/// Opens a connection to the target through a pool member, honouring the
/// rotation and retry policy.
async fn connect_upstream(
    state: &ServeState,
    host: &str,
    port: u16,
    tunnel: Tunnel,
    path: &str,
    head: &[u8],
    leftover: &[u8],
) -> Result<BoxStream> {
    let mut last_error = anyhow::anyhow!("no proxy in the pool");

    for attempt in 0..state.pool.len() {
        let proxy = state.pick_at(attempt);
        let future = open_through(proxy, host, port, tunnel);
        match tokio::time::timeout(state.timeout, future).await {
            Ok(Ok(mut stream)) => {
                if tunnel == Tunnel::No {
                    let prefix = forward_prefix(proxy, head, leftover, path);
                    stream
                        .write_all(&prefix)
                        .await
                        .context("cannot forward the request upstream")?;
                }
                return Ok(stream);
            }
            Ok(Err(error)) => {
                last_error = anyhow::anyhow!("{}: {error}", proxy.redacted());
                if !state.config.rotate_on_error {
                    return Err(last_error.context("upstream proxy failed"));
                }
            }
            Err(_) => {
                last_error = anyhow::anyhow!("{}: timed out", proxy.redacted());
                if !state.config.rotate_on_error {
                    return Err(last_error.context("upstream proxy timed out"));
                }
            }
        }
    }

    Err(last_error.context("every proxy in the pool failed"))
}

/// Reaches `host:port` through one proxy, either as a CONNECT tunnel or as a
/// plain forward connection the client request travels over.
async fn open_through(
    proxy: &Proxy,
    host: &str,
    port: u16,
    tunnel: Tunnel,
) -> Result<BoxStream, transport::TransportError> {
    let target = Target {
        tls: false,
        host: host.to_string(),
        port,
        path: "/".into(),
    };

    let tcp = TcpStream::connect((proxy.host.as_str(), proxy.port)).await?;
    let _ = tcp.set_nodelay(true);
    let mut stream: BoxStream = Box::new(tcp);

    if proxy.protocol.is_tls() {
        stream = Box::new(transport::tls_wrap(&proxy.host, stream).await?);
    }

    match tunnel {
        Tunnel::Yes => {
            if proxy.protocol.is_socks() {
                transport::socks_handshake(stream.as_mut(), proxy, &target).await?;
            } else {
                transport::http_connect(stream.as_mut(), proxy, &target).await?;
            }
        }
        Tunnel::No => {
            // Absolute-form requests are forwarded as they are through HTTP
            // proxies; SOCKS needs the handshake first.
            if proxy.protocol.is_socks() {
                transport::socks_handshake(stream.as_mut(), proxy, &target).await?;
            }
        }
    }

    Ok(stream)
}

/// The bytes to write upstream before relaying: the original request with the
/// client's own proxy headers removed, the URI rewritten to origin form for
/// SOCKS pool members, and the pool member's credentials added for HTTP ones.
fn forward_prefix(proxy: &Proxy, head: &[u8], leftover: &[u8], path: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();

    let mut out: Vec<String> = Vec::new();
    if proxy.protocol.is_socks() {
        let mut tokens = request_line.splitn(3, ' ');
        let method = tokens.next().unwrap_or("GET");
        let version = request_line.split(' ').next_back().unwrap_or("HTTP/1.1");
        out.push(format!("{method} {path} {version}"));
    } else {
        out.push(request_line.to_string());
    }

    let mut ended = false;
    for line in lines {
        if !ended && line.is_empty() {
            // End of the header block: inject the pool member's credentials.
            ended = true;
            if !proxy.protocol.is_socks()
                && let Some(user) = &proxy.username
            {
                let password = proxy.password.as_deref().unwrap_or("");
                let token =
                    base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
                out.push(format!("Proxy-Authorization: Basic {token}"));
            }
        } else if !ended {
            let name = line.split(':').next().unwrap_or_default().trim();
            if name.eq_ignore_ascii_case("proxy-authorization")
                || name.eq_ignore_ascii_case("proxy-connection")
            {
                continue;
            }
        }
        out.push(line.to_string());
    }

    let mut bytes = out.join("\r\n").into_bytes();
    bytes.extend_from_slice(leftover);
    bytes
}

async fn relay(client: TcpStream, upstream: BoxStream) {
    let (mut client_read, mut client_write) = client.into_split();
    let (mut upstream_read, mut upstream_write) = tokio::io::split(upstream);

    let to_upstream = tokio::io::copy(&mut client_read, &mut upstream_write);
    let to_client = tokio::io::copy(&mut upstream_read, &mut client_write);

    let _ = tokio::join!(to_upstream, to_client);
}

fn parse_connect_target(uri: &str) -> Result<(String, u16)> {
    let (host, port) = uri
        .rsplit_once(':')
        .with_context(|| format!("malformed CONNECT target `{uri}`"))?;
    let port: u16 = port
        .parse()
        .with_context(|| format!("malformed CONNECT target `{uri}`"))?;
    Ok((
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .to_string(),
        port,
    ))
}

/// Parses an absolute-form request URI into a [`Target`].
fn absolute_target(uri: &str) -> Result<Target> {
    let target = Target::parse(uri).with_context(|| format!("malformed request URI `{uri}`"))?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connect_targets() {
        let (host, port) = parse_connect_target("example.com:443").unwrap();
        assert_eq!((host.as_str(), port), ("example.com", 443));

        let (host, port) = parse_connect_target("[2001:db8::1]:8443").unwrap();
        assert_eq!((host.as_str(), port), ("2001:db8::1", 8443));
    }

    #[test]
    fn rejects_malformed_connect_targets() {
        assert!(parse_connect_target("no-port").is_err());
        assert!(parse_connect_target("host:notaport").is_err());
    }

    #[test]
    fn parses_absolute_uris() {
        let target = absolute_target("http://example.com/path?q=1").unwrap();
        assert_eq!(target.host, "example.com");
        assert_eq!(target.port, 80);
        assert_eq!(target.path, "/path?q=1");
    }

    #[test]
    fn forward_prefix_is_untouched_for_http_proxies() {
        let proxy = Proxy::new(crate::proxy::Protocol::Http, "10.0.0.1", 3128);
        let prefix = forward_prefix(
            &proxy,
            b"GET http://example.com/x HTTP/1.1\r\nHost: example.com\r\n\r\n",
            b"",
            "/x",
        );
        assert_eq!(
            prefix,
            b"GET http://example.com/x HTTP/1.1\r\nHost: example.com\r\n\r\n"
        );
    }

    #[test]
    fn forward_prefix_injects_pool_credentials_and_strips_client_proxy_headers() {
        let proxy = Proxy::new(crate::proxy::Protocol::Http, "10.0.0.1", 3128)
            .with_credentials("user", Some("pass".to_string()));
        let prefix = forward_prefix(
            &proxy,
            b"GET http://example.com/x HTTP/1.1\r\nHost: example.com\r\nproxy-authorization: Basic Y2xpZW50\r\nProxy-Connection: keep-alive\r\n\r\n",
            b"body",
            "/x",
        );
        assert_eq!(
            prefix,
            b"GET http://example.com/x HTTP/1.1\r\nHost: example.com\r\nProxy-Authorization: Basic dXNlcjpwYXNz\r\n\r\nbody"
        );
    }

    #[test]
    fn forward_prefix_strips_client_proxy_headers_without_credentials() {
        let proxy = Proxy::new(crate::proxy::Protocol::Socks5, "10.0.0.1", 1080);
        let prefix = forward_prefix(
            &proxy,
            b"GET http://example.com/x HTTP/1.1\r\nHost: example.com\r\nProxy-Authorization: Basic Y2xpZW50\r\n\r\n",
            b"",
            "/x",
        );
        assert_eq!(prefix, b"GET /x HTTP/1.1\r\nHost: example.com\r\n\r\n");
    }

    #[test]
    fn forward_prefix_is_rewritten_for_socks_proxies() {
        let proxy = Proxy::new(crate::proxy::Protocol::Socks5, "10.0.0.1", 1080);
        let prefix = forward_prefix(
            &proxy,
            b"GET http://example.com/x?q=1 HTTP/1.1\r\nHost: example.com\r\n\r\n",
            b"",
            "/x?q=1",
        );
        assert_eq!(prefix, b"GET /x?q=1 HTTP/1.1\r\nHost: example.com\r\n\r\n");
    }

    #[test]
    fn rotation_walks_the_pool() {
        let state = ServeState {
            pool: vec![
                Proxy::new(crate::proxy::Protocol::Http, "10.0.0.1", 1),
                Proxy::new(crate::proxy::Protocol::Http, "10.0.0.2", 2),
            ],
            counter: AtomicUsize::new(1),
            config: ServeConfig {
                bind: "127.0.0.1:0".parse().unwrap(),
                rotate: 1,
                random: false,
                rotate_on_error: true,
            },
            timeout: Duration::from_secs(3),
        };

        // Request 1 with rotate=1 is the second one: pool index 1.
        assert_eq!(state.pick_at(0).port, 2);
        // The retry wraps back to the first pool member.
        assert_eq!(state.pick_at(1).port, 1);

        state.counter.store(2, Ordering::Relaxed);
        assert_eq!(state.pick_at(0).port, 1);
    }

    #[test]
    fn rotation_respects_the_rotate_interval() {
        let state = ServeState {
            pool: vec![
                Proxy::new(crate::proxy::Protocol::Http, "10.0.0.1", 1),
                Proxy::new(crate::proxy::Protocol::Http, "10.0.0.2", 2),
            ],
            counter: AtomicUsize::new(2),
            config: ServeConfig {
                bind: "127.0.0.1:0".parse().unwrap(),
                rotate: 3,
                random: false,
                rotate_on_error: false,
            },
            timeout: Duration::from_secs(3),
        };

        // Requests 0..3 use the first proxy, 3..6 the second.
        assert_eq!(state.pick_at(0).port, 1);
    }
}
