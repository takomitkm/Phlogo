//! PROXY protocol v1 support (A1).
//!
//! Caddy (or any PROXY-aware proxy) prepends a small header to the connection that
//! carries the **real** client address. Unlike `X-Forwarded-For` — which is just an
//! HTTP header any client can write — this arrives at the TCP layer, so it cannot be
//! forged by a client that reaches the API port directly.
//!
//! v1 format (text, terminated by CRLF, at most 107 bytes):
//! ```text
//! PROXY TCP4 192.168.0.1 192.168.0.11 56324 443\r\n
//! PROXY TCP6 2001:db8::1 2001:db8::2 56324 443\r\n
//! PROXY UNKNOWN\r\n
//! ```
//!
//! Only `TCP4`/`TCP6` carry an address; `UNKNOWN` does not.
//!
//! axum 0.7's `axum::serve` creates the TCP stream internally and hands it straight to
//! hyper, so there is no hook to consume a PROXY header first. This module therefore
//! provides its own accept loop ([`serve_with_proxy_protocol`]) and drives the router
//! through hyper directly — modelled on axum's own `serve.rs`.
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::extract::ConnectInfo;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};

/// Maximum length of a v1 header including CRLF (per the PROXY protocol spec).
pub const MAX_V1_HEADER: usize = 107;

/// Result of looking at the first bytes of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parsed {
    /// A PROXY v1 header was present; it carried this address.
    Proxy(SocketAddr),
    /// A PROXY v1 header was present but carried no address (`PROXY UNKNOWN`).
    Unknown,
    /// No PROXY header — the bytes are the start of a plain HTTP request.
    NotProxy,
}

/// Parse a PROXY v1 header body (without the trailing CRLF).
///
/// Returns `Ok(None)` when the text is not a PROXY v1 header, or when it is
/// `PROXY UNKNOWN` (a header without an address).
pub fn parse_v1(body: &[u8]) -> Result<Option<SocketAddr>, String> {
    let text = String::from_utf8_lossy(body);
    let mut parts = text.split_whitespace();
    if parts.next() != Some("PROXY") {
        return Ok(None);
    }
    let proto = parts.next().unwrap_or("");
    let src_ip = parts.next().unwrap_or("");
    let _dst_ip = parts.next().unwrap_or("");
    let src_port = parts.next().unwrap_or("");
    if proto == "UNKNOWN" {
        return Ok(None);
    }
    if proto != "TCP4" && proto != "TCP6" {
        return Err(format!("unsupported PROXY protocol family: {proto}"));
    }
    let port: u16 = src_port
        .parse()
        .map_err(|_| format!("bad PROXY source port: {src_port}"))?;
    let ip: std::net::IpAddr = src_ip
        .parse()
        .map_err(|_| format!("bad PROXY source ip: {src_ip}"))?;
    Ok(Some(SocketAddr::new(ip, port)))
}

/// Decide whether a byte prefix starts a PROXY v1 header.
///
/// `PROXY UNKNOWN` is reported as `Parsed::Unknown` (a header, but no address);
/// anything else that does not start with `PROXY ` is `Parsed::NotProxy`.
///
/// The caller is responsible for cutting the header at the CRLF; this function only
/// looks at the header body.
pub fn classify(prefix: &[u8]) -> Parsed {
    if !prefix.starts_with(b"PROXY ") {
        return Parsed::NotProxy;
    }
    if prefix.starts_with(b"PROXY UNKNOWN") {
        return Parsed::Unknown;
    }
    match parse_v1(prefix) {
        Ok(Some(addr)) => Parsed::Proxy(addr),
        Ok(None) => Parsed::Unknown,
        Err(_) => Parsed::Unknown,
    }
}

/// The client address of a request.
///
/// `ConnectInfo` is inserted by the server (either axum's
/// `into_make_service_with_connect_info` or this module's own loop); with the PROXY
/// path it carries the *real* client address instead of the proxy's.
pub fn peer_addr(extensions: &axum::http::Extensions) -> Option<SocketAddr> {
    extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0)
}

/// The client IP used as the rate-limit bucket key.
///
/// Prefers the PROXY-aware `ConnectInfo`; falls back to `0.0.0.0` (one shared bucket)
/// when the connection info is missing — the conservative direction.
pub fn peer_ip(extensions: &axum::http::Extensions) -> std::net::IpAddr {
    peer_addr(extensions)
        .map(|a| a.ip())
        .unwrap_or_else(|| "0.0.0.0".parse().expect("valid ip"))
}

/// A stream that yields `prefix` first and then delegates to the inner stream.
///
/// Needed because we must peek at the first bytes to decide whether a PROXY header is
/// present; if it is not, those bytes are the start of the HTTP request and must be
/// replayed rather than dropped.
pub struct PrefixedStream<S> {
    prefix: Vec<u8>,
    offset: usize,
    inner: S,
}

