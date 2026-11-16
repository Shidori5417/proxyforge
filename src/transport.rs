//! Minimal HTTP/1.1 client with proxy tunnelling support.
//!
//! Only the features needed by the checker are implemented: a single `GET`
//! request, optional TLS towards the target, and proxying through HTTP
//! `CONNECT`, SOCKS4a and SOCKS5. Keeping the client small avoids the
//! connection reuse and pooling behaviour of full HTTP clients, which would
//! hide per-proxy failures.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, lookup_host};
use tokio::time::timeout as timeout_fut;
use tokio_rustls::TlsConnector;

use crate::proxy::Proxy;

/// Largest response header block we are willing to buffer.
pub const MAX_HEAD_SIZE: usize = 64 * 1024;
/// Largest response body we are willing to buffer.
pub const MAX_BODY_SIZE: usize = 2 * 1024 * 1024;

const USER_AGENT: &str = concat!("proxyforge/", env!("CARGO_PKG_VERSION"));
const CONNECT_HEAD_LIMIT: usize = 32 * 1024;
const MAX_LINE_SIZE: usize = 8 * 1024;

/// Anything we can speak HTTP over: a plain socket, a TLS stream or a tunnel.
pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + ?Sized> AsyncStream for T {}

type BoxStream = Box<dyn AsyncStream>;

/// Destination requested through the proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    /// Path including the query string, always starting with `/`.
    pub path: String,
}

impl Target {
    /// Parses an absolute `http://` or `https://` URL.
    pub fn parse(raw: &str) -> Result<Self, TransportError> {
        let url = url::Url::parse(raw.trim())
            .map_err(|err| TransportError::InvalidUrl(err.to_string()))?;
        let tls = match url.scheme() {
            "http" => false,
            "https" => true,
            other => {
                return Err(TransportError::InvalidUrl(format!(
                    "unsupported scheme `{other}`, expected http or https"
                )));
            }
        };
        let host = url
            .host_str()
            .ok_or_else(|| TransportError::InvalidUrl("missing host".into()))?
            .to_string();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| TransportError::InvalidUrl("missing port".into()))?;

        let mut path = url.path().to_string();
        if path.is_empty() {
            path.push('/');
        }
        if let Some(query) = url.query() {
            path.push('?');
            path.push_str(query);
        }

        Ok(Self {
            tls,
            host,
            port,
            path,
        })
    }

    pub fn host_header(&self) -> String {
        let default_port = if self.tls { 443 } else { 80 };
        if self.port == default_port {
            self.host.clone()
        } else if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    fn absolute_uri(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        format!("{scheme}://{}{}", self.host_header(), self.path)
    }

    /// `host:port` with the port always present, as required by `CONNECT`.
    fn connect_authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// HTTP response with lower-cased header names.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn is_success(&self) -> bool {
        (200..400).contains(&self.status)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    #[error("cannot connect to proxy: {0}")]
    Connect(String),
    #[error("SOCKS handshake failed: {0}")]
    Socks(String),
    #[error("CONNECT tunnel rejected: {0}")]
    Tunnel(String),
    #[error("TLS handshake failed: {0}")]
    Tls(String),
    #[error("invalid HTTP response: {0}")]
    Protocol(String),
    #[error("network error: {0}")]
    Io(#[from] std::io::Error),
}

/// Sends a `GET` request to `target` through `proxy`.
///
/// The whole exchange, including the proxy handshake and TLS setup, is bounded
/// by `timeout`.
pub async fn get(
    proxy: &Proxy,
    target: &Target,
    timeout: Duration,
) -> Result<Response, TransportError> {
    timeout_fut(timeout, get_inner(Some(proxy), target))
        .await
        .map_err(|_| TransportError::Timeout(timeout))?
}

/// Sends a `GET` request straight to `target`, without a proxy.
pub async fn get_direct(target: &Target, timeout: Duration) -> Result<Response, TransportError> {
    timeout_fut(timeout, get_inner(None, target))
        .await
        .map_err(|_| TransportError::Timeout(timeout))?
}

async fn get_inner(proxy: Option<&Proxy>, target: &Target) -> Result<Response, TransportError> {
    let mut stream = open_stream(proxy, target).await?;
    let request = build_request(proxy, target);
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;
    read_response(stream.as_mut()).await
}

/// Connects to the proxy, performs its handshake and returns a stream that
/// reaches the target.
async fn open_stream(proxy: Option<&Proxy>, target: &Target) -> Result<BoxStream, TransportError> {
    let (host, port) = match proxy {
        Some(proxy) => (proxy.host.as_str(), proxy.port),
        None => (target.host.as_str(), target.port),
    };

    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|err| TransportError::Connect(format!("{host}:{port}: {err}")))?;
    let _ = tcp.set_nodelay(true);
    let mut stream: BoxStream = Box::new(tcp);

    if let Some(proxy) = proxy {
        if proxy.protocol.is_tls() {
            stream = Box::new(tls_wrap(&proxy.host, stream).await?);
        }

        if proxy.protocol.is_socks() {
            socks_handshake(stream.as_mut(), proxy, target).await?;
        } else if target.tls {
            http_connect(stream.as_mut(), proxy, target).await?;
        }
    }

    if target.tls {
        stream = Box::new(tls_wrap(&target.host, stream).await?);
    }

    Ok(stream)
}

