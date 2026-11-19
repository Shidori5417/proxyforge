//! End-to-end tests against local mock proxies and a mock judge.
//!
//! Every test spins up its servers on an ephemeral loopback port, so the suite
//! is completely offline.

use std::future::Future;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use clap::Parser as _;

use proxyforge::anonymity::AnonymityLevel;
use proxyforge::checker::{CheckSettings, check_proxy};
use proxyforge::cli::Cli;
use proxyforge::input;
use proxyforge::proxy::{Protocol, Proxy};
use proxyforge::transport::{self, Target};

/// Accepts connections forever and runs `handler` for each of them.
async fn spawn<F, Fut>(handler: F) -> SocketAddr
where
    F: Fn(TcpStream) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handler = Arc::new(handler);

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let handler = Arc::clone(&handler);
            tokio::spawn(async move { handler(stream).await });
        }
    });

    address
}

async fn read_head(stream: &mut BufReader<TcpStream>) -> (String, Vec<(String, String)>) {
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    let request_line = line.trim_end().to_string();

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if stream.read_line(&mut line).await.unwrap() == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }

    (request_line, headers)
}

/// Copies bytes in both directions until both peers close.
async fn relay(left: TcpStream, right: TcpStream) {
    let (mut left_read, mut left_write) = left.into_split();
    let (mut right_read, mut right_write) = right.into_split();
    let _ = tokio::join!(
        tokio::io::copy(&mut left_read, &mut right_write),
        tokio::io::copy(&mut right_read, &mut left_write),
    );
}

/// Judge that reports a fixed origin and echoes the request headers.
async fn spawn_judge(extra_headers: Vec<(String, String)>) -> SocketAddr {
    spawn(move |stream| {
        let extra_headers = extra_headers.clone();
        async move {
            let mut reader = BufReader::new(stream);
            let (_line, headers) = read_head(&mut reader).await;

            let mut echoed = serde_json::Map::new();
            for (name, value) in headers {
                echoed.insert(name, serde_json::Value::String(value));
            }
            for (name, value) in extra_headers {
                echoed.insert(name, serde_json::Value::String(value));
            }

            let body = serde_json::json!({
                "origin": "203.0.113.7",
                "headers": echoed,
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );

            let mut stream = reader.into_inner();
            let _ = stream.write_all(response.as_bytes()).await;
        }
    })
    .await
}

/// Serves a raw page containing proxy endpoints, like a public list site.
async fn spawn_list_page(body: &'static str) -> SocketAddr {
    spawn(move |stream| async move {
        let mut reader = BufReader::new(stream);
        let (_line, _headers) = read_head(&mut reader).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let mut stream = reader.into_inner();
        let _ = stream.write_all(response.as_bytes()).await;
    })
    .await
}

/// HTTP proxy supporting absolute-form `GET` and `CONNECT`.
async fn spawn_http_proxy() -> SocketAddr {
    spawn(|stream| async move {
        let mut reader = BufReader::new(stream);
        let (request_line, headers) = read_head(&mut reader).await;
        let mut parts = request_line.split(' ');
        let method = parts.next().unwrap_or_default();
        let uri = parts.next().unwrap_or_default();
        let version = parts.next().unwrap_or("HTTP/1.1");
        let mut client = reader.into_inner();

        if method == "CONNECT" {
            match TcpStream::connect(uri).await {
                Ok(upstream) => {
                    let _ = client
                        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                        .await;
                    relay(client, upstream).await;
                }
                Err(_) => {
                    let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
                }
            }
            return;
        }

        let url = url::Url::parse(uri).expect("absolute request URI");
        let host = url.host_str().expect("host").to_string();
        let port = url.port_or_known_default().expect("port");
        let path = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_string(),
        };

        let mut request = format!("{method} {path} {version}\r\n");
        for (name, value) in &headers {
            if name.eq_ignore_ascii_case("proxy-authorization") {
                continue;
            }
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("Connection: close\r\n\r\n");

        match TcpStream::connect((host.as_str(), port)).await {
            Ok(mut upstream) => {
                let _ = upstream.write_all(request.as_bytes()).await;
                relay(client, upstream).await;
            }
            Err(_) => {
                let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
            }
        }
    })
    .await
}

/// SOCKS4a proxy that resolves the requested hostname.
async fn spawn_socks4() -> SocketAddr {
    spawn(|mut stream| async move {
        let mut head = [0u8; 8];
        stream.read_exact(&mut head).await.unwrap();
        assert_eq!(head[0], 0x04);
        assert_eq!(head[1], 0x01);
        let port = u16::from_be_bytes([head[2], head[3]]);

        let mut user = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            stream.read_exact(&mut byte).await.unwrap();
            if byte[0] == 0 {
                break;
            }
            user.push(byte[0]);
        }

        let host = if head[4..8] == [0, 0, 0, 1] {
            let mut host = Vec::new();
            loop {
                let mut byte = [0u8; 1];
                stream.read_exact(&mut byte).await.unwrap();
                if byte[0] == 0 {
                    break;
                }
                host.push(byte[0]);
            }
            String::from_utf8(host).unwrap()
        } else {
            format!("{}.{}.{}.{}", head[4], head[5], head[6], head[7])
        };

        match TcpStream::connect((host.as_str(), port)).await {
            Ok(upstream) => {
                let _ = stream.write_all(&[0x00, 0x5a, 0, 0, 0, 0, 0, 0]).await;
                relay(stream, upstream).await;
            }
            Err(_) => {
                let _ = stream.write_all(&[0x00, 0x5b, 0, 0, 0, 0, 0, 0]).await;
            }
        }
    })
    .await
}

