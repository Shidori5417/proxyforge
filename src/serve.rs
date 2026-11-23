//! Local rotating proxy server (`--serve`).
//!
//! After a check run, the alive proxies form a pool and the tool listens as an
//! ordinary HTTP proxy itself: every incoming request or `CONNECT` tunnel is
//! forwarded through the next pool member, either in order or at random.
//!
//! Plain HTTP requests are forwarded with `Connection: close` and the client
//! connection is closed once the response has been relayed, so every request
//! gets its own pool pick (keep-alive clients simply reconnect). `CONNECT`
//! tunnels are opaque byte streams: one tunnel is one pick.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use tokio::io::{AsyncWrite, AsyncWriteExt};
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
    /// Rotate to the next proxy after this many requests (plain HTTP) or
    /// `CONNECT` tunnels.
    pub rotate: usize,
    /// Pick a random pool member instead of going in order.
    pub random: bool,
    /// On connect failure, rotate to another proxy and retry.
    pub rotate_on_error: bool,
}

struct ServeState {
    pool: Vec<Proxy>,
    /// Requests and tunnels accepted so far; drives rotation.
    counter: AtomicUsize,
    /// Pool index currently in use in random mode (re-drawn every `rotate`).
    random_base: AtomicUsize,
    config: ServeConfig,
    timeout: Duration,
}

impl ServeState {
    /// Registers a request or tunnel and returns the pool index it starts
    /// from. Sequential mode moves one step every `rotate` requests; random
    /// mode draws a fresh random member every `rotate` requests.
    fn next_base(&self) -> usize {
        let served = self.counter.fetch_add(1, Ordering::Relaxed);
        self.base_for(served)
    }

    fn base_for(&self, served: usize) -> usize {
        let length = self.pool.len().max(1);
        let rotate = self.config.rotate.max(1);
        if self.config.random {
            if served % rotate == 0 {
                self.random_base
                    .store(fastrand::usize(..length), Ordering::Relaxed);
            }
            self.random_base.load(Ordering::Relaxed) % length
        } else {
            (served / rotate) % length
        }
    }

    /// The pool member for retry `attempt` of a request starting at `base`.
    fn pick_at(&self, base: usize, attempt: usize) -> &Proxy {
        &self.pool[(base + attempt) % self.pool.len().max(1)]
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
        random_base: AtomicUsize::new(0),
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
        let base = state.next_base();
        let mut upstream =
            connect_upstream(state, base, &host, port, Tunnel::Yes, "", &[], &[]).await?;
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
    let base = state.next_base();
    let upstream = connect_upstream(
        state,
        base,
        &target.host,
        target.port,
        Tunnel::No,
        &target.path,
        &head,
        &leftover,
    )
    .await?;
    relay_http(client, upstream, method.eq_ignore_ascii_case("HEAD")).await
}

/// Whether the client asked for a raw tunnel (`CONNECT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tunnel {
    Yes,
    No,
}