fn tls_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            Arc::new(
                ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            )
        })
        .clone()
}

async fn tls_wrap(
    host: &str,
    stream: BoxStream,
) -> Result<tokio_rustls::client::TlsStream<BoxStream>, TransportError> {
    let server_name = ServerName::try_from(host.to_string())
        .map_err(|err| TransportError::Tls(err.to_string()))?;
    let connector = TlsConnector::from(tls_config());
    connector
        .connect(server_name, stream)
        .await
        .map_err(|err| TransportError::Tls(err.to_string()))
}

fn build_request(proxy: Option<&Proxy>, target: &Target) -> String {
    // Plain HTTP targets are requested in absolute form through HTTP proxies;
    // everything else travels inside a CONNECT tunnel or a SOCKS connection.
    let absolute = proxy.is_some_and(|proxy| !proxy.protocol.is_socks()) && !target.tls;
    let uri = if absolute {
        target.absolute_uri()
    } else {
        target.path.clone()
    };

    let mut request = format!(
        "GET {uri} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {USER_AGENT}\r\nAccept: */*\r\nConnection: close\r\n",
        target.host_header()
    );

    if absolute
        && let Some(proxy) = proxy
        && let Some(user) = &proxy.username
    {
        let password = proxy.password.as_deref().unwrap_or("");
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
        request.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
    }

    request.push_str("\r\n");
    request
}

async fn read_response<S: AsyncRead + Unpin + ?Sized>(
    stream: &mut S,
) -> Result<Response, TransportError> {
    let mut buffer = Vec::with_capacity(4096);
    let head = read_head(stream, &mut buffer).await?;
    let (status, headers) = parse_head(&head)?;
    let body = read_body(stream, &mut buffer, &headers).await?;
    Ok(Response {
        status,
        headers,
        body,
    })
}

/// Reads everything up to and including the blank line that ends the headers.
/// Any body bytes that arrived in the same read stay in `buffer`.
async fn read_head<S: AsyncRead + Unpin + ?Sized>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
) -> Result<Vec<u8>, TransportError> {
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(position) = find(buffer, b"\r\n\r\n") {
            let rest = buffer.split_off(position + 4);
            return Ok(std::mem::replace(buffer, rest));
        }
        if buffer.len() > MAX_HEAD_SIZE {
            return Err(TransportError::Protocol(
                "response headers exceed the size limit".into(),
            ));
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(TransportError::Protocol(
                "connection closed before the response headers".into(),
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

fn parse_head(head: &[u8]) -> Result<(u16, Vec<(String, String)>), TransportError> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/") {
        return Err(TransportError::Protocol(format!(
            "malformed status line `{status_line}`"
        )));
    }
    let status = parts
        .next()
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| {
            TransportError::Protocol(format!("malformed status line `{status_line}`"))
        })?;

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }

    Ok((status, headers))
}

