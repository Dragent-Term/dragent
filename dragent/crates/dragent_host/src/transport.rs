//! Local IPC transport between the app, the host and the PTY daemon.
//!
//! macOS and Linux use Unix domain sockets: no port to allocate or collide
//! on, no loopback exposure to other local users (the socket file is
//! owner-only), and less per-connection overhead. Windows uses loopback TCP
//! (the standard library has no Unix-socket support there); every request
//! still carries the endpoint token.
//!
//! [`Stream`] and [`Listener`] hide the difference so the protocol code is
//! the same on every platform.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

/// Where a service listens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Address {
    Tcp(u16),
    #[cfg(unix)]
    Unix(PathBuf),
}

impl Address {
    /// The socket path when one is recorded and usable on this platform,
    /// else the loopback port.
    pub fn from_parts(port: u16, socket: Option<&str>) -> Self {
        #[cfg(unix)]
        if let Some(path) = socket.filter(|p| !p.is_empty()) {
            return Address::Unix(PathBuf::from(path));
        }
        let _ = socket;
        Address::Tcp(port)
    }
}

/// One connection.
#[derive(Debug)]
pub enum Stream {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(UnixStream),
}

impl Stream {
    pub fn connect(address: &Address, timeout: Duration) -> io::Result<Stream> {
        match address {
            Address::Tcp(port) => {
                let addr = std::net::SocketAddr::from(([127, 0, 0, 1], *port));
                let stream = TcpStream::connect_timeout(&addr, timeout)?;
                stream.set_nodelay(true)?;
                Ok(Stream::Tcp(stream))
            }
            // Local connects succeed or fail immediately (no handshake).
            #[cfg(unix)]
            Address::Unix(path) => UnixStream::connect(path).map(Stream::Unix),
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.set_read_timeout(timeout),
            #[cfg(unix)]
            Stream::Unix(s) => s.set_read_timeout(timeout),
        }
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.set_write_timeout(timeout),
            #[cfg(unix)]
            Stream::Unix(s) => s.set_write_timeout(timeout),
        }
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.set_nonblocking(nonblocking),
            #[cfg(unix)]
            Stream::Unix(s) => s.set_nonblocking(nonblocking),
        }
    }

    pub fn shutdown(&self) {
        let _ = match self {
            Stream::Tcp(s) => s.shutdown(Shutdown::Both),
            #[cfg(unix)]
            Stream::Unix(s) => s.shutdown(Shutdown::Both),
        };
    }

    /// The peer is on this machine. Always true for a Unix socket; for TCP
    /// the listener binds loopback, so this is a belt-and-braces check.
    pub fn is_local_peer(&self) -> bool {
        match self {
            Stream::Tcp(s) => s.peer_addr().map(|a| a.ip().is_loopback()).unwrap_or(false),
            #[cfg(unix)]
            Stream::Unix(_) => true,
        }
    }
}

impl Read for &Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match *self {
            Stream::Tcp(s) => (&*s).read(buf),
            #[cfg(unix)]
            Stream::Unix(s) => (&*s).read(buf),
        }
    }
}

impl Write for &Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match *self {
            Stream::Tcp(s) => (&*s).write(buf),
            #[cfg(unix)]
            Stream::Unix(s) => (&*s).write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match *self {
            Stream::Tcp(s) => (&*s).flush(),
            #[cfg(unix)]
            Stream::Unix(s) => (&*s).flush(),
        }
    }
}

/// A listening endpoint.
#[derive(Debug)]
pub enum Listener {
    Tcp(TcpListener),
    #[cfg(unix)]
    Unix(UnixListener, PathBuf),
}

