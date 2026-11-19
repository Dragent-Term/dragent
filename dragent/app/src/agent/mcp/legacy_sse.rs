//! Legacy MCP SSE transport for servers that predate the Streamable HTTP protocol.
//!
//! Protocol:
//!   1. GET {url} with `Accept: text/event-stream`
//!   2. Server sends `event: endpoint` with the URL to POST messages to
//!   3. Client POSTs JSON-RPC messages; server sends JSON-RPC responses via the SSE stream

use futures::channel::mpsc;
use futures::{StreamExt, TryStreamExt};
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::sink_stream::SinkStreamTransport;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::io::StreamReader;

/// Connect to a legacy MCP SSE server.
///
/// Returns a `SinkStreamTransport` that rmcp's `().serve(transport)` can drive.
/// Two background tasks run for the lifetime of the transport:
///   - An SSE reader that forwards server→client messages
///   - A POST sender that forwards client→server messages
pub async fn connect(
    sse_url: &str,
    auth_token: Option<String>,
) -> anyhow::Result<
    SinkStreamTransport<mpsc::Sender<ClientJsonRpcMessage>, mpsc::Receiver<ServerJsonRpcMessage>>,
> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    // Open the SSE stream.
    let mut builder = client
        .get(sse_url)
        .header("Accept", "text/event-stream")
        .header("Cache-Control", "no-cache");
    if let Some(tok) = &auth_token {
        builder = builder.bearer_auth(tok);
    }
    let response = builder
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("SSE connect: {e}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        anyhow::bail!("SSE endpoint requires authentication (401)");
    }
    if !status.is_success() {
        anyhow::bail!("SSE endpoint returned {status}");
    }

    // Convert reqwest bytes stream → AsyncBufRead
    let byte_stream = response.bytes_stream().map_err(std::io::Error::other);
    let reader = BufReader::new(StreamReader::new(byte_stream));

    // Read SSE events until we find `event: endpoint` (10 s timeout).
    let (endpoint_url, reader) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        read_endpoint_event(reader, sse_url),
    )
    .await
    .map_err(|_| anyhow::anyhow!("timeout: no SSE endpoint event within 10 s"))?
    .map_err(|e| anyhow::anyhow!("SSE endpoint event error: {e}"))?;

    log::info!("[mcp sse] endpoint: {endpoint_url}");

    let (server_tx, server_rx) = mpsc::channel::<ServerJsonRpcMessage>(64);
    let (client_tx, client_rx) = mpsc::channel::<ClientJsonRpcMessage>(64);

    // Task 1: SSE reader → server_tx
    let sse_client = client.clone();
    let sse_url_owned = sse_url.to_string();
    let endpoint_url_c = endpoint_url.clone();
    let auth_c = auth_token.clone();
    tokio::spawn(sse_read_loop(
        reader,
        server_tx,
        sse_url_owned,
        endpoint_url_c,
        sse_client,
        auth_c,
    ));

    // Task 2: client_rx → POST
    tokio::spawn(post_send_loop(client_rx, client, endpoint_url, auth_token));

    Ok(SinkStreamTransport::new(client_tx, server_rx))
}

/// Read SSE lines until we find `event: endpoint` and extract the POST URL.
/// Returns `(endpoint_url, reader)` positioned immediately after the endpoint event.
async fn read_endpoint_event<R>(
    mut reader: BufReader<R>,
    base_url: &str,
) -> anyhow::Result<(String, BufReader<R>)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = String::new();
    let mut current_event: Option<String> = None;
    let mut current_data = String::new();

    loop {
        buf.clear();
        let n = reader
            .read_line(&mut buf)
            .await
            .map_err(|e| anyhow::anyhow!("SSE read: {e}"))?;
        if n == 0 {
            anyhow::bail!("SSE stream closed before endpoint event");
        }

        let line = buf.trim_end_matches('\n').trim_end_matches('\r');

        if line.is_empty() {
            // Blank line = dispatch event
            if current_event.as_deref() == Some("endpoint") && !current_data.is_empty() {
                let url = resolve_url(base_url, current_data.trim());
                return Ok((url, reader));
            }
            current_event = None;
            current_data.clear();
        } else if let Some(val) = line.strip_prefix("event:") {
            current_event = Some(val.trim_start().to_string());
        } else if let Some(val) = line.strip_prefix("data:") {
            if !current_data.is_empty() {
                current_data.push('\n');
            }
            current_data.push_str(val.trim_start());
        }
    }
}

