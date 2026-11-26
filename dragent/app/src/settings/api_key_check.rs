//! Asks a provider whether an API key works before it is saved. The call goes
//! straight from this computer to the provider; the key never reaches
//! Dragent's servers, and nothing here logs it.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KeyCheck {
    Works,
    /// The provider said this key is not valid.
    Rejected,
    /// No answer either way: offline, or the provider is having trouble.
    Unknown,
}

/// The company a provider's API keys come from.
pub(crate) fn key_issuer(provider: &str) -> &'static str {
    match provider {
        "codex" => "OpenAI",
        _ => "Anthropic",
    }
}

/// Blocking; run it off the UI thread.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn check(provider: &str, key: &str) -> KeyCheck {
    match provider {
        "codex" => check_at(provider, "https://api.openai.com", key),
        _ => check_at(provider, "https://api.anthropic.com", key),
    }
}

#[cfg(target_family = "wasm")]
pub(crate) fn check(_: &str, _: &str) -> KeyCheck {
    KeyCheck::Unknown
}

#[cfg(not(target_family = "wasm"))]
fn check_at(provider: &str, base: &str, key: &str) -> KeyCheck {
    let request = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(15))
        // Never carry the key to another address.
        .redirects(0)
        .build()
        .get(&format!("{base}/v1/models"));
    let request = match provider {
        "codex" => request.set("Authorization", &format!("Bearer {key}")),
        _ => request
            .set("x-api-key", key)
            .set("anthropic-version", "2023-06-01"),
    };
    match request.call() {
        Ok(response) if response.status() == 200 => KeyCheck::Works,
        Err(ureq::Error::Status(401, _)) => KeyCheck::Rejected,
        // A real key that may not list models, or is over its rate limit.
        Err(ureq::Error::Status(403 | 429, _)) => KeyCheck::Works,
        Ok(_) | Err(_) => KeyCheck::Unknown,
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// One answer from a local server; returns the request it saw.
    fn answer(status: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut seen = Vec::new();
            let mut chunk = [0; 1024];
            while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                seen.extend_from_slice(&chunk[..n]);
            }
            write!(
                stream,
                "HTTP/1.1 {status}\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            )
            .unwrap();
            String::from_utf8_lossy(&seen).to_string()
        });
        (base, handle)
    }

    #[test]
    fn each_answer_means_what_it_should() {
        for (status, expected) in [
            ("200 OK", KeyCheck::Works),
            ("401 Unauthorized", KeyCheck::Rejected),
            ("403 Forbidden", KeyCheck::Works),
            ("429 Too Many Requests", KeyCheck::Works),
            ("500 Internal Server Error", KeyCheck::Unknown),
            ("302 Found", KeyCheck::Unknown),
        ] {
            let (base, server) = answer(status);
            assert_eq!(check_at("claude", &base, "made-up"), expected, "{status}");
            server.join().unwrap();
        }
        assert_eq!(check_at("claude", "http://127.0.0.1:1", "made-up"), KeyCheck::Unknown);
    }

    #[test]
    fn each_provider_gets_the_key_its_own_way() {
        let (base, server) = answer("200 OK");
        check_at("claude", &base, "made-up");
        assert!(server.join().unwrap().to_lowercase().contains("x-api-key: made-up"));
        let (base, server) = answer("200 OK");
        check_at("codex", &base, "made-up");
        assert!(server.join().unwrap().contains("Authorization: Bearer made-up"));
    }
}