async fn read_body<S: AsyncRead + Unpin + ?Sized>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
    headers: &[(String, String)],
) -> Result<Vec<u8>, TransportError> {
    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    };

    if header("transfer-encoding").is_some_and(|value| {
        value
            .split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("chunked"))
    }) {
        return read_chunked(stream, buffer).await;
    }

    if let Some(length) = header("content-length").and_then(|value| value.trim().parse().ok()) {
        if length > MAX_BODY_SIZE {
            return Err(TransportError::Protocol(
                "response body exceeds the size limit".into(),
            ));
        }
        fill(stream, buffer, length).await?;
        buffer.truncate(length);
        return Ok(std::mem::take(buffer));
    }

    read_to_end(stream, buffer).await
}

async fn read_chunked<S: AsyncRead + Unpin + ?Sized>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
) -> Result<Vec<u8>, TransportError> {
    let mut body = Vec::new();
    loop {
        let line = read_line(stream, buffer).await?;
        let size_text = line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| TransportError::Protocol(format!("invalid chunk size `{size_text}`")))?;

        if size == 0 {
            // Consume optional trailers up to the final blank line.
            loop {
                let trailer = read_line(stream, buffer).await?;
                if trailer.is_empty() {
                    break;
                }
            }
            return Ok(body);
        }
        if body.len() + size > MAX_BODY_SIZE {
            return Err(TransportError::Protocol(
                "response body exceeds the size limit".into(),
            ));
        }

        fill(stream, buffer, size + 2).await?;
        body.extend_from_slice(&buffer[..size]);
        if &buffer[size..size + 2] != b"\r\n" {
            return Err(TransportError::Protocol(
                "missing CRLF after chunk data".into(),
            ));
        }
        buffer.drain(..size + 2);
    }
}

async fn read_line<S: AsyncRead + Unpin + ?Sized>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
) -> Result<String, TransportError> {
    loop {
        if let Some(position) = find(buffer, b"\r\n") {
            let line = String::from_utf8_lossy(&buffer[..position]).into_owned();
            buffer.drain(..position + 2);
            return Ok(line);
        }
        if buffer.len() > MAX_LINE_SIZE {
            return Err(TransportError::Protocol("response line too long".into()));
        }
        read_more(stream, buffer).await?;
    }
}

async fn read_to_end<S: AsyncRead + Unpin + ?Sized>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
) -> Result<Vec<u8>, TransportError> {
    let mut chunk = [0u8; 8192];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(std::mem::take(buffer));
        }
        if buffer.len() + read > MAX_BODY_SIZE {
            return Err(TransportError::Protocol(
                "response body exceeds the size limit".into(),
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

/// Makes sure `buffer` holds at least `wanted` bytes.
async fn fill<S: AsyncRead + Unpin + ?Sized>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
    wanted: usize,
) -> Result<(), TransportError> {
    while buffer.len() < wanted {
        read_more(stream, buffer).await?;
    }
    Ok(())
}

async fn read_more<S: AsyncRead + Unpin + ?Sized>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
) -> Result<(), TransportError> {
    let mut chunk = [0u8; 8192];
    let read = stream.read(&mut chunk).await?;
    if read == 0 {
        return Err(TransportError::Protocol(
            "connection closed before the message was complete".into(),
        ));
    }
    buffer.extend_from_slice(&chunk[..read]);
    Ok(())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

async fn http_connect<S: AsyncRead + AsyncWrite + Unpin + ?Sized>(
    stream: &mut S,
    proxy: &Proxy,
    target: &Target,
) -> Result<(), TransportError> {
    let authority = target.connect_authority();
    let mut request = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nUser-Agent: {USER_AGENT}\r\n"
    );
    if let Some(user) = &proxy.username {
        let password = proxy.password.as_deref().unwrap_or("");
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
        request.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
    }
    request.push_str("Proxy-Connection: keep-alive\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte).await?;
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
        if head.len() > CONNECT_HEAD_LIMIT {
            return Err(TransportError::Tunnel(
                "CONNECT response headers too large".into(),
            ));
        }
    }

    let text = String::from_utf8_lossy(&head);
    let status = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok());

    match status {
        Some(200) => Ok(()),
        Some(code) => Err(TransportError::Tunnel(format!("proxy answered {code}"))),
        None => Err(TransportError::Tunnel("malformed CONNECT response".into())),
    }
}