/// Forward SSE events to `tx` as `ServerJsonRpcMessage`.
/// Reconnects if the stream drops (up to 5 times with 1 s backoff).
async fn sse_read_loop<R>(
    mut reader: BufReader<R>,
    mut tx: mpsc::Sender<ServerJsonRpcMessage>,
    sse_url: String,
    _endpoint_url: String,
    http_client: reqwest::Client,
    auth_token: Option<String>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    if !run_sse_lines(&mut reader, &mut tx).await {
        return; // receiver dropped
    }

    // Stream dropped — attempt auto-reconnect up to 5 times
    for attempt in 1..=5u32 {
        tokio::time::sleep(std::time::Duration::from_millis(1000 * u64::from(attempt))).await;
        log::info!("[mcp sse] reconnecting SSE stream (attempt {attempt})");

        match reconnect_sse_stream(&sse_url, auth_token.as_deref(), &http_client).await {
            Ok((new_endpoint, new_reader)) => {
                log::info!("[mcp sse] reconnected, endpoint: {new_endpoint}");
                let mut boxed: Box<dyn tokio::io::AsyncRead + Unpin + Send> = Box::new(new_reader);
                let mut new_buf_reader = BufReader::new(&mut *boxed);
                if !run_sse_lines(&mut new_buf_reader, &mut tx).await {
                    return;
                }
            }
            Err(e) => {
                log::warn!("[mcp sse] reconnect attempt {attempt} failed: {e}");
            }
        }
    }
    log::warn!("[mcp sse] SSE stream lost after 5 reconnect attempts");
}

async fn reconnect_sse_stream(
    url: &str,
    auth_token: Option<&str>,
    client: &reqwest::Client,
) -> anyhow::Result<(String, impl tokio::io::AsyncRead + Unpin + Send + 'static)> {
    let mut builder = client
        .get(url)
        .header("Accept", "text/event-stream")
        .header("Cache-Control", "no-cache");
    if let Some(tok) = auth_token {
        builder = builder.bearer_auth(tok);
    }
    let response = builder.send().await?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("SSE reconnect returned {status}");
    }
    let byte_stream = response.bytes_stream().map_err(std::io::Error::other);
    let reader = BufReader::new(StreamReader::new(byte_stream));
    read_endpoint_event(reader, url).await
}

/// Parse SSE lines from `reader`, send complete message-events to `tx`.
/// Returns `true` if the stream terminated normally (EOF), `false` if `tx` closed.
async fn run_sse_lines<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    tx: &mut mpsc::Sender<ServerJsonRpcMessage>,
) -> bool {
    let mut buf = String::new();
    let mut current_event: Option<String> = None;
    let mut current_data = String::new();

    loop {
        buf.clear();
        match reader.read_line(&mut buf).await {
            Ok(0) => return true, // EOF — caller decides whether to reconnect
            Err(e) => {
                log::warn!("[mcp sse] read error: {e}");
                return true; // let caller reconnect
            }
            Ok(_) => {}
        }

        let line = buf.trim_end_matches('\n').trim_end_matches('\r');

        if line.is_empty() {
            // Dispatch event
            let is_message = matches!(current_event.as_deref(), None | Some("") | Some("message"));
            if is_message && !current_data.is_empty() {
                match serde_json::from_str::<ServerJsonRpcMessage>(&current_data) {
                    Ok(msg) => {
                        if tx.try_send(msg).is_err() {
                            // Channel full or closed
                            if tx.is_closed() {
                                return false; // transport dropped
                            }
                        }
                    }
                    Err(e) => {
                        log::debug!("[mcp sse] JSON parse error: {e}: {}", &current_data);
                    }
                }
            }
            current_event = None;
            current_data.clear();
        } else if let Some(val) = line.strip_prefix("event:") {
            current_event = Some(val.trim_start().to_string());
        } else if let Some(val) = line.strip_prefix("data:") {
            if !current_data.is_empty() {
                current_data.push('\n');
            }
            current_data.push_str(val.trim_start());
        }
        // Ignore `id:` and `retry:` for now
    }
}

/// POST outbound JSON-RPC messages from `rx` to `endpoint`.
async fn post_send_loop(
    mut rx: mpsc::Receiver<ClientJsonRpcMessage>,
    client: reqwest::Client,
    endpoint: String,
    auth_token: Option<String>,
) {
    while let Some(msg) = rx.next().await {
        let mut builder = client.post(&endpoint).json(&msg);
        if let Some(tok) = &auth_token {
            builder = builder.bearer_auth(tok);
        }
        match builder.send().await {
            Ok(r) if !r.status().is_success() => {
                log::warn!("[mcp sse] POST returned {}", r.status());
            }
            Err(e) => {
                log::warn!("[mcp sse] POST failed: {e}");
            }
            Ok(_) => {}
        }
    }
}

/// Resolve a possibly-relative URL from the SSE `endpoint` event against the origin of `base`.
fn resolve_url(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    if let Ok(parsed) = url::Url::parse(base) {
        let origin = match parsed.port() {
            Some(p) => format!(
                "{}://{}:{}",
                parsed.scheme(),
                parsed.host_str().unwrap_or(""),
                p
            ),
            None => format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap_or("")),
        };
        if path.starts_with('/') {
            return format!("{origin}{path}");
        }
        if let Some(base_dir) = parsed.path().rsplit_once('/').map(|(p, _)| p) {
            return format!("{origin}{base_dir}/{path}");
        }
        return format!("{origin}/{path}");
    }
    path.to_string()
}