/// SOCKS5 proxy with optional username/password authentication.
async fn spawn_socks5(credentials: Option<(&'static str, &'static str)>) -> SocketAddr {
    spawn(move |mut stream| async move {
        let mut greeting = [0u8; 2];
        stream.read_exact(&mut greeting).await.unwrap();
        assert_eq!(greeting[0], 0x05);
        let mut methods = vec![0u8; greeting[1] as usize];
        stream.read_exact(&mut methods).await.unwrap();

        let wants_auth = credentials.is_some() && methods.contains(&0x02);
        let method = if wants_auth { 0x02 } else { 0x00 };
        stream.write_all(&[0x05, method]).await.unwrap();

        if wants_auth {
            let (expected_user, expected_password) = credentials.unwrap();
            let mut header = [0u8; 2];
            stream.read_exact(&mut header).await.unwrap();
            let mut user = vec![0u8; header[1] as usize];
            stream.read_exact(&mut user).await.unwrap();
            let mut password_len = [0u8; 1];
            stream.read_exact(&mut password_len).await.unwrap();
            let mut password = vec![0u8; password_len[0] as usize];
            stream.read_exact(&mut password).await.unwrap();

            let allowed =
                user == expected_user.as_bytes() && password == expected_password.as_bytes();
            stream
                .write_all(&[0x01, if allowed { 0x00 } else { 0x01 }])
                .await
                .unwrap();
            if !allowed {
                return;
            }
        }

        let mut request = [0u8; 4];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(request[0], 0x05);
        assert_eq!(request[1], 0x01);

        let host = match request[3] {
            0x01 => {
                let mut octets = [0u8; 4];
                stream.read_exact(&mut octets).await.unwrap();
                std::net::Ipv4Addr::from(octets).to_string()
            }
            0x04 => {
                let mut octets = [0u8; 16];
                stream.read_exact(&mut octets).await.unwrap();
                Ipv6Addr::from(octets).to_string()
            }
            0x03 => {
                let mut len = [0u8; 1];
                stream.read_exact(&mut len).await.unwrap();
                let mut host = vec![0u8; len[0] as usize];
                stream.read_exact(&mut host).await.unwrap();
                String::from_utf8(host).unwrap()
            }
            other => panic!("unexpected address type {other}"),
        };
        let mut port = [0u8; 2];
        stream.read_exact(&mut port).await.unwrap();
        let port = u16::from_be_bytes(port);

        match TcpStream::connect((host.as_str(), port)).await {
            Ok(upstream) => {
                let _ = stream
                    .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                    .await;
                relay(stream, upstream).await;
            }
            Err(_) => {
                let _ = stream
                    .write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                    .await;
            }
        }
    })
    .await
}

