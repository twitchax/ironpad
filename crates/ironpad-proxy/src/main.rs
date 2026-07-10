//! Domain-filtering forward proxy for the cell-compile network sandbox.
//!
//! Speaks the HTTP `CONNECT` method only: a client asks to tunnel to
//! `host:port`, the proxy checks the hostname against a fail-closed allowlist
//! (`IRONPAD_PROXY_ALLOWLIST`), and either opens a bidirectional TCP tunnel or
//! rejects it. Binds `127.0.0.1` — its sole client is the trusted cargo build
//! sandbox — so the request parsing is deliberately minimal but bounded.
#![deny(clippy::all, clippy::pedantic)]

use std::env;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

// ── Domain Allowlist ─────────────────────────────────────────────────────────

/// A fail-closed domain allowlist that supports suffix matching.
///
/// A domain `crates.io` in the allowlist matches `crates.io` (exact) and
/// `static.crates.io` (subdomain), but NOT `evilcrates.io`.
#[derive(Debug, Clone)]
struct DomainAllowlist {
    domains: Vec<String>,
}

impl DomainAllowlist {
    /// Build from the `IRONPAD_PROXY_ALLOWLIST` env var.
    /// Missing or empty → empty list (deny all).
    fn from_env() -> Self {
        let raw = env::var("IRONPAD_PROXY_ALLOWLIST").unwrap_or_default();
        Self::parse(&raw)
    }

    /// Parse a comma-separated domain string into an allowlist.
    fn parse(raw: &str) -> Self {
        let domains: Vec<String> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
            .collect();
        Self { domains }
    }

    /// Check whether `hostname` is allowed.
    ///
    /// Match rules: hostname == domain OR hostname ends with `.{domain}`.
    fn is_allowed(&self, hostname: &str) -> bool {
        let hostname = hostname.to_lowercase();
        self.domains
            .iter()
            .any(|domain| hostname == *domain || hostname.ends_with(&format!(".{domain}")))
    }
}

// ── Connection Handler ───────────────────────────────────────────────────────

const RESPONSE_200: &[u8] = b"HTTP/1.1 200 Connection Established\r\n\r\n";
const RESPONSE_403: &[u8] = b"HTTP/1.1 403 Forbidden\r\n\r\n";
const RESPONSE_405: &[u8] = b"HTTP/1.1 405 Method Not Allowed\r\n\r\n";
const RESPONSE_413: &[u8] = b"HTTP/1.1 413 Payload Too Large\r\n\r\n";

/// Cap on the `CONNECT` request line. The only valid line is
/// `CONNECT host:port HTTP/1.x`, so this is generous; it exists to stop a client
/// that streams bytes without a newline from growing the read buffer without
/// bound (the failure the IPC/WS frame caps also guard against).
const MAX_REQUEST_LINE_BYTES: u64 = 8 * 1024;

/// Cap on the total header block after the request line. Bounds a client that
/// streams headers (or one giant header) without the terminating blank line.
const MAX_HEADER_BYTES: u64 = 64 * 1024;

