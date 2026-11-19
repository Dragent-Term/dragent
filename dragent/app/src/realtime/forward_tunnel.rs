//! Dedicated binary data plane for public port forwarding.
//!
//! Terminal/session sharing stays on the JSON control socket. This worker owns
//! a separate authenticated WebSocket and talks directly to loopback TCP
//! sockets, so a cold frontend module burst never has to pass through the UI
//! actor or the shared-session message bus.

#[cfg(not(target_family = "wasm"))]
mod native {
    use serde::Deserialize;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::{Duration, Instant};

    const VERSION: u8 = 1;
    const OPEN: u8 = 1;
    const OPENED: u8 = 2;
    const DATA: u8 = 3;
    const FIN: u8 = 4;
    const RESET: u8 = 5;
    const HEADER_LEN: usize = 14;
    const READ_TIMEOUT: Duration = Duration::from_millis(20);

    pub struct ForwardTunnelHandle {
        stop: Arc<AtomicBool>,
    }

    impl ForwardTunnelHandle {
        pub fn stop(&self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    impl Drop for ForwardTunnelHandle {
        fn drop(&mut self) {
            self.stop();
        }
    }

    #[derive(Deserialize)]
    struct OpenMetadata {
        port: u16,
        method: String,
        path: String,
        headers: Vec<(String, String)>,
    }

    enum StreamCommand {
        Data(Vec<u8>),
        Fin,
    }

    type StreamMap = Arc<Mutex<HashMap<u64, mpsc::Sender<StreamCommand>>>>;

    pub fn start(device_id: String) -> ForwardTunnelHandle {
        let stop = Arc::new(AtomicBool::new(false));
        // An empty backend is a permanent build/runtime configuration issue,
        // not a transient network failure. Do not start an authentication and
        // reconnect loop that can never produce a valid WebSocket URL.
        if crate::api::backend_url().is_empty() {
            log::debug!("[forward-tunnel] disabled: no backend URL configured");
            return ForwardTunnelHandle { stop };
        }
        let thread_stop = stop.clone();
        let _ = std::thread::Builder::new()
            .name("forward-tunnel".into())
            .spawn(move || run(device_id, thread_stop));
        ForwardTunnelHandle { stop }
    }

    fn run(device_id: String, stop: Arc<AtomicBool>) {
        let url = format!("{}/forward?device={device_id}", super::super::gateway_url());
        let (outbound_tx, outbound_rx) = async_channel::unbounded::<Vec<u8>>();
        let streams: StreamMap = Arc::new(Mutex::new(HashMap::new()));
        let mut backoff = Duration::from_millis(250);

        while !stop.load(Ordering::Relaxed) {
            let connected_at = Instant::now();
            let result =
                connect_and_serve(&url, &stop, &outbound_tx, &outbound_rx, streams.clone());
            let was_stable = connected_at.elapsed() >= Duration::from_secs(30);
            close_streams(&streams);
            while outbound_rx.try_recv().is_ok() {}
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if let Err(error) = result {
                log::warn!("[forward-tunnel] {error}");
            }
            // A healthy long-lived connection should recover quickly from a
            // later network handoff. Do not retain a 5-second backoff merely
            // because this worker experienced transient failures hours ago.
            if was_stable {
                backoff = Duration::from_millis(250);
            }
            sleep_interruptible(backoff, &stop);
            if !was_stable {
                backoff = (backoff * 2).min(Duration::from_secs(5));
            }
        }
    }

    fn connect_and_serve(
        url: &str,
        stop: &AtomicBool,
        outbound_tx: &async_channel::Sender<Vec<u8>>,
        outbound_rx: &async_channel::Receiver<Vec<u8>>,
        streams: StreamMap,
    ) -> Result<(), String> {
        let token = crate::api::API_CLIENT
            .get_access_token()
            .map_err(|error| format!("authentication failed: {error:?}"))?;
        let mut socket = super::super::dial(url, &token)?;
        super::super::set_socket_read_timeout(&mut socket, READ_TIMEOUT);
        log::info!("[forward-tunnel] connected for dedicated port forwarding");

        while !stop.load(Ordering::Relaxed) {
            while let Ok(frame) = outbound_rx.try_recv() {
                socket
                    .send(tungstenite::Message::Binary(frame.into()))
                    .map_err(|error| format!("send failed: {error}"))?;
            }
            match socket.read() {
                Ok(tungstenite::Message::Binary(frame)) => {
                    handle_frame(frame.as_ref(), outbound_tx, streams.clone())?;
                }
                Ok(tungstenite::Message::Close(frame)) => {
                    // Only one data-plane socket may own a device id. When a
                    // newer worker connects, the gateway deliberately replaces
                    // this one. Reconnecting here would replace the new socket
                    // in turn and create an endless ping-pong that resets every
                    // in-flight forwarded asset request.
                    if frame
                        .as_ref()
                        .is_some_and(|frame| frame.reason.as_ref() == "forward tunnel replaced")
                    {
                        log::info!(
                            "[forward-tunnel] superseded by a newer connection; stopping stale worker"
                        );
                        stop.store(true, Ordering::Relaxed);
                    }
                    break;
                }
                Ok(tungstenite::Message::Ping(_))
                | Ok(tungstenite::Message::Pong(_))
                | Ok(tungstenite::Message::Text(_))
                | Ok(tungstenite::Message::Frame(_)) => {}
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(error) => return Err(format!("read failed: {error}")),
            }
        }
        let _ = socket.close(None);
        Ok(())
    }

    fn handle_frame(
        frame: &[u8],
        outbound: &async_channel::Sender<Vec<u8>>,
        streams: StreamMap,
    ) -> Result<(), String> {
        let (kind, stream_id, payload) = decode_frame(frame)?;
        match kind {
            OPEN => {
                let (metadata, body) = decode_open(payload)?;
                let (tx, rx) = mpsc::channel();
                streams.lock().unwrap().insert(stream_id, tx);
                let outbound = outbound.clone();
                let worker_streams = streams.clone();
                let _ = std::thread::Builder::new()
                    .name(format!("forward-stream-{stream_id}"))
                    .spawn(move || {
                        run_stream(stream_id, metadata, body, rx, &outbound);
                        worker_streams.lock().unwrap().remove(&stream_id);
                    });
            }
            DATA => {
                if let Some(stream) = streams.lock().unwrap().get(&stream_id).cloned() {
                    let _ = stream.send(StreamCommand::Data(payload.to_vec()));
                }
            }
            FIN | RESET => {
                if let Some(stream) = streams.lock().unwrap().remove(&stream_id) {
                    let _ = stream.send(StreamCommand::Fin);
                }
            }
            _ => return Err("gateway sent an invalid forward tunnel frame".into()),
        }
        Ok(())
    }

    fn run_stream(
        stream_id: u64,
        metadata: OpenMetadata,
        body: Vec<u8>,
        commands: mpsc::Receiver<StreamCommand>,
        outbound: &async_channel::Sender<Vec<u8>>,
    ) {
        let mut socket = match crate::realtime::forward::stream(
            metadata.port,
            &metadata.method,
            &metadata.path,
            &metadata.headers,
            &body,
        ) {
            Ok(socket) => socket,
            Err(error) => {
                let _ = outbound.send_blocking(encode_frame(RESET, stream_id, error.as_bytes()));
                return;
            }
        };
        if outbound
            .send_blocking(encode_frame(OPENED, stream_id, &[]))
            .is_err()
        {
            return;
        }
        loop {
            while let Ok(command) = commands.try_recv() {
                match command {
                    StreamCommand::Data(data) => {
                        if let Err(error) =
                            crate::realtime::forward::stream_write(&mut socket, &data)
                        {
                            let _ = outbound.send_blocking(encode_frame(
                                RESET,
                                stream_id,
                                error.as_bytes(),
                            ));
                            return;
                        }
                    }
                    StreamCommand::Fin => return,
                }
            }
            match crate::realtime::forward::stream_read(&mut socket) {
                Ok(Some(data)) => {
                    if outbound
                        .send_blocking(encode_frame(DATA, stream_id, &data))
                        .is_err()
                    {
                        return;
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    let _ = outbound.send_blocking(encode_frame(FIN, stream_id, &[]));
                    return;
                }
            }
        }
    }

    fn encode_frame(kind: u8, stream_id: u64, payload: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
        frame.push(VERSION);
        frame.push(kind);
        frame.extend_from_slice(&stream_id.to_be_bytes());
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    fn decode_frame(frame: &[u8]) -> Result<(u8, u64, &[u8]), String> {
        if frame.len() < HEADER_LEN || frame[0] != VERSION {
            return Err("invalid forward tunnel frame".into());
        }
        let stream_id = u64::from_be_bytes(frame[2..10].try_into().unwrap());
        let size = u32::from_be_bytes(frame[10..14].try_into().unwrap()) as usize;
        if size != frame.len() - HEADER_LEN {
            return Err("invalid forward tunnel payload length".into());
        }
        Ok((frame[1], stream_id, &frame[HEADER_LEN..]))
    }

    fn decode_open(payload: &[u8]) -> Result<(OpenMetadata, Vec<u8>), String> {
        if payload.len() < 4 {
            return Err("invalid forward open frame".into());
        }
        let metadata_len = u32::from_be_bytes(payload[..4].try_into().unwrap()) as usize;
        if metadata_len > payload.len() - 4 {
            return Err("invalid forward open metadata length".into());
        }
        let metadata = serde_json::from_slice(&payload[4..4 + metadata_len])
            .map_err(|error| format!("invalid forward open metadata: {error}"))?;
        Ok((metadata, payload[4 + metadata_len..].to_vec()))
    }

    fn close_streams(streams: &StreamMap) {
        for (_, stream) in streams.lock().unwrap().drain() {
            let _ = stream.send(StreamCommand::Fin);
        }
    }

    fn sleep_interruptible(duration: Duration, stop: &AtomicBool) {
        let deadline = std::time::Instant::now() + duration;
        while std::time::Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn binary_frame_round_trip_preserves_raw_bytes() {
            let frame = encode_frame(DATA, 42, &[0, 1, 127, 255]);
            let (kind, stream, payload) = decode_frame(&frame).unwrap();
            assert_eq!(kind, DATA);
            assert_eq!(stream, 42);
            assert_eq!(payload, &[0, 1, 127, 255]);
        }

        #[test]
        fn open_payload_keeps_body_outside_json() {
            let metadata = br#"{"port":5173,"method":"POST","path":"/x","headers":[]}"#;
            let mut payload = (metadata.len() as u32).to_be_bytes().to_vec();
            payload.extend_from_slice(metadata);
            payload.extend_from_slice(&[0, 255, 3]);
            let (open, body) = decode_open(&payload).unwrap();
            assert_eq!(open.port, 5173);
            assert_eq!(body, vec![0, 255, 3]);
        }
    }
}

#[cfg(not(target_family = "wasm"))]
pub use native::{start, ForwardTunnelHandle};