fn settings(judge: SocketAddr) -> CheckSettings {
    CheckSettings {
        target: Target::parse(&format!("http://{judge}/get")).unwrap(),
        timeout: Duration::from_secs(3),
        retries: 0,
        anonymity: true,
        local_ip: Some("198.51.100.1".into()),
    }
}

fn proxy(protocol: Protocol, addr: SocketAddr) -> Proxy {
    Proxy::new(protocol, "127.0.0.1", addr.port())
}

#[tokio::test]
async fn http_proxy_is_alive_and_elite() {
    let judge = spawn_judge(Vec::new()).await;
    let address = spawn_http_proxy().await;
    let result = check_proxy(&proxy(Protocol::Http, address), &settings(judge)).await;

    assert!(result.alive, "error: {:?}", result.error);
    assert_eq!(result.status_code, Some(200));
    assert_eq!(result.exit_ip.as_deref(), Some("203.0.113.7"));
    assert_eq!(result.anonymity, Some(AnonymityLevel::Elite));
}

#[tokio::test]
async fn forwarding_headers_make_a_proxy_anonymous() {
    let judge = spawn_judge(vec![("X-Forwarded-For".into(), "203.0.113.7".into())]).await;
    let address = spawn_http_proxy().await;
    let result = check_proxy(&proxy(Protocol::Http, address), &settings(judge)).await;

    assert!(result.alive, "error: {:?}", result.error);
    assert_eq!(result.anonymity, Some(AnonymityLevel::Anonymous));
    assert_eq!(result.leaks.len(), 1);
}

#[tokio::test]
async fn leaking_the_client_ip_makes_a_proxy_transparent() {
    let judge = spawn_judge(vec![("X-Forwarded-For".into(), "198.51.100.1".into())]).await;
    let address = spawn_http_proxy().await;
    let result = check_proxy(&proxy(Protocol::Http, address), &settings(judge)).await;

    assert!(result.alive, "error: {:?}", result.error);
    assert_eq!(result.anonymity, Some(AnonymityLevel::Transparent));
}

#[tokio::test]
async fn socks5h_proxy_is_alive() {
    let judge = spawn_judge(Vec::new()).await;
    let address = spawn_socks5(None).await;
    let result = check_proxy(&proxy(Protocol::Socks5h, address), &settings(judge)).await;

    assert!(result.alive, "error: {:?}", result.error);
    assert_eq!(result.exit_ip.as_deref(), Some("203.0.113.7"));
}

#[tokio::test]
async fn socks5_proxy_resolves_the_target_locally() {
    let judge = spawn_judge(Vec::new()).await;
    let address = spawn_socks5(None).await;
    let result = check_proxy(&proxy(Protocol::Socks5, address), &settings(judge)).await;

    assert!(result.alive, "error: {:?}", result.error);
}

#[tokio::test]
async fn socks5_authentication_succeeds_with_credentials() {
    let judge = spawn_judge(Vec::new()).await;
    let address = spawn_socks5(Some(("user", "pass"))).await;
    let proxy = proxy(Protocol::Socks5h, address).with_credentials("user", Some("pass".into()));
    let result = check_proxy(&proxy, &settings(judge)).await;

    assert!(result.alive, "error: {:?}", result.error);
}

#[tokio::test]
async fn socks5_authentication_rejects_wrong_credentials() {
    let judge = spawn_judge(Vec::new()).await;
    let address = spawn_socks5(Some(("user", "pass"))).await;
    let proxy = proxy(Protocol::Socks5h, address).with_credentials("user", Some("wrong".into()));
    let result = check_proxy(&proxy, &settings(judge)).await;

    assert!(!result.alive);
    assert!(result.error.unwrap().contains("authentication rejected"));
}

