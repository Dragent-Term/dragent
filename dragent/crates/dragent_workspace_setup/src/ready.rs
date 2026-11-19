//! Readiness probes for launch profiles: `{"port": N}` or `{"url": ...}`.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::config::ReadyCheck;

/// What the probe found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadyResult {
    pub ready: bool,
    pub detail: String,
}

/// Wait up to the check's timeout for the service to answer.
pub fn wait_for_ready(check: &ReadyCheck) -> ReadyResult {
    let timeout = Duration::from_secs(check.timeout_secs_or_default());
    let deadline = Instant::now() + timeout;
    if let Some(port) = check.port {
        let target = format!("127.0.0.1:{port}");
        loop {
            if TcpStream::connect(&target).is_ok() {
                return ReadyResult {
                    ready: true,
                    detail: format!("port {port} accepted a connection"),
                };
            }
            if Instant::now() >= deadline {
                return ReadyResult {
                    ready: false,
                    detail: format!(
                        "port {port} did not accept a connection within {}s",
                        check.timeout_secs_or_default()
                    ),
                };
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    if let Some(url) = &check.url {
        loop {
            match probe_url(url) {
                Ok(detail) => {
                    return ReadyResult {
                        ready: true,
                        detail,
                    }
                }
                Err(message) => {
                    if Instant::now() >= deadline {
                        return ReadyResult {
                            ready: false,
                            detail: format!(
                                "{message} (within {}s)",
                                check.timeout_secs_or_default()
                            ),
                        };
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
    }
    ReadyResult {
        ready: false,
        detail: "ready needs port or url".into(),
    }
}

/// Minimal HTTP GET over a plain TCP (or TLS-looking) connection.
///
/// Only `http://` is probed at the socket level; `https://` does a TCP
/// handshake to the host:port (default 443) without cert validation, which
/// is enough to know the server is listening. A 2xx/3xx status counts as
/// ready; anything else is retried until the timeout.
fn probe_url(url: &str) -> Result<String, String> {
    let (host, port, path, secure) = parse_url(url)?;
    let addrs: Vec<SocketAddr> = format!("{host}:{port}")
        .to_socket_addrs()
        .map_err(|error| format!("could not resolve {host}: {error}"))?
        .collect();
    let addr = addrs
        .into_iter()
        .next()
        .ok_or_else(|| format!("could not resolve {host}"))?;
    if secure {
        // TLS handshake without validation: only proves something listens.
        // A full HTTPS GET needs a TLS stack the crate deliberately avoids;
        // report TCP readiness for https URLs.
        match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
            Ok(_) => Ok(format!("{host}:{port} accepted a connection")),
            Err(error) => Err(format!("{url} not ready: {error}")),
        }
    } else {
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
            .map_err(|error| format!("{url} not ready: {error}"))?;
        stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
        let request = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        stream
            .write_all(request.as_bytes())
            .map_err(|error| format!("{url} not ready: {error}"))?;
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .map_err(|error| format!("{url} not ready: {error}"))?;
        let status = response.lines().next().unwrap_or_default().to_string();
        if status.contains(" 2") || status.contains(" 3") {
            Ok(format!("{url} answered: {status}"))
        } else if status.is_empty() {
            Err(format!("{url} not ready: empty response"))
        } else {
            Err(format!("{url} not ready: {status}"))
        }
    }
}

fn parse_url(url: &str) -> Result<(String, u16, String, bool), String> {
    let (secure, rest) = if let Some(rest) = url.strip_prefix("http://") {
        (false, rest)
    } else if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else {
        return Err(format!("url must start with http:// or https://"));
    };
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], rest[index..].to_string()),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(port) => (host.to_string(), port),
            Err(_) => (authority.to_string(), if secure { 443 } else { 80 }),
        },
        None => (authority.to_string(), if secure { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err("url has no host".into());
    }
    Ok((host, port, path, secure))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_probe_times_out_fast_when_nothing_listens() {
        // Port 1 is privileged and (almost) never open for tests.
        let check = ReadyCheck {
            port: Some(1),
            url: None,
            timeout_secs: Some(1),
        };
        let started = Instant::now();
        let result = wait_for_ready(&check);
        assert!(!result.ready);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn port_probe_sees_a_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let _ = stream;
            }
        });
        let check = ReadyCheck {
            port: Some(port),
            url: None,
            timeout_secs: Some(5),
        };
        let result = wait_for_ready(&check);
        assert!(result.ready, "{}", result.detail);
    }

    #[test]
    fn url_probe_sees_http_ok() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut buf);
                let _ = stream.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nok");
            }
        });
        let check = ReadyCheck {
            port: None,
            url: Some(format!("http://127.0.0.1:{port}/health")),
            timeout_secs: Some(5),
        };
        let result = wait_for_ready(&check);
        assert!(result.ready, "{}", result.detail);
    }
}