async fn socks_handshake<S: AsyncRead + AsyncWrite + Unpin + ?Sized>(
    stream: &mut S,
    proxy: &Proxy,
    target: &Target,
) -> Result<(), TransportError> {
    if proxy.protocol == crate::proxy::Protocol::Socks4 {
        socks4_handshake(stream, proxy, target).await
    } else {
        socks5_handshake(stream, proxy, target).await
    }
}

async fn socks4_handshake<S: AsyncRead + AsyncWrite + Unpin + ?Sized>(
    stream: &mut S,
    proxy: &Proxy,
    target: &Target,
) -> Result<(), TransportError> {
    let user = proxy.username.as_deref().unwrap_or("");
    let mut request = vec![0x04, 0x01];
    request.extend_from_slice(&target.port.to_be_bytes());

    let ipv4 = target.host.parse::<Ipv4Addr>().ok();
    if let Some(ip) = ipv4 {
        request.extend_from_slice(&ip.octets());
    } else {
        // SOCKS4a: 0.0.0.1 tells the proxy to resolve the hostname itself.
        request.extend_from_slice(&[0, 0, 0, 1]);
    }
    request.extend_from_slice(user.as_bytes());
    request.push(0);
    if ipv4.is_none() {
        request.extend_from_slice(target.host.as_bytes());
        request.push(0);
    }

    stream.write_all(&request).await?;
    stream.flush().await?;

    let mut reply = [0u8; 8];
    stream.read_exact(&mut reply).await?;
    if reply[0] != 0x00 {
        return Err(TransportError::Socks(format!(
            "unexpected SOCKS4 version byte 0x{:02x}",
            reply[0]
        )));
    }
    match reply[1] {
        0x5a => Ok(()),
        0x5b => Err(TransportError::Socks("request rejected or failed".into())),
        0x5c => Err(TransportError::Socks(
            "request failed: identd unreachable".into(),
        )),
        0x5d => Err(TransportError::Socks(
            "request failed: identd mismatch".into(),
        )),
        code => Err(TransportError::Socks(format!(
            "request failed with code 0x{code:02x}"
        ))),
    }
}