async fn handle_connection(client: TcpStream, allowlist: &DomainAllowlist) {
    let peer = client
        .peer_addr()
        .map_or_else(|_| "unknown".to_string(), |a| a.to_string());

    let (reader, mut writer) = client.into_split();
    let mut buf_reader = BufReader::new(reader);

    // Read the request line, bounded so a client can't stream an unbounded line.
    let mut request_line = String::new();
    {
        let mut limited = (&mut buf_reader).take(MAX_REQUEST_LINE_BYTES);
        match limited.read_line(&mut request_line).await {
            Ok(0) => return, // clean EOF before any request
            Ok(_) => {}
            Err(e) => {
                warn!(peer = %peer, error = %e, "failed to read request line");
                return;
            }
        }
        // A line that fills the cap without a terminating newline was truncated —
        // reject rather than parse a partial request.
        if !request_line.ends_with('\n') {
            debug!(peer = %peer, "request line exceeds cap");
            let _ = writer.write_all(RESPONSE_413).await;
            return;
        }
    }

    // Parse: CONNECT host:port HTTP/1.x
    let parts: Vec<&str> = request_line.trim().splitn(3, ' ').collect();
    if parts.len() < 3 || !parts[0].eq_ignore_ascii_case("CONNECT") {
        debug!(peer = %peer, line = %request_line.trim(), "non-CONNECT request");
        let _ = writer.write_all(RESPONSE_405).await;
        return;
    }

    let target = parts[1];

    // Consume remaining headers until the blank line, bounding the total so a
    // client can't stream headers (or one giant header) without the terminator.
    {
        let mut limited = (&mut buf_reader).take(MAX_HEADER_BYTES);
        let mut header_line = String::new();
        loop {
            header_line.clear();
            match limited.read_line(&mut header_line).await {
                Ok(0) | Err(_) => break, // EOF / error → stop draining
                Ok(_) => {
                    if header_line.trim().is_empty() {
                        break; // end of headers
                    }
                }
            }
            // The cap was reached before the terminating blank line.
            if limited.limit() == 0 {
                debug!(peer = %peer, "header block exceeds cap");
                let _ = writer.write_all(RESPONSE_413).await;
                return;
            }
        }
    }

    // Extract hostname from host:port.
    let hostname = target.rsplit_once(':').map_or(target, |(host, _)| host);

    if !allowlist.is_allowed(hostname) {
        debug!(peer = %peer, hostname = %hostname, "denied");
        let _ = writer.write_all(RESPONSE_403).await;
        return;
    }

    debug!(peer = %peer, hostname = %hostname, target = %target, "allowed — tunneling");

    // Connect to upstream.
    let mut upstream = match TcpStream::connect(target).await {
        Ok(s) => s,
        Err(e) => {
            warn!(peer = %peer, target = %target, error = %e, "upstream connect failed");
            let _ = writer.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
            return;
        }
    };

    // Signal success to client.
    if let Err(e) = writer.write_all(RESPONSE_200).await {
        warn!(peer = %peer, error = %e, "failed to send 200");
        return;
    }

    // Re-join the halves so we can use copy_bidirectional.
    let mut client = buf_reader
        .into_inner()
        .reunite(writer)
        .expect("reuniting halves of the same socket");

    // Bidirectional tunnel.
    match tokio::io::copy_bidirectional(&mut client, &mut upstream).await {
        Ok((to_upstream, to_client)) => {
            debug!(peer = %peer, target = %target, to_upstream, to_client, "tunnel closed");
        }
        Err(e) => {
            debug!(peer = %peer, target = %target, error = %e, "tunnel error");
        }
    }
}

// ── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let bind_addr = env::var("IRONPAD_PROXY_BIND").unwrap_or_else(|_| "127.0.0.1:3112".to_string());

    let allowlist = Arc::new(DomainAllowlist::from_env());

    info!(
        bind = %bind_addr,
        domains = ?allowlist.domains,
        "ironpad-proxy starting"
    );

    if allowlist.domains.is_empty() {
        warn!("IRONPAD_PROXY_ALLOWLIST is empty — all connections will be denied");
    }

    let listener = TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {bind_addr}: {e}"));

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let allowlist = Arc::clone(&allowlist);
                tokio::spawn(async move {
                    handle_connection(stream, &allowlist).await;
                });
            }
            Err(e) => {
                warn!(error = %e, "accept error");
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // ── Unit: allowlist parsing ──────────────────────────────────────────

    #[test]
    fn parse_basic_allowlist() {
        let al = DomainAllowlist::parse("crates.io,github.com,githubusercontent.com");
        assert_eq!(al.domains.len(), 3);
        assert_eq!(al.domains[0], "crates.io");
        assert_eq!(al.domains[1], "github.com");
        assert_eq!(al.domains[2], "githubusercontent.com");
    }

    #[test]
    fn parse_trims_whitespace() {
        let al = DomainAllowlist::parse(" crates.io , github.com ");
        assert_eq!(al.domains, vec!["crates.io", "github.com"]);
    }

    #[test]
    fn parse_empty_string() {
        let al = DomainAllowlist::parse("");
        assert!(al.domains.is_empty());
    }

    #[test]
    fn parse_only_commas() {
        let al = DomainAllowlist::parse(",,,");
        assert!(al.domains.is_empty());
    }

    #[test]
    fn parse_normalizes_case() {
        let al = DomainAllowlist::parse("Crates.IO,GITHUB.COM");
        assert_eq!(al.domains, vec!["crates.io", "github.com"]);
    }

    // ── Unit: domain matching ───────────────────────────────────────────

    #[test]
    fn match_exact() {
        let al = DomainAllowlist::parse("crates.io");
        assert!(al.is_allowed("crates.io"));
    }

    #[test]
    fn match_subdomain() {
        let al = DomainAllowlist::parse("crates.io");
        assert!(al.is_allowed("static.crates.io"));
        assert!(al.is_allowed("index.crates.io"));
    }

    #[test]
    fn no_match_evil_prefix() {
        let al = DomainAllowlist::parse("crates.io");
        assert!(!al.is_allowed("evilcrates.io"));
    }

    #[test]
    fn no_match_unrelated() {
        let al = DomainAllowlist::parse("crates.io");
        assert!(!al.is_allowed("evil.com"));
    }

    #[test]
    fn match_case_insensitive() {
        let al = DomainAllowlist::parse("crates.io");
        assert!(al.is_allowed("CRATES.IO"));
        assert!(al.is_allowed("Static.Crates.IO"));
    }

    #[test]
    fn empty_allowlist_denies_all() {
        let al = DomainAllowlist::parse("");
        assert!(!al.is_allowed("crates.io"));
        assert!(!al.is_allowed("anything.com"));
    }

    #[test]
    fn multiple_domains() {
        let al = DomainAllowlist::parse("crates.io,github.com");
        assert!(al.is_allowed("crates.io"));
        assert!(al.is_allowed("github.com"));
        assert!(al.is_allowed("static.crates.io"));
        assert!(!al.is_allowed("evil.com"));
    }

    // ── Integration: proxy accept/deny ──────────────────────────────────

    /// Spawn the proxy on a random port and return (addr, allowlist).
    async fn spawn_proxy(allowlist_str: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let allowlist = Arc::new(DomainAllowlist::parse(allowlist_str));

        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    let al = Arc::clone(&allowlist);
                    tokio::spawn(async move {
                        handle_connection(stream, &al).await;
                    });
                }
            }
        });

        addr
    }

    #[tokio::test]
    async fn proxy_denies_unlisted_domain() {
        let addr = spawn_proxy("crates.io").await;
        let mut stream = TcpStream::connect(&addr).await.unwrap();

        stream
            .write_all(b"CONNECT evil.com:443 HTTP/1.1\r\nHost: evil.com:443\r\n\r\n")
            .await
            .unwrap();

        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(
            response.contains("403 Forbidden"),
            "expected 403, got: {response}"
        );
    }

    #[tokio::test]
    async fn proxy_denies_with_empty_allowlist() {
        let addr = spawn_proxy("").await;
        let mut stream = TcpStream::connect(&addr).await.unwrap();

        stream
            .write_all(b"CONNECT crates.io:443 HTTP/1.1\r\nHost: crates.io:443\r\n\r\n")
            .await
            .unwrap();

        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(
            response.contains("403 Forbidden"),
            "expected 403, got: {response}"
        );
    }

    #[tokio::test]
    async fn proxy_rejects_non_connect() {
        let addr = spawn_proxy("crates.io").await;
        let mut stream = TcpStream::connect(&addr).await.unwrap();

        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: crates.io\r\n\r\n")
            .await
            .unwrap();

        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(
            response.contains("405 Method Not Allowed"),
            "expected 405, got: {response}"
        );
    }

    #[tokio::test]
    async fn proxy_rejects_oversized_request_line() {
        let addr = spawn_proxy("crates.io").await;
        let mut stream = TcpStream::connect(&addr).await.unwrap();

        // A request line far larger than MAX_REQUEST_LINE_BYTES, with the newline
        // pushed past the cap — the proxy must reject it rather than buffer the
        // line without bound.
        let big = "a".repeat(16 * 1024);
        let req = format!("CONNECT {big}:443 HTTP/1.1\r\n\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();

        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(
            response.contains("413"),
            "expected 413 for oversized request line, got: {response}"
        );
    }

    #[tokio::test]
    async fn proxy_allows_listed_domain() {
        // Spin up a fake "upstream" server so the proxy can connect to it.
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let upstream_host = format!("127.0.0.1:{}", upstream_addr.port());

        tokio::spawn(async move {
            if let Ok((mut stream, _)) = upstream.accept().await {
                let mut buf = vec![0u8; 64];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                // Echo back what we received.
                let _ = stream.write_all(&buf[..n]).await;
            }
        });

        // Allowlist 127.0.0.1 so the proxy allows connecting to our upstream.
        let addr = spawn_proxy("127.0.0.1").await;
        let mut stream = TcpStream::connect(&addr).await.unwrap();

        let connect_req =
            format!("CONNECT {upstream_host} HTTP/1.1\r\nHost: {upstream_host}\r\n\r\n");
        stream.write_all(connect_req.as_bytes()).await.unwrap();

        // Read the 200 response.
        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(
            response.contains("200 Connection Established"),
            "expected 200, got: {response}"
        );

        // The tunnel is open — send data through and get echo.
        stream.write_all(b"hello").await.unwrap();
        let mut echo_buf = vec![0u8; 64];
        let n = stream.read(&mut echo_buf).await.unwrap();
        assert_eq!(&echo_buf[..n], b"hello");
    }
}