impl<S> PrefixedStream<S> {
    pub fn new(prefix: Vec<u8>, inner: S) -> Self {
        Self {
            prefix,
            offset: 0,
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for PrefixedStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.offset < self.prefix.len() {
            let remaining = &self.prefix[self.offset..];
            let n = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..n]);
            self.offset += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PrefixedStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Serve the router, accepting an optional PROXY protocol v1 header per connection.
///
/// When `expect_proxy` is true the header is **required**: a connection that does not
/// start with a valid PROXY header is dropped (the API port must not be reachable
/// directly). When false, plain connections are served as-is.
pub async fn serve_with_proxy_protocol(
    listener: TcpListener,
    router: axum::Router,
    expect_proxy: bool,
) -> std::io::Result<()> {
    let router = router.clone();
    loop {
        let (stream, peer) = listener.accept().await?;
        let router = router.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, peer, router, expect_proxy).await {
                tracing::debug!(%peer, error = %e, "connection closed");
            }
        });
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    router: axum::Router,
    expect_proxy: bool,
) -> std::io::Result<()> {
    let (client_addr, prefix) = read_proxy_header(&mut stream).await?;
    let client_addr = match client_addr {
        Some(addr) => addr,
        None => {
            if expect_proxy {
                tracing::warn!(%peer, "connection without PROXY header rejected");
                return Ok(());
            }
            peer
        }
    };

    let io = TokioIo::new(PrefixedStream::new(prefix, stream));
    let service = service_fn(move |req: hyper::Request<Incoming>| {
        let router = router.clone();
        async move {
            let (mut parts, body) = req.into_parts();
            parts.extensions.insert(ConnectInfo(client_addr));
            // hyper's Incoming -> axum's Body, then hand the request to the Router.
            let req = axum::extract::Request::from_parts(parts, axum::body::Body::new(body));
            let mut svc = router.clone();
            // `Router`'s Service error type is `Infallible`.
            Ok::<_, std::convert::Infallible>(
                tower::Service::call(&mut svc, req).await.unwrap_or_else(|e| match e {}),
            )
        }
    });

    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(io, service)
        .with_upgrades()
        .await;
    Ok(())
}

/// Read an optional PROXY v1 header from the connection.
///
/// Returns `(real client address or None, leftover bytes)` — the leftover must be
/// replayed to hyper because it may contain the start of the HTTP request (the first
/// read can return the PROXY header *and* part of the request in one go).
async fn read_proxy_header(
    stream: &mut TcpStream,
) -> std::io::Result<(Option<SocketAddr>, Vec<u8>)> {
    let mut buf = [0u8; MAX_V1_HEADER];
    let mut filled = 0usize;

    loop {
        let n = stream.read(&mut buf[filled..]).await?;
        if n == 0 {
            return Ok((None, buf[..filled].to_vec()));
        }
        filled += n;
        if let Some(pos) = buf[..filled].windows(2).position(|w| w == b"\r\n") {
            let header = &buf[..pos];
            if header.starts_with(b"PROXY ") {
                let parsed = match classify(header) {
                    Parsed::Proxy(addr) => Some(addr),
                    Parsed::Unknown | Parsed::NotProxy => None,
                };
                // The PROXY header (up to and including CRLF) is consumed; anything
                // after it belongs to the HTTP request and must be replayed.
                return Ok((parsed, buf[pos + 2..filled].to_vec()));
            }
            // Not a PROXY header: the CRLF was part of the HTTP request line.
            return Ok((None, buf[..filled].to_vec()));
        }
        if filled == buf.len() {
            // No CRLF within the maximum header length — not a PROXY header.
            return Ok((None, buf[..filled].to_vec()));
        }
    }
}

/// Write a PROXY v1 header — used by the test helper that emulates a proxy.
#[allow(dead_code)]
pub fn write_v1_header(addr: SocketAddr) -> Vec<u8> {
    let ip = addr.ip();
    let proto = if ip.is_ipv4() { "TCP4" } else { "TCP6" };
    format!("PROXY {proto} {ip} {ip} {} 0\r\n", addr.port()).into_bytes()
}

/// Helper for tests/tools: send a PROXY v1 header on a stream.
#[allow(dead_code)]
pub async fn send_v1_header(stream: &mut TcpStream, addr: SocketAddr) -> std::io::Result<()> {
    stream.write_all(&write_v1_header(addr)).await
}

/// Unused import guard for `TokioExecutor` (kept for future HTTP/2 support).
#[allow(dead_code)]
fn _executor_guard() -> TokioExecutor {
    TokioExecutor::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tcp4() {
        let p = parse_v1(b"PROXY TCP4 192.168.0.1 192.168.0.11 56324 443").unwrap();
        assert_eq!(
            p,
            Some("192.168.0.1:56324".parse::<SocketAddr>().unwrap())
        );
    }

    #[test]
    fn parses_tcp6() {
        let p = parse_v1(b"PROXY TCP6 2001:db8::1 2001:db8::2 56324 443").unwrap();
        assert_eq!(
            p,
            Some("[2001:db8::1]:56324".parse::<SocketAddr>().unwrap())
        );
    }

    #[test]
    fn unknown_has_no_address() {
        assert_eq!(parse_v1(b"PROXY UNKNOWN").unwrap(), None);
    }

    #[test]
    fn non_proxy_is_none() {
        assert_eq!(parse_v1(b"GET / HTTP/1.1").unwrap(), None);
    }

    #[test]
    fn classify_distinguishes() {
        assert_eq!(classify(b"GET / HTTP/1.1\r\n"), Parsed::NotProxy);
        assert_eq!(
            classify(b"PROXY TCP4 10.0.0.1 10.0.0.2 1234 443\r\n"),
            Parsed::Proxy("10.0.0.1:1234".parse().unwrap())
        );
        assert_eq!(classify(b"PROXY UNKNOWN\r\n"), Parsed::Unknown);
    }

    #[tokio::test]
    async fn prefixed_stream_replays_bytes() {
        // The prefix must be delivered before the inner stream's data.
        let (mut w, r) = tokio::io::duplex(64);
        w.write_all(b"world").await.unwrap();
        // Close the write half so the reader sees EOF after "world".
        drop(w);
        let mut prefixed = PrefixedStream::new(b"hello ".to_vec(), r);
        let mut out = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut prefixed, &mut out)
            .await
            .unwrap();
        assert_eq!(out, b"hello world");
    }
}