async fn socks5_handshake<S: AsyncRead + AsyncWrite + Unpin + ?Sized>(
    stream: &mut S,
    proxy: &Proxy,
    target: &Target,
) -> Result<(), TransportError> {
    let credentials = proxy.username.as_ref().map(|user| {
        (
            user.as_str(),
            proxy.password.as_deref().unwrap_or("").to_string(),
        )
    });

    let methods: &[u8] = if credentials.is_some() {
        &[0x00, 0x02]
    } else {
        &[0x00]
    };
    let mut greeting = vec![0x05, methods.len() as u8];
    greeting.extend_from_slice(methods);
    stream.write_all(&greeting).await?;
    stream.flush().await?;

    let mut choice = [0u8; 2];
    stream.read_exact(&mut choice).await?;
    if choice[0] != 0x05 {
        return Err(TransportError::Socks(format!(
            "unexpected SOCKS5 version byte 0x{:02x}",
            choice[0]
        )));
    }
    match choice[1] {
        0x00 => {}
        0x02 => {
            let Some((user, password)) = credentials else {
                return Err(TransportError::Socks(
                    "proxy requires username/password authentication".into(),
                ));
            };
            if user.len() > 255 || password.len() > 255 {
                return Err(TransportError::Socks(
                    "credentials longer than 255 bytes".into(),
                ));
            }
            let mut auth = vec![0x01, user.len() as u8];
            auth.extend_from_slice(user.as_bytes());
            auth.push(password.len() as u8);
            auth.extend_from_slice(password.as_bytes());
            stream.write_all(&auth).await?;
            stream.flush().await?;

            let mut result = [0u8; 2];
            stream.read_exact(&mut result).await?;
            if result[1] != 0x00 {
                return Err(TransportError::Socks(format!(
                    "authentication rejected with status 0x{:02x}",
                    result[1]
                )));
            }
        }
        0xff => {
            return Err(TransportError::Socks(
                "no acceptable authentication method".into(),
            ));
        }
        method => {
            return Err(TransportError::Socks(format!(
                "unsupported authentication method 0x{method:02x}"
            )));
        }
    }

    // SOCKS5 resolves the hostname locally, SOCKS5h leaves it to the proxy.
    let destination = if proxy.protocol == crate::proxy::Protocol::Socks5 {
        match lookup_host((target.host.as_str(), target.port)).await {
            Ok(mut addresses) => match addresses.next() {
                Some(address) => Destination::Ip(address.ip()),
                None => {
                    return Err(TransportError::Socks(format!(
                        "cannot resolve `{}`",
                        target.host
                    )));
                }
            },
            Err(err) => {
                return Err(TransportError::Socks(format!(
                    "cannot resolve `{}`: {err}",
                    target.host
                )));
            }
        }
    } else {
        Destination::Domain(target.host.clone())
    };

    let mut request = vec![0x05, 0x01, 0x00];
    match destination {
        Destination::Ip(IpAddr::V4(ip)) => {
            request.push(0x01);
            request.extend_from_slice(&ip.octets());
        }
        Destination::Ip(IpAddr::V6(ip)) => {
            request.push(0x04);
            request.extend_from_slice(&ip.octets());
        }
        Destination::Domain(host) => {
            if host.len() > 255 {
                return Err(TransportError::Socks("hostname too long".into()));
            }
            request.push(0x03);
            request.push(host.len() as u8);
            request.extend_from_slice(host.as_bytes());
        }
    }
    request.extend_from_slice(&target.port.to_be_bytes());
    stream.write_all(&request).await?;
    stream.flush().await?;

    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await?;
    if head[0] != 0x05 {
        return Err(TransportError::Socks(format!(
            "unexpected SOCKS5 version byte 0x{:02x}",
            head[0]
        )));
    }
    if head[1] != 0x00 {
        return Err(TransportError::Socks(reply_message(head[1])));
    }

    let address_len = match head[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            len[0] as usize
        }
        other => {
            return Err(TransportError::Socks(format!(
                "unknown address type 0x{other:02x}"
            )));
        }
    };
    let mut rest = vec![0u8; address_len + 2];
    stream.read_exact(&mut rest).await?;
    Ok(())
}

enum Destination {
    Ip(IpAddr),
    Domain(String),
}

