//! Shared HTTP/WebSocket plumbing for market-data adapters.
//!
//! Both clients honour `HTTPS_PROXY` / `NO_PROXY` and trust the platform's
//! certificate store (and `SSL_CERT_FILE`).

use std::sync::Once;
use std::time::Duration;

use anyhow::{Context, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

const USER_AGENT: &str = concat!("ledger-portfolio/", env!("CARGO_PKG_VERSION"));

/// rustls needs one process-wide crypto backend; we use ring everywhere.
pub fn init_crypto() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub fn http_client() -> reqwest::Client {
    init_crypto();
    reqwest::Client::builder()
        .user_agent(USER_AGENT) // Coinbase rejects requests without one.
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .expect("http client")
}

/// Open a WebSocket, tunnelling through `HTTPS_PROXY` when one applies.
pub async fn connect_ws(url: &str) -> anyhow::Result<WsStream> {
    init_crypto();
    let mut request = url.into_client_request().context("invalid websocket url")?;
    request.headers_mut().insert("User-Agent", USER_AGENT.parse().unwrap());
    let uri = request.uri().clone();
    let host = uri.host().context("websocket url has no host")?.to_string();
    let port = uri.port_u16().unwrap_or(if uri.scheme_str() == Some("wss") { 443 } else { 80 });

    let stream = match proxy_for(&host) {
        Some(proxy) => tunnel(&proxy, &host, port).await?,
        None => TcpStream::connect((host.as_str(), port)).await.with_context(|| format!("connect {host}:{port}"))?,
    };
    stream.set_nodelay(true).ok();
    let (ws, _) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::client_async_tls_with_config(request, stream, None, None),
    )
    .await
    .context("websocket handshake timed out")?
    .with_context(|| format!("websocket handshake with {host}"))?;
    Ok(ws)
}

fn env_any(names: &[&str]) -> Option<String> {
    names.iter().find_map(|n| std::env::var(n).ok()).filter(|v| !v.trim().is_empty())
}

/// Proxy `host:port` to use for `host`, if any.
fn proxy_for(host: &str) -> Option<String> {
    let proxy = env_any(&["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"])?;
    let no_proxy = env_any(&["NO_PROXY", "no_proxy"]).unwrap_or_default();
    if bypass(host, &no_proxy) {
        return None;
    }
    let addr = proxy.split("://").nth(1).unwrap_or(&proxy);
    let addr = addr.rsplit('@').next().unwrap_or(addr).trim_end_matches('/');
    Some(if addr.contains(':') { addr.to_string() } else { format!("{addr}:80") })
}

/// `NO_PROXY` matching: exact hosts, domain suffixes (`.example.com` or
/// `example.com`), and `*`. CIDR ranges are matched only for loopback.
fn bypass(host: &str, no_proxy: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    if host == "localhost" || host.starts_with("127.") || host == "::1" {
        return true;
    }
    no_proxy.split(',').map(|e| e.trim().to_ascii_lowercase()).filter(|e| !e.is_empty()).any(|e| {
        let e = e.trim_start_matches("*.").trim_start_matches('.');
        e == "*" || host == e || host.ends_with(&format!(".{e}"))
    })
}

/// HTTP CONNECT tunnel through a proxy.
async fn tunnel(proxy: &str, host: &str, port: u16) -> anyhow::Result<TcpStream> {
    let mut s = TcpStream::connect(proxy).await.with_context(|| format!("connect to proxy {proxy}"))?;
    let req = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\nUser-Agent: {USER_AGENT}\r\n\r\n");
    s.write_all(req.as_bytes()).await?;
    // Read the response head byte-by-byte so no tunnelled bytes are consumed.
    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 8192 || s.read(&mut byte).await? == 0 {
            bail!("proxy closed connection during CONNECT");
        }
        head.push(byte[0]);
    }
    let status_line = String::from_utf8_lossy(&head).lines().next().unwrap_or_default().to_string();
    if !status_line.split_whitespace().nth(1).is_some_and(|c| c.starts_with('2')) {
        bail!("proxy refused CONNECT to {host}:{port}: {status_line}");
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_proxy_matching() {
        let np = "localhost,.svc.cluster.local,example.com,*.internal";
        assert!(bypass("127.0.0.1", ""));
        assert!(bypass("api.example.com", np));
        assert!(bypass("example.com", np));
        assert!(bypass("foo.svc.cluster.local", np));
        assert!(bypass("db.internal", np));
        assert!(!bypass("ws-feed.exchange.coinbase.com", np));
        assert!(!bypass("notexample.com", np));
        assert!(bypass("anything", "*"));
    }
}