/// Opens a connection to the target through a pool member, honouring the
/// rotation and retry policy.
#[allow(clippy::too_many_arguments)]
async fn connect_upstream(
    state: &ServeState,
    base: usize,
    host: &str,
    port: u16,
    tunnel: Tunnel,
    path: &str,
    head: &[u8],
    leftover: &[u8],
) -> Result<BoxStream> {
    let mut last_error = anyhow::anyhow!("no proxy in the pool");

    for attempt in 0..state.pool.len() {
        let proxy = state.pick_at(base, attempt);
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
            // One request per upstream connection; the client reconnects.
            out.push("Connection: close".to_string());
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
                || name.eq_ignore_ascii_case("connection")
                || name.eq_ignore_ascii_case("keep-alive")
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

/// Relays one plain HTTP exchange and then closes the client connection.
///
/// The response is streamed back with its own framing respected and rewritten
/// to `Connection: close`, so the client opens a new connection (and thus
/// gets a fresh pool pick) for its next request. Request body bytes that
/// follow the head keep flowing upstream while the response is relayed.
async fn relay_http(client: TcpStream, upstream: BoxStream, request_was_head: bool) -> Result<()> {
    let (mut client_read, mut client_write) = client.into_split();
    let (mut upstream_read, mut upstream_write) = tokio::io::split(upstream);

    let upload = async {
        let _ = tokio::io::copy(&mut client_read, &mut upstream_write).await;
        // The request is complete (or the client left); only the response
        // decides when the exchange ends.
        std::future::pending::<()>().await;
    };

    let mut started = false;
    let response = transport::relay_response(
        &mut upstream_read,
        &mut client_write,
        request_was_head,
        close_response_head,
        &mut started,
    );

    let result = tokio::select! {
        result = response => result,
        () = upload => unreachable!("the upload future never completes"),
    };

    if let Err(error) = &result
        && !started
    {
        let _ = write_bad_gateway(&mut client_write).await;
        return Err(anyhow::anyhow!("invalid upstream response: {error}"));
    }
    let _ = client_write.shutdown().await;
    result.context("cannot relay the upstream response")
}

async fn write_bad_gateway<W: AsyncWrite + Unpin>(client: &mut W) -> std::io::Result<()> {
    client
        .write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
        .await
}

/// Replaces the response's connection management headers with
/// `Connection: close`.
fn close_response_head(head: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    let block = text.strip_suffix("\r\n\r\n").unwrap_or(&text);
    let mut out: Vec<&str> = Vec::new();
    for (index, line) in block.split("\r\n").enumerate() {
        let name = line.split(':').next().unwrap_or_default().trim();
        if index > 0
            && (name.eq_ignore_ascii_case("connection")
                || name.eq_ignore_ascii_case("keep-alive")
                || name.eq_ignore_ascii_case("proxy-connection"))
        {
            continue;
        }
        out.push(line);
    }
    out.push("Connection: close");
    let mut bytes = out.join("\r\n").into_bytes();
    bytes.extend_from_slice(b"\r\n\r\n");
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
    fn forward_prefix_only_adds_connection_close_for_http_proxies() {
        let proxy = Proxy::new(crate::proxy::Protocol::Http, "10.0.0.1", 3128);
        let prefix = forward_prefix(
            &proxy,
            b"GET http://example.com/x HTTP/1.1\r\nHost: example.com\r\n\r\n",
            b"",
            "/x",
        );
        assert_eq!(
            prefix,
            b"GET http://example.com/x HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n"
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
            b"GET http://example.com/x HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\nProxy-Authorization: Basic dXNlcjpwYXNz\r\n\r\nbody"
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
        assert_eq!(
            prefix,
            b"GET /x HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n"
        );
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
        assert_eq!(
            prefix,
            b"GET /x?q=1 HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n"
        );
    }

    fn state(pool_size: u16, rotate: usize, random: bool) -> ServeState {
        ServeState {
            pool: (1..=pool_size)
                .map(|port| Proxy::new(crate::proxy::Protocol::Http, "10.0.0.1", port))
                .collect(),
            counter: AtomicUsize::new(0),
            random_base: AtomicUsize::new(0),
            config: ServeConfig {
                bind: "127.0.0.1:0".parse().unwrap(),
                rotate,
                random,
                rotate_on_error: true,
            },
            timeout: Duration::from_secs(3),
        }
    }

    #[test]
    fn rotation_walks_the_pool_from_the_first_member() {
        let state = state(2, 1, false);

        // The very first request uses the first pool member.
        let base = state.next_base();
        assert_eq!(state.pick_at(base, 0).port, 1);
        // The retry moves on, wrapping around.
        assert_eq!(state.pick_at(base, 1).port, 2);
        assert_eq!(state.pick_at(base, 2).port, 1);

        let base = state.next_base();
        assert_eq!(state.pick_at(base, 0).port, 2);
        let base = state.next_base();
        assert_eq!(state.pick_at(base, 0).port, 1);
    }

    #[test]
    fn rotation_respects_the_rotate_interval() {
        let state = state(2, 3, false);

        // Requests 0..3 use the first proxy, 3..6 the second, then it wraps.
        let ports: Vec<u16> = (0..7)
            .map(|_| {
                let base = state.next_base();
                state.pick_at(base, 0).port
            })
            .collect();
        assert_eq!(ports, [1, 1, 1, 2, 2, 2, 1]);
    }

    #[test]
    fn random_rotation_reaches_every_member_and_is_not_sequential() {
        let state = state(5, 1, true);

        let ports: Vec<u16> = (0..400)
            .map(|_| {
                let base = state.next_base();
                state.pick_at(base, 0).port
            })
            .collect();

        for port in 1..=5 {
            assert!(ports.contains(&port), "member {port} was never picked");
        }
        // Not a plain 1,2,3,4,5 walk and not the old counter hash either.
        let sequential: Vec<u16> = (0..400).map(|index| (index % 5 + 1) as u16).collect();
        assert_ne!(ports, sequential);
        assert!(ports.windows(2).any(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn random_rotation_holds_a_member_for_the_rotate_interval() {
        let state = state(50, 4, true);

        for _ in 0..25 {
            let ports: Vec<u16> = (0..4)
                .map(|_| {
                    let base = state.next_base();
                    state.pick_at(base, 0).port
                })
                .collect();
            assert!(ports.iter().all(|port| *port == ports[0]), "{ports:?}");
        }
    }

    #[test]
    fn forward_prefix_forces_connection_close_and_strips_hop_by_hop_headers() {
        let proxy = Proxy::new(crate::proxy::Protocol::Http, "10.0.0.1", 3128);
        let prefix = forward_prefix(
            &proxy,
            b"GET http://example.com/x HTTP/1.1\r\nHost: example.com\r\nConnection: keep-alive\r\nkeep-alive: timeout=5\r\nProxy-Connection: keep-alive\r\n\r\n",
            b"",
            "/x",
        );
        assert_eq!(
            prefix,
            b"GET http://example.com/x HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n"
        );
    }

    #[test]
    fn response_head_is_rewritten_to_connection_close() {
        let head = close_response_head(
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n",
        );
        assert_eq!(
            head,
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n"
        );
    }
}
