//! Safe loopback connector used by shared-session port forwarding.
//!
//! This module deliberately accepts only a port, never an arbitrary host. A
//! forwarded request therefore cannot be turned into an SSRF primitive against
//! the host machine's LAN, cloud metadata endpoint, or Unix socket.

use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_REQUEST: usize = 8 * 1024 * 1024;
const MAX_RESPONSE: usize = 16 * 1024 * 1024;

/// Headers that describe the public gateway hop rather than the local service.
/// They must never be replayed into a loopback development server: Vite uses
/// forwarded host information when enforcing its host allow-list.
fn is_gateway_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("forwarded")
        || name
            .get(..12)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("x-forwarded-"))
        || name.eq_ignore_ascii_case("x-real-ip")
}

fn is_upgrade_request(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .any(|(name, value)| name.eq_ignore_ascii_case("upgrade") && !value.trim().is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[cfg(not(target_family = "wasm"))]
pub type LocalStream = TcpStream;

/// Open the local TCP endpoint and write the browser's original upgrade
/// request. The desktop must not terminate and recreate the browser's
/// WebSocket handshake; it relays the connection as bytes.
#[cfg(not(target_family = "wasm"))]
pub fn stream(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<LocalStream, String> {
    if port == 0 || !path.starts_with('/') || path.contains(['\r', '\n']) {
        return Err("invalid forwarded stream target".into());
    }
    if !matches!(
        method,
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
    ) {
        return Err("invalid forwarded stream method".into());
    }
    if body.len() > MAX_REQUEST {
        return Err("forwarded stream request is too large".into());
    }
    let addresses: Vec<_> = ("localhost", port)
        .to_socket_addrs()
        .map_err(|e| format!("localhost resolution failed: {e}"))?
        .filter(|addr| addr.ip().is_loopback())
        .collect();
    let mut last_error = None;
    let mut socket = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(connected) => {
                socket = Some(connected);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let mut socket = socket.ok_or_else(|| {
        format!(
            "loopback connect failed: {}",
            last_error
                .map(|e| e.to_string())
                .unwrap_or_else(|| "connection refused".into())
        )
    })?;
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|e| format!("set stream read timeout: {e}"))?;
    socket
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|e| format!("set stream write timeout: {e}"))?;

    let is_upgrade = is_upgrade_request(headers);
    // A public forward is a reverse proxy, not the local service's real
    // origin. Keep browser context headers consistent with the loopback Host:
    // Next.js validates its development/HMR WebSocket Origin against Host and
    // otherwise replies with a bare `Unauthorized` body.
    let local_origin = format!("http://localhost:{port}");
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: localhost:{port}\r\n");
    let mut has_content_length = false;
    for (name, value) in headers {
        if name.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
            return Err("invalid forwarded websocket header".into());
        }
        if name.eq_ignore_ascii_case("host")
            || name.eq_ignore_ascii_case("transfer-encoding")
            || name.eq_ignore_ascii_case("proxy-connection")
            || (!is_upgrade && name.eq_ignore_ascii_case("connection"))
            || is_gateway_header(name)
        {
            continue;
        }
        if name.eq_ignore_ascii_case("origin") {
            request.push_str("Origin: ");
            request.push_str(&local_origin);
            request.push_str("\r\n");
            continue;
        }
        if name.eq_ignore_ascii_case("referer") {
            request.push_str("Referer: ");
            request.push_str(&local_origin);
            request.push_str("/\r\n");
            continue;
        }
        if name.eq_ignore_ascii_case("content-length") {
            has_content_length = true;
        }
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    if !body.is_empty() && !has_content_length {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    // The server relays the first HTTP request as structured data, but later
    // bytes on a persistent browser connection are intentionally raw (needed
    // for WebSockets). Reusing a normal HTTP/1.1 connection would therefore
    // bypass our Host rewrite for the next request. Close non-upgrade requests
    // after their response so every browser request gets its own rewritten
    // tunnel stream. WebSocket/HMR upgrades keep their native duplex socket.
    if !is_upgrade {
        request.push_str("Connection: close\r\n");
    }
    request.push_str("\r\n");
    socket
        .write_all(request.as_bytes())
        .and_then(|_| socket.write_all(body))
        .map_err(|e| format!("write loopback stream request failed: {e}"))?;
    Ok(socket)
}

#[cfg(not(target_family = "wasm"))]
pub fn stream_read(socket: &mut LocalStream) -> Result<Option<Vec<u8>>, String> {
    let mut buffer = vec![0u8; 32 * 1024];
    match socket.read(&mut buffer) {
        Ok(0) => Err("loopback stream closed".into()),
        Ok(size) => {
            buffer.truncate(size);
            Ok(Some(buffer))
        }
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            Ok(None)
        }
        Err(error) => Err(format!("loopback stream read failed: {error}")),
    }
}

#[cfg(not(target_family = "wasm"))]
pub fn stream_write(socket: &mut LocalStream, data: &[u8]) -> Result<(), String> {
    socket
        .write_all(data)
        .and_then(|_| socket.flush())
        .map_err(|e| format!("loopback stream write failed: {e}"))
}

/// Perform one bounded HTTP/1.1 request against 127.0.0.1:`port`.
///
/// The first implementation intentionally uses `Connection: close`. It keeps
/// stream ownership simple while the multiplexed tunnel is still text-frame
/// based; persistent connections and raw TCP can be added without weakening
/// the loopback policy.
pub fn request(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<Response, String> {
    if port == 0 {
        return Err("forward port must be non-zero".into());
    }
    if !matches!(
        method,
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
    ) {
        return Err("unsupported forwarded method".into());
    }
    if !path.starts_with('/') || path.contains('\r') || path.contains('\n') {
        return Err("invalid forwarded path".into());
    }
    if body.len() > MAX_REQUEST {
        return Err("forwarded request is too large".into());
    }

    // `localhost` is commonly IPv6-only on macOS/Linux dev setups. Resolve it
    // through the OS and try every loopback result, while explicitly rejecting
    // any non-loopback address that a custom hosts file might provide.
    let addresses: Vec<_> = ("localhost", port)
        .to_socket_addrs()
        .map_err(|e| format!("localhost resolution failed: {e}"))?
        .filter(|addr| match addr.ip() {
            IpAddr::V4(ip) => ip.is_loopback(),
            IpAddr::V6(ip) => ip.is_loopback(),
        })
        .collect();
    if addresses.is_empty() {
        return Err("localhost did not resolve to a loopback address".into());
    }
    let mut last_error = None;
    let mut stream = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(connected) => {
                stream = Some(connected);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let mut stream = stream.ok_or_else(|| {
        format!(
            "loopback connect failed: {}",
            last_error
                .map(|e| e.to_string())
                .unwrap_or_else(|| "connection refused".into())
        )
    })?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|e| format!("set read timeout: {e}"))?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|e| format!("set write timeout: {e}"))?;

    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (name, value) in headers {
        if name.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
            return Err("invalid forwarded header".into());
        }
        let lower = name.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "host" | "connection" | "content-length" | "transfer-encoding"
        ) || is_gateway_header(name)
        {
            continue;
        }
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    if request.len() + body.len() > MAX_REQUEST {
        return Err("forwarded request is too large".into());
    }
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write request failed: {e}"))?;
    stream
        .write_all(body)
        .map_err(|e| format!("write body failed: {e}"))?;
    read_response(&mut stream)
}

/// Read one HTTP response without waiting for the local server to close its
/// keep-alive connection. Dev servers such as Vite normally provide a
/// Content-Length and leave the socket open for the next request.
fn read_response(stream: &mut TcpStream) -> Result<Response, String> {
    let mut raw = Vec::with_capacity(16 * 1024);
    loop {
        let body_start = loop {
            if let Some((_, delimiter)) = response_header_end(&raw) {
                break parse_head(&raw)?.2;
            }
            let mut chunk = [0u8; 32 * 1024];
            let size = stream
                .read(&mut chunk)
                .map_err(|e| format!("read response headers failed: {e}"))?;
            if size == 0 {
                return Err("local service closed before sending HTTP headers".into());
            }
            raw.extend_from_slice(&chunk[..size]);
            if raw.len() > MAX_RESPONSE {
                return Err("forwarded response is too large".into());
            }
        };
        let (status, headers, _) = parse_head(&raw)?;
        if status == 100 {
            raw.drain(..body_start);
            continue;
        }

        let body_len = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.parse::<usize>().ok());
        let chunked = headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
        });

        if let Some(body_len) = body_len {
            let required = body_start
                .checked_add(body_len)
                .ok_or("forwarded response is too large")?;
            while raw.len() < required {
                let mut chunk = [0u8; 32 * 1024];
                let size = stream
                    .read(&mut chunk)
                    .map_err(|e| format!("read response body failed: {e}"))?;
                if size == 0 {
                    return Err("local service closed before response body completed".into());
                }
                raw.extend_from_slice(&chunk[..size]);
                if raw.len() > MAX_RESPONSE {
                    return Err("forwarded response is too large".into());
                }
            }
            raw.truncate(required);
            return parse_response(&raw);
        }

        if chunked {
            loop {
                if decode_chunked(&raw[body_start..]).is_ok() {
                    return parse_response(&raw);
                }
                let mut chunk = [0u8; 32 * 1024];
                let size = stream
                    .read(&mut chunk)
                    .map_err(|e| format!("read chunked response failed: {e}"))?;
                if size == 0 {
                    return Err("local service closed before chunked response completed".into());
                }
                raw.extend_from_slice(&chunk[..size]);
                if raw.len() > MAX_RESPONSE {
                    return Err("forwarded response is too large".into());
                }
            }
        }

        stream
            .read_to_end(&mut raw)
            .map_err(|e| format!("read response failed: {e}"))?;
        if raw.len() > MAX_RESPONSE {
            return Err("forwarded response is too large".into());
        }
        return parse_response(&raw);
    }
}