fn reply_message(code: u8) -> String {
    let reason = match code {
        0x01 => "general SOCKS server failure",
        0x02 => "connection not allowed by ruleset",
        0x03 => "network unreachable",
        0x04 => "host unreachable",
        0x05 => "connection refused",
        0x06 => "TTL expired",
        0x07 => "command not supported",
        0x08 => "address type not supported",
        _ => "unknown error",
    };
    format!("{reason} (0x{code:02x})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_content_length_response() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX-Test: yes\r\n\r\nhello";
        let mut stream = &raw[..];
        let response = read_response(&mut stream).await.unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.text(), "hello");
        assert_eq!(response.header("x-test"), Some("yes"));
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn parses_chunked_response() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                    4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
        let mut stream = &raw[..];
        let response = read_response(&mut stream).await.unwrap();
        assert_eq!(response.text(), "Wikipedia");
    }

    #[tokio::test]
    async fn parses_chunked_response_with_extensions_and_trailers() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                    3;foo=bar\r\nabc\r\n0\r\nX-Trailer: 1\r\n\r\n";
        let mut stream = &raw[..];
        let response = read_response(&mut stream).await.unwrap();
        assert_eq!(response.text(), "abc");
    }

    #[tokio::test]
    async fn reads_body_until_eof_without_length() {
        let raw = b"HTTP/1.1 200 OK\r\n\r\nstreamed body";
        let mut stream = &raw[..];
        let response = read_response(&mut stream).await.unwrap();
        assert_eq!(response.text(), "streamed body");
    }

    #[tokio::test]
    async fn error_status_is_not_success() {
        let raw = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n";
        let mut stream = &raw[..];
        let response = read_response(&mut stream).await.unwrap();
        assert_eq!(response.status, 403);
        assert!(!response.is_success());
    }

    #[tokio::test]
    async fn truncation_is_reported() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc";
        let mut stream = &raw[..];
        assert!(read_response(&mut stream).await.is_err());
    }

    #[tokio::test]
    async fn malformed_status_line_is_reported() {
        let raw = b"NOT-HTTP\r\n\r\n";
        let mut stream = &raw[..];
        assert!(read_response(&mut stream).await.is_err());
    }

    #[test]
    fn parses_targets() {
        let target = Target::parse("https://example.com/path?q=1").unwrap();
        assert!(target.tls);
        assert_eq!(target.host, "example.com");
        assert_eq!(target.port, 443);
        assert_eq!(target.path, "/path?q=1");
        assert_eq!(target.host_header(), "example.com");
        assert_eq!(target.connect_authority(), "example.com:443");

        let plain = Target::parse("http://example.com:8080").unwrap();
        assert!(!plain.tls);
        assert_eq!(plain.port, 8080);
        assert_eq!(plain.path, "/");
        assert_eq!(plain.host_header(), "example.com:8080");
    }

    #[test]
    fn rejects_unsupported_urls() {
        assert!(Target::parse("ftp://example.com").is_err());
        assert!(Target::parse("not a url").is_err());
    }

    #[test]
    fn builds_absolute_uri_for_plain_http_proxies() {
        let proxy = Proxy::new(crate::proxy::Protocol::Http, "10.0.0.1", 3128);
        let target = Target::parse("http://example.com/hello").unwrap();
        let request = build_request(Some(&proxy), &target);
        assert!(request.starts_with("GET http://example.com/hello HTTP/1.1\r\n"));
        assert!(!request.contains("Proxy-Authorization"));
    }

    #[test]
    fn uses_origin_form_inside_tunnels() {
        let proxy = Proxy::new(crate::proxy::Protocol::Socks5, "10.0.0.1", 1080);
        let target = Target::parse("https://example.com/hello").unwrap();
        let request = build_request(Some(&proxy), &target);
        assert!(request.starts_with("GET /hello HTTP/1.1\r\n"));
    }

    #[test]
    fn adds_basic_auth_for_plain_http_proxies() {
        let proxy = Proxy::new(crate::proxy::Protocol::Http, "10.0.0.1", 3128)
            .with_credentials("user", Some("pass".into()));
        let target = Target::parse("http://example.com/").unwrap();
        let request = build_request(Some(&proxy), &target);
        assert!(request.contains("Proxy-Authorization: Basic dXNlcjpwYXNz\r\n"));
    }
}