impl Listener {
    /// Listen on the Unix socket at `socket_path` (macOS/Linux), replacing a
    /// stale socket file, owner-only. Falls back to loopback TCP (port from
    /// `tcp_ports`, first that binds) on Windows or when the socket cannot be
    /// created (e.g. a path over the platform's length limit).
    pub fn bind_local(socket_path: Option<&Path>, tcp_ports: &[u16]) -> io::Result<Listener> {
        #[cfg(unix)]
        if let Some(path) = socket_path {
            match bind_unix(path) {
                Ok(listener) => return Ok(Listener::Unix(listener, path.to_path_buf())),
                Err(error) => {
                    log::warn!(
                        "unix socket {} unavailable ({error}); using TCP",
                        path.display()
                    )
                }
            }
        }
        let _ = socket_path;
        let mut last = io::Error::other("no port to bind");
        for port in tcp_ports.iter().copied().chain(std::iter::once(0)) {
            match TcpListener::bind(("127.0.0.1", port)) {
                Ok(listener) => return Ok(Listener::Tcp(listener)),
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        match self {
            Listener::Tcp(l) => l.set_nonblocking(nonblocking),
            #[cfg(unix)]
            Listener::Unix(l, _) => l.set_nonblocking(nonblocking),
        }
    }

    /// Wait for a connection without delaying one that arrives while idle.
    /// The timeout lets service loops keep checking shutdown and ownership.
    #[cfg(unix)]
    pub(crate) fn wait_readable(&self, timeout: Duration) -> io::Result<bool> {
        let mut descriptor = libc::pollfd {
            fd: self.raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let milliseconds = timeout.as_millis().min(i32::MAX as u128) as i32;
        // The listener owns this descriptor throughout the synchronous wait.
        let ready = unsafe { libc::poll(&mut descriptor, 1, milliseconds) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::Interrupted {
                Ok(false)
            } else {
                Err(error)
            };
        }
        if descriptor.revents & libc::POLLNVAL != 0 {
            return Err(io::Error::other("terminal listener descriptor is invalid"));
        }
        Ok(ready > 0)
    }

    /// Accept one connection, as a blocking stream (accepted sockets inherit
    /// the listener's non-blocking mode on macOS/BSD).
    pub fn accept(&self) -> io::Result<Stream> {
        let stream = match self {
            Listener::Tcp(l) => l.accept().map(|(s, _)| Stream::Tcp(s))?,
            #[cfg(unix)]
            Listener::Unix(l, _) => l.accept().map(|(s, _)| Stream::Unix(s))?,
        };
        stream.set_nonblocking(false)?;
        if let Stream::Tcp(tcp) = &stream {
            tcp.set_nodelay(true)?;
        }
        Ok(stream)
    }

    /// TCP port (0 for a Unix socket).
    pub fn port(&self) -> u16 {
        match self {
            Listener::Tcp(l) => l.local_addr().map(|a| a.port()).unwrap_or(0),
            #[cfg(unix)]
            Listener::Unix(..) => 0,
        }
    }

    /// Socket path for a Unix listener.
    pub fn socket_path(&self) -> Option<String> {
        match self {
            Listener::Tcp(_) => None,
            #[cfg(unix)]
            Listener::Unix(_, path) => Some(path.to_string_lossy().into_owned()),
        }
    }

    /// Raw listener fd (Unix), for handing the listener to a successor.
    #[cfg(unix)]
    pub fn raw_fd(&self) -> i32 {
        use std::os::fd::AsRawFd;
        match self {
            Listener::Tcp(l) => l.as_raw_fd(),
            Listener::Unix(l, _) => l.as_raw_fd(),
        }
    }

    /// Rebuild a listener inherited from a predecessor (`socket_path` set
    /// for a Unix socket, `None` for TCP).
    ///
    /// # Safety
    /// `fd` must be an open listening socket of the given kind that nothing
    /// else owns.
    #[cfg(unix)]
    pub unsafe fn from_raw_fd(fd: i32, socket_path: Option<&str>) -> Listener {
        use std::os::fd::FromRawFd;
        match socket_path {
            Some(path) => Listener::Unix(
                unsafe { UnixListener::from_raw_fd(fd) },
                PathBuf::from(path),
            ),
            None => Listener::Tcp(unsafe { TcpListener::from_raw_fd(fd) }),
        }
    }

    /// Remove the socket file (Unix). Only the owner of the service should
    /// call this, on a clean shutdown.
    pub fn remove_socket_file(&self) {
        #[cfg(unix)]
        if let Listener::Unix(_, path) = self {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(unix)]
fn bind_unix(path: &Path) -> io::Result<UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // A socket file left by a dead process refuses connections; replace it.
    // A live one answers, and must not be stolen.
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AddrInUse, "socket is in use"));
        }
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

impl crate::protocol::ServiceEndpoint {
    pub fn address(&self) -> Address {
        Address::from_parts(self.port, self.socket.as_deref())
    }
}

impl crate::protocol::PtyDaemonManifest {
    pub fn address(&self) -> Address {
        Address::from_parts(self.port, self.socket.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(listener: Listener, address: Address) {
        let server = std::thread::spawn(move || {
            loop {
                // Skip probe connections that close without sending anything.
                let stream = listener.accept().expect("accept");
                let mut buf = [0u8; 5];
                if (&stream).read_exact(&mut buf).is_ok() {
                    (&stream).write_all(&buf).expect("echo");
                    return;
                }
            }
        });
        let client = Stream::connect(&address, Duration::from_secs(1)).expect("connect");
        (&client).write_all(b"hello").unwrap();
        let mut back = [0u8; 5];
        (&client).read_exact(&mut back).unwrap();
        assert_eq!(&back, b"hello");
        server.join().unwrap();
    }

    #[test]
    fn tcp_round_trip() {
        let listener = Listener::bind_local(None, &[]).expect("bind");
        let address = Address::Tcp(listener.port());
        assert!(listener.socket_path().is_none());
        round_trip(listener, address);
    }

    #[cfg(unix)]
    fn readiness_round_trip(listener: Listener, address: Address) {
        use std::time::Instant;
        listener.set_nonblocking(true).unwrap();
        let idle = Instant::now();
        assert!(!listener.wait_readable(Duration::from_millis(100)).unwrap());
        assert!(idle.elapsed() >= Duration::from_millis(90));
        assert!(idle.elapsed() < Duration::from_secs(1));
        let peer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            let stream = Stream::connect(&address, Duration::from_secs(1)).unwrap();
            (&stream).write_all(b"r").unwrap();
        });
        let waiting = Instant::now();
        assert!(listener.wait_readable(Duration::from_secs(2)).unwrap());
        assert!(waiting.elapsed() < Duration::from_secs(1));
        let stream = listener.accept().unwrap();
        let mut byte = [0];
        (&stream).read_exact(&mut byte).unwrap();
        assert_eq!(byte, [b'r']);
        peer.join().unwrap();
        listener.remove_socket_file();
    }

    #[cfg(unix)]
    #[test]
    fn tcp_listener_wait_wakes_for_connection_and_bounds_idle_shutdown_check() {
        let listener = Listener::bind_local(None, &[]).unwrap();
        let address = Address::Tcp(listener.port());
        readiness_round_trip(listener, address);
    }

    #[cfg(unix)]
    #[test]
    fn unix_listener_wait_wakes_for_connection_and_bounds_idle_shutdown_check() {
        let path =
            std::env::temp_dir().join(format!("dr-r-{}.sock", uuid::Uuid::new_v4().simple()));
        let listener = Listener::bind_local(Some(&path), &[]).unwrap();
        assert!(matches!(listener, Listener::Unix(..)));
        readiness_round_trip(listener, Address::Unix(path));
    }

    #[cfg(unix)]
    #[test]
    fn unix_socket_round_trip_owner_only_and_stale_file_replaced() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("dragent-transport-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("svc.sock");
        // A stale socket file from a dead service.
        drop(UnixListener::bind(&path).unwrap());
        assert!(path.exists());

        // The kernel can briefly keep reporting a just-closed listener as
        // live when many threads churn sockets at once; the liveness probe
        // then (correctly, conservatively) refuses to steal it and this
        // falls back to TCP. Retry until teardown settles instead of
        // assuming close-then-probe is synchronous. A genuinely live
        // socket never settles, so the deadline keeps that guard loud.
        let start = std::time::Instant::now();
        let listener = loop {
            let listener = Listener::bind_local(Some(&path), &[]).expect("bind over stale file");
            if matches!(listener, Listener::Unix(..)) {
                break listener;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(10),
                "stale socket still looks live after 10s; refusing to steal it"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // A live socket is never stolen.
        assert!(bind_unix(&path).is_err());
        let address = Address::from_parts(0, listener.socket_path().as_deref());
        assert_eq!(address, Address::Unix(path.clone()));
        round_trip(listener, address);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