fn response_header_end(raw: &[u8]) -> Option<(usize, usize)> {
    let crlf = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| (position, 4));
    let lf = raw
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|position| (position, 2));
    match (crlf, lf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn parse_response(raw: &[u8]) -> Result<Response, String> {
    if raw.is_empty() {
        return Err("local service returned an empty HTTP response".into());
    }
    let mut offset = 0;
    loop {
        let (status, headers, body_start) = parse_head(&raw[offset..])?;
        offset += body_start;
        // Proxies and dev servers may send an interim 100 response before the
        // real response. Ignore it and parse the next response head.
        if status == 100 {
            continue;
        }
        let mut body = raw[offset..].to_vec();
        if headers.iter().any(|(n, v)| {
            n.eq_ignore_ascii_case("transfer-encoding")
                && v.to_ascii_lowercase().contains("chunked")
        }) {
            body = decode_chunked(&body)?;
        }
        return Ok(Response {
            status,
            headers,
            body,
        });
    }
}

fn parse_head(raw: &[u8]) -> Result<(u16, Vec<(String, String)>, usize), String> {
    let crlf = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4));
    let lf = raw.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2));
    let (split, delimiter) = match (crlf, lf) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return Err("malformed HTTP response: headers were not terminated".into()),
    };
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| "response headers are not UTF-8")?;
    let mut lines = head.split('\n');
    let status = lines
        .next()
        .map(str::trim)
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or("malformed HTTP status")?;
    let mut headers = Vec::new();
    for line in lines {
        let line = line.trim_end_matches('\r');
        let Some((name, value)) = line.split_once(':') else {
            return Err("malformed HTTP header".into());
        };
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }
    Ok((status, headers, split + delimiter))
}

