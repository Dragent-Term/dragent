//! Short lived, owner-only credential channel. Environment variables contain
//! only its address; credentials are never written to a file or child env.
use super::Secret;
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const LIMIT: usize = 16 * 1024;

pub(super) struct Broker {
    pub address: String,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    directory: Option<PathBuf>,
}
impl std::fmt::Debug for Broker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AskpassBroker")
    }
}
impl Broker {
    pub fn new(secret: &Secret) -> Result<Self, String> {
        if secret.0.len() > LIMIT || secret.0.contains(['\n', '\r', '\0']) {
            return Err("SSH credential contains unsupported characters or exceeds 16 KiB".into());
        }
        let (server, address, directory) = platform::listen()
            .map_err(|e| format!("Cannot create private SSH credential channel: {e}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let secret = secret.clone();
        let worker = std::thread::spawn(move || {
            // Multiple prompts are possible (encrypted keys, authentication retry).
            while !stopping.load(Ordering::Acquire) {
                if let Ok(Some(mut stream)) = server.accept() {
                    let deadline = Instant::now() + Duration::from_secs(2);
                    let _ = (|| -> io::Result<()> {
                        let mut request = [0];
                        read_exact(&mut stream, &mut request, deadline, &stopping)?;
                        if request != [1] {
                            return Ok(());
                        }
                        write_all(
                            &mut stream,
                            &(secret.0.len() as u32).to_le_bytes(),
                            deadline,
                            &stopping,
                        )?;
                        write_all(&mut stream, secret.0.as_bytes(), deadline, &stopping)?;
                        // Keep a Windows pipe connected until the client consumes it.
                        read_exact(&mut stream, &mut request, deadline, &stopping)
                    })();
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Ok(Self {
            address,
            stop,
            worker: Some(worker),
            directory,
        })
    }
}
impl Drop for Broker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(directory) = &self.directory {
            let _ = std::fs::remove_dir_all(directory);
        }
    }
}

fn retry(error: &io::Error, deadline: Instant, stop: &AtomicBool) -> bool {
    let pending = error.kind() == io::ErrorKind::WouldBlock
        || error.kind() == io::ErrorKind::Interrupted
        || cfg!(windows) && matches!(error.raw_os_error(), Some(232 | 536));
    if pending && Instant::now() < deadline && !stop.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(10));
        true
    } else {
        false
    }
}
fn read_exact(
    stream: &mut impl Read,
    mut bytes: &mut [u8],
    deadline: Instant,
    stop: &AtomicBool,
) -> io::Result<()> {
    while !bytes.is_empty() {
        match stream.read(bytes) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => bytes = &mut bytes[n..],
            Err(e) if retry(&e, deadline, stop) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
fn write_all(
    stream: &mut impl Write,
    mut bytes: &[u8],
    deadline: Instant,
    stop: &AtomicBool,
) -> io::Result<()> {
    while !bytes.is_empty() {
        match stream.write(bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => bytes = &bytes[n..],
            Err(e) if retry(&e, deadline, stop) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

pub(super) fn run() -> bool {
    if std::env::var("DRAGENT_SSH_ASKPASS").as_deref() != Ok("1") {
        return false;
    }
    let result = (|| -> io::Result<()> {
        let address =
            std::env::var("DRAGENT_SSH_ASKPASS_ADDRESS").map_err(|_| io::ErrorKind::NotFound)?;
        let mut stream = platform::connect(&address)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let stop = AtomicBool::new(false);
        write_all(&mut stream, &[1], deadline, &stop)?;
        let mut length = [0; 4];
        read_exact(&mut stream, &mut length, deadline, &stop)?;
        let length = u32::from_le_bytes(length) as usize;
        if length > LIMIT {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let mut secret = Zeroizing::new(vec![0; length]);
        read_exact(&mut stream, &mut secret, deadline, &stop)?;
        write_all(&mut stream, &[1], deadline, &stop)?;
        let mut stdout = io::stdout().lock();
        stdout.write_all(&secret)?;
        stdout.write_all(b"\n")?;
        stdout.flush()
    })();
    // Failure must not fall through into normal app startup or return an empty password.
    if result.is_err() {
        std::process::exit(1);
    }
    true
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::{
        fs::{DirBuilderExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    };
    pub struct Server(UnixListener);
    pub fn listen() -> io::Result<(Server, String, Option<PathBuf>)> {
        let base = if cfg!(target_os = "macos") {
            PathBuf::from("/private/tmp")
        } else {
            std::env::temp_dir()
        };
        let dir = base.join(format!("drask-{}", uuid::Uuid::new_v4().simple()));
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let path = dir.join("socket");
        let result = (|| {
            let listener = UnixListener::bind(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            listener.set_nonblocking(true)?;
            Ok((
                Server(listener),
                path.to_string_lossy().into_owned(),
                Some(dir.clone()),
            ))
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(dir);
        }
        result
    }
    impl Server {
        pub fn accept(&self) -> io::Result<Option<UnixStream>> {
            match self.0.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(true)?;
                    Ok(Some(stream))
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
                Err(e) => Err(e),
            }
        }
    }
    pub fn connect(address: &str) -> io::Result<UnixStream> {
        let stream = UnixStream::connect(address)?;
        stream.set_nonblocking(true)?;
        Ok(stream)
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{
        fs::File,
        os::windows::io::{AsRawHandle, FromRawHandle},
    };
    use windows_sys::Win32::{
        Foundation::{
            ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING, GetLastError, INVALID_HANDLE_VALUE,
            LocalFree,
        },
        Security::{
            Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX},
        System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_NOWAIT,
            PIPE_REJECT_REMOTE_CLIENTS, SetNamedPipeHandleState,
        },
    };
    pub struct Server(File);
    pub struct Stream(File);
    impl Read for Stream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }
    impl Write for Stream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Drop for Stream {
        fn drop(&mut self) {
            unsafe {
                DisconnectNamedPipe(self.0.as_raw_handle());
            }
        }
    }
    pub fn listen() -> io::Result<(Server, String, Option<PathBuf>)> {
        let address = format!(
            r"\\.\pipe\dragent-askpass-{}",
            uuid::Uuid::new_v4().simple()
        );
        let name: Vec<u16> = address.encode_utf16().chain(Some(0)).collect();
        // An explicit protected DACL: only the creating account (owner rights).
        // Default named-pipe ACLs also grant Everyone read access.
        let sddl: Vec<u16> = "D:P(A;;GA;;;OW)".encode_utf16().chain(Some(0)).collect();
        let mut descriptor = std::ptr::null_mut();
        unsafe {
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let attrs = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            };
            let handle = CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                32768,
                64,
                0,
                &attrs,
            );
            let error = io::Error::last_os_error();
            LocalFree(descriptor);
            if handle == INVALID_HANDLE_VALUE {
                return Err(error);
            }
            Ok((Server(File::from_raw_handle(handle)), address, None))
        }
    }
    impl Server {
        pub fn accept(&self) -> io::Result<Option<Stream>> {
            unsafe {
                if ConnectNamedPipe(self.0.as_raw_handle(), std::ptr::null_mut()) != 0
                    || GetLastError() == ERROR_PIPE_CONNECTED
                {
                    Ok(Some(Stream(self.0.try_clone()?)))
                } else if GetLastError() == ERROR_PIPE_LISTENING {
                    Ok(None)
                } else if GetLastError() == 232 {
                    // ERROR_NO_DATA: disconnected before accept
                    DisconnectNamedPipe(self.0.as_raw_handle());
                    Ok(None)
                } else {
                    Err(io::Error::last_os_error())
                }
            }
        }
    }
    pub fn connect(address: &str) -> io::Result<File> {
        if !address.starts_with(r"\\.\pipe\dragent-askpass-") {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(address)?;
        let mode = PIPE_NOWAIT;
        if unsafe {
            SetNamedPipeHandleState(
                file.as_raw_handle(),
                &mode,
                std::ptr::null(),
                std::ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(file)
    }
}
