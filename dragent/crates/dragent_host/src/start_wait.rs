//! An agent's launch monitor waiting its turn in [`crate::start_gate`]'s
//! line, in the agent's own terminal.
//!
//! The wait needs neither the app nor the host to stay up: the monitor asks
//! again whenever the host answers, and decides by itself while there is no
//! host. Pressing S starts the agent at once; Ctrl-C calls the start off.

use std::io::Write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::client::HostClient;
use crate::start_gate::{waiting_text, Answer, Ask, ASK_EVERY};

/// How long one answer from the host may take.
const ANSWER_WITHIN: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Start,
    Cancelled,
}

/// A key that ends a wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    StartAnyway,
    Cancel,
}

/// What `typed` means. Only S by itself starts: a message pasted or typed
/// in by something else must not.
fn key(typed: &[u8]) -> Option<Key> {
    match typed {
        b"s" | b"S" => Some(Key::StartAnyway),
        [0x03] => Some(Key::Cancel),
        _ => None,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Ask the host; without one, decide from this machine's state alone.
fn ask(ask: &Ask) -> Answer {
    let asked = HostClient::connect_existing()
        .and_then(|client| client.agent_start(ask.clone(), ANSWER_WITHIN));
    match asked {
        Ok(answer) => answer,
        Err(_) => match crate::resources::held_now(&dragent_paths::organisation_dir()) {
            Some(reason) => Answer::Wait {
                position: 1,
                reason: Some(reason),
            },
            None => Answer::Go,
        },
    }
}

/// Wait for the agent `label` in `pane` to be let go. `on_wait` runs once,
/// before the first wait.
pub fn wait_for_turn(id: &str, terminal_id: &str, label: &str, on_wait: impl FnOnce()) -> Outcome {
    let request = Ask {
        id: id.to_string(),
        terminal_id: terminal_id.to_string(),
        label: label.to_string(),
        since_ms: now_ms(),
    };
    let mut answer = ask(&request);
    if answer == Answer::Go {
        return Outcome::Start;
    }
    on_wait();
    let mut keys = keys::Keys::open();
    let mut said = String::new();
    let outcome = loop {
        let mut text = waiting_text(label, &answer);
        if keys.is_some() {
            text.push_str(" Press S to start it now, Ctrl-C to cancel.");
        }
        if text != said {
            eprint!("\r\n{text}\r\n");
            let _ = std::io::stderr().flush();
            said = text;
        }
        let pressed = match keys.as_mut() {
            Some(keys) => keys.wait(ASK_EVERY).and_then(|typed| key(&typed)),
            None => {
                std::thread::sleep(ASK_EVERY);
                None
            }
        };
        match pressed {
            Some(Key::StartAnyway) => break Outcome::Start,
            Some(Key::Cancel) => break Outcome::Cancelled,
            None => {}
        }
        answer = ask(&request);
        if answer == Answer::Go {
            return Outcome::Start;
        }
    };
    if let Ok(client) = HostClient::connect_existing() {
        let _ = client.leave_start_line(id);
    }
    outcome
}

/// `dragent --dragent-agent-wait <label>`: hold a terminal's next command
/// until its turn. Exit code 0 lets it run.
pub fn run_wait(label: &str) -> i32 {
    let terminal =
        std::env::var("DRAGENT_HOST_TERMINAL_ID").unwrap_or_else(|_| Uuid::new_v4().to_string());
    match wait_for_turn(&terminal, &terminal, label, || {}) {
        Outcome::Start => 0,
        Outcome::Cancelled => 130,
    }
}

#[cfg(unix)]
mod keys {
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::time::Duration;

    /// The terminal's keys, unechoed, for as long as this lives. Opened
    /// directly: standard input may be a piped prompt meant for the agent.
    pub struct Keys {
        tty: File,
        saved: libc::termios,
    }

    impl Keys {
        pub fn open() -> Option<Self> {
            let tty = File::options()
                .read(true)
                .write(true)
                .open("/dev/tty")
                .ok()?;
            // SAFETY: the descriptor is open for both calls and `saved` is
            // filled before it's read. In the background, changing the
            // terminal would stop this process instead.
            unsafe {
                if libc::tcgetpgrp(tty.as_raw_fd()) != libc::getpgrp() {
                    return None;
                }
                let mut saved: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(tty.as_raw_fd(), &mut saved) != 0 {
                    return None;
                }
                let mut raw = saved;
                raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
                raw.c_cc[libc::VMIN] = 1;
                raw.c_cc[libc::VTIME] = 0;
                (libc::tcsetattr(tty.as_raw_fd(), libc::TCSANOW, &raw) == 0)
                    .then_some(Self { tty, saved })
            }
        }

        /// What was typed at once, if anything was within `timeout`.
        pub fn wait(&mut self, timeout: Duration) -> Option<Vec<u8>> {
            let fd = self.tty.as_raw_fd();
            let mut typed = [0u8; 1024];
            // SAFETY: the set holds this one descriptor, just opened in a
            // small process and so within its size. `poll` would do, but on
            // macOS it doesn't work on /dev/tty.
            let read = unsafe {
                let mut set: libc::fd_set = std::mem::zeroed();
                libc::FD_ZERO(&mut set);
                libc::FD_SET(fd, &mut set);
                let mut within = libc::timeval {
                    tv_sec: timeout.as_secs() as _,
                    tv_usec: timeout.subsec_micros() as _,
                };
                let none = std::ptr::null_mut;
                if libc::select(fd + 1, &mut set, none(), none(), &mut within) <= 0 {
                    return None;
                }
                libc::read(fd, typed.as_mut_ptr().cast(), typed.len())
            };
            if read <= 0 {
                // The terminal is gone: don't spin until the hang-up arrives.
                std::thread::sleep(timeout);
                return None;
            }
            Some(typed[..read as usize].to_vec())
        }
    }

    impl Drop for Keys {
        fn drop(&mut self) {
            // SAFETY: restores what was read from the same terminal. Typed
            // text meant for nobody is dropped with it.
            unsafe { libc::tcsetattr(self.tty.as_raw_fd(), libc::TCSAFLUSH, &self.saved) };
        }
    }
}

#[cfg(windows)]
mod keys {
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{
        GetNumberOfConsoleInputEvents, GetStdHandle, ReadConsoleInputW, INPUT_RECORD, KEY_EVENT,
        STD_INPUT_HANDLE,
    };

    /// The console's keys. Ctrl-C keeps its usual meaning, which ends this
    /// process.
    pub struct Keys {
        console: HANDLE,
    }

    impl Keys {
        pub fn open() -> Option<Self> {
            let mut waiting = 0;
            // SAFETY: the handle is only used if the console answers for it.
            unsafe {
                let console = GetStdHandle(STD_INPUT_HANDLE);
                (!console.is_null()
                    && console != INVALID_HANDLE_VALUE
                    && GetNumberOfConsoleInputEvents(console, &mut waiting) != 0)
                    .then_some(Self { console })
            }
        }

        pub fn wait(&mut self, timeout: Duration) -> Option<Vec<u8>> {
            let until = Instant::now() + timeout;
            loop {
                let typed = self.pending();
                if !typed.is_empty() {
                    return Some(typed);
                }
                if Instant::now() >= until {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        fn pending(&self) -> Vec<u8> {
            let mut typed = Vec::new();
            let mut waiting = 0;
            // SAFETY: at most as many records are read as the array holds,
            // and only key records are read as keys.
            unsafe {
                if GetNumberOfConsoleInputEvents(self.console, &mut waiting) == 0 || waiting == 0 {
                    return typed;
                }
                let mut records: [INPUT_RECORD; 64] = std::mem::zeroed();
                let mut read = 0;
                if ReadConsoleInputW(self.console, records.as_mut_ptr(), 64, &mut read) == 0 {
                    return typed;
                }
                for record in &records[..read as usize] {
                    if record.EventType == KEY_EVENT as u16 {
                        let event = record.Event.KeyEvent;
                        let unit = event.uChar.UnicodeChar;
                        if event.bKeyDown != 0 && unit != 0 {
                            typed.extend(
                                char::from_u32(u32::from(unit))
                                    .unwrap_or('\u{fffd}')
                                    .to_string()
                                    .into_bytes(),
                            );
                        }
                    }
                }
            }
            typed
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod keys {
    pub struct Keys;

    impl Keys {
        pub fn open() -> Option<Self> {
            None
        }

        pub fn wait(&mut self, _timeout: std::time::Duration) -> Option<Vec<u8>> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_lone_s_starts_a_waiting_agent() {
        assert_eq!(key(b"s"), Some(Key::StartAnyway));
        assert_eq!(key(b"S"), Some(Key::StartAnyway));
        assert_eq!(key(&[0x03]), Some(Key::Cancel));
        // A message typed in meanwhile, and its Enter.
        for text in [
            &b"start the tests"[..],
            b"ss",
            b"\r",
            b"\n",
            b"\x1b[200~s\x1b[201~",
            b"",
        ] {
            assert_eq!(key(text), None);
        }
    }
}