fn decode_chunked(mut raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoded = Vec::new();
    loop {
        let Some(end) = raw.windows(2).position(|w| w == b"\r\n") else {
            return Err("malformed chunked HTTP response".into());
        };
        let size = usize::from_str_radix(
            std::str::from_utf8(&raw[..end])
                .map_err(|_| "invalid chunk size")?
                .trim(),
            16,
        )
        .map_err(|_| "invalid chunk size")?;
        raw = &raw[end + 2..];
        if size == 0 {
            return Ok(decoded);
        }
        if raw.len() < size + 2 || &raw[size..size + 2] != b"\r\n" {
            return Err("malformed chunked HTTP response".into());
        }
        decoded.extend_from_slice(&raw[..size]);
        raw = &raw[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_loopback_inputs() {
        assert!(request(0, "GET", "/", &[], &[]).is_err());
        assert!(request(80, "GET", "http://evil/", &[], &[]).is_err());
        assert!(request(80, "CONNECT", "/", &[], &[]).is_err());
    }

    #[test]
    fn stream_rejects_invalid_target() {
        assert!(stream(0, "GET", "/", &[], &[]).is_err());
        assert!(stream(80, "GET", "http://evil/", &[], &[]).is_err());
        assert!(stream(80, "GET", "/bad\r\n", &[], &[]).is_err());
    }

    #[test]
    fn stream_replays_upgrade_request_as_raw_http() {
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            socket
                .write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n")
                .unwrap();
            String::from_utf8(request).unwrap()
        });

        let _client = stream(
            port,
            "GET",
            "/@vite/client?token=abc",
            &[
                ("Connection".into(), "Upgrade".into()),
                ("Upgrade".into(), "websocket".into()),
                ("Sec-WebSocket-Key".into(), "test-key".into()),
                ("Sec-WebSocket-Protocol".into(), "vite-hmr".into()),
                (
                    "Forwarded".into(),
                    "host=public.example.test;proto=https".into(),
                ),
                ("X-Forwarded-Host".into(), "public.example.test".into()),
                ("X-Real-IP".into(), "203.0.113.1".into()),
                ("Origin".into(), "https://public.example.test".into()),
                (
                    "Referer".into(),
                    "https://public.example.test/dashboard".into(),
                ),
            ],
            &[],
        )
        .unwrap();
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /@vite/client?token=abc HTTP/1.1\r\n"));
        assert!(request.contains("Host: localhost:"));
        assert!(request.contains("Sec-WebSocket-Key: test-key\r\n"));
        assert!(request.contains("Sec-WebSocket-Protocol: vite-hmr\r\n"));
        assert!(request.contains("Connection: Upgrade\r\n"));
        assert!(!request.contains("Connection: close\r\n"));
        assert!(!request.contains("Forwarded:"));
        assert!(!request.contains("X-Forwarded-Host:"));
        assert!(!request.contains("X-Real-IP:"));
        assert!(request.contains(&format!("Origin: http://localhost:{port}\r\n")));
        assert!(request.contains(&format!("Referer: http://localhost:{port}/\r\n")));
    }

    #[test]
    fn stream_closes_regular_http_connections_after_one_request() {
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            String::from_utf8(request).unwrap()
        });

        let _client = stream(
            port,
            "GET",
            "/src/main.tsx",
            &[
                ("Connection".into(), "keep-alive".into()),
                ("X-Forwarded-Host".into(), "public.example.test".into()),
            ],
            &[],
        )
        .unwrap();
        let request = server.join().unwrap();
        assert!(request.contains("Host: localhost:"));
        assert!(request.contains("Connection: close\r\n"));
        assert!(!request.contains("Connection: keep-alive\r\n"));
        assert!(!request.contains("X-Forwarded-Host:"));
    }

    #[test]
    fn request_returns_content_length_response_without_waiting_for_close() {
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok",
                )
                .unwrap();
            thread::sleep(Duration::from_secs(1));
        });

        let response = request(port, "GET", "/iconsax-react.js", &[], &[]).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"ok");
        server.join().unwrap();
    }

    #[test]
    fn parses_response() {
        let got =
            parse_response(b"HTTP/1.1 201 Created\r\nContent-Type: text/plain\r\n\r\nok").unwrap();
        assert_eq!(got.status, 201);
        assert_eq!(got.body, b"ok");
    }

    #[test]
    fn parses_lf_only_and_chunked_responses() {
        let got =
            parse_response(b"HTTP/1.1 200 OK\nTransfer-Encoding: chunked\n\n2\r\nok\r\n0\r\n\r\n")
                .unwrap();
        assert_eq!(got.body, b"ok");
    }
}