#[tokio::test]
async fn socks4_proxy_is_alive() {
    let judge = spawn_judge(Vec::new()).await;
    let address = spawn_socks4().await;
    let result = check_proxy(&proxy(Protocol::Socks4, address), &settings(judge)).await;

    assert!(result.alive, "error: {:?}", result.error);
    assert_eq!(result.exit_ip.as_deref(), Some("203.0.113.7"));
}

#[tokio::test]
async fn refused_connection_is_dead() {
    let judge = spawn_judge(Vec::new()).await;
    let proxy = Proxy::new(Protocol::Http, "127.0.0.1", 1);
    let result = check_proxy(&proxy, &settings(judge)).await;

    assert!(!result.alive);
    assert!(result.error.unwrap().to_lowercase().contains("connect"));
}

#[tokio::test]
async fn silent_proxy_times_out() {
    let judge = spawn_judge(Vec::new()).await;
    let address = spawn(|stream| async move {
        // Hold the connection open without ever answering.
        let _open = stream;
        tokio::time::sleep(Duration::from_secs(5)).await;
    })
    .await;

    let mut settings = settings(judge);
    settings.timeout = Duration::from_millis(200);
    let result = check_proxy(&proxy(Protocol::Http, address), &settings).await;

    assert!(!result.alive);
    assert!(
        result.error.as_deref().unwrap_or("").contains("timed out"),
        "error: {:?}",
        result.error
    );
}

#[tokio::test]
async fn transport_returns_the_judge_body() {
    let judge = spawn_judge(Vec::new()).await;
    let address = spawn_http_proxy().await;
    let target = Target::parse(&format!("http://{judge}/get")).unwrap();
    let response = transport::get(
        &proxy(Protocol::Http, address),
        &target,
        Duration::from_secs(3),
    )
    .await
    .unwrap();

    assert_eq!(response.status, 200);
    assert!(response.text().contains("203.0.113.7"));
    assert!(response.header("content-type").is_some());
}

#[tokio::test]
async fn cli_writes_a_json_report() {
    let judge = spawn_judge(Vec::new()).await;
    let address = spawn_http_proxy().await;

    let args: Vec<String> = vec![
        format!("127.0.0.1:{}", address.port()),
        "--judge".into(),
        format!("http://{judge}/get"),
        "--no-anonymity".into(),
        "--no-progress".into(),
        "--alive-only".into(),
        "--format".into(),
        "json".into(),
    ];

    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_proxyforge"))
            .args(&args)
            .output()
            .expect("binary runs")
    })
    .await
    .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report[0]["alive"], true);
    assert_eq!(report[0]["status_code"], 200);
    assert_eq!(
        report[0]["proxy"],
        format!("http://127.0.0.1:{}", address.port())
    );
}

#[tokio::test]
async fn scrapes_proxies_from_a_remote_list() {
    let proxy_addr = spawn_http_proxy().await;
    let judge = spawn_judge(Vec::new()).await;
    let body: &'static str = Box::leak(
        format!(
            "<html><td>1.2.3.4:8080</td><td>http://user:pass@127.0.0.1:{}</td></html>",
            proxy_addr.port()
        )
        .into_boxed_str(),
    );
    let page = spawn_list_page(body).await;

    let cli = Cli::parse_from(["proxyforge", "--input", &format!("http://{page}/list")]);
    let outcome = input::load(&cli).await.unwrap();

    assert_eq!(outcome.scraped, 2);
    assert_eq!(outcome.proxies.len(), 2);
    assert!(
        outcome
            .proxies
            .iter()
            .any(|proxy| proxy.port == proxy_addr.port())
    );

    // The scraped proxy really works end to end.
    let mut settings = settings(judge);
    settings.anonymity = false;
    let scraped = outcome
        .proxies
        .iter()
        .find(|proxy| proxy.port == proxy_addr.port())
        .unwrap();
    let result = check_proxy(scraped, &settings).await;
    assert!(result.alive, "error: {:?}", result.error);
}
