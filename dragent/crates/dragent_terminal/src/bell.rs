//! Cross-platform audible terminal bell.
//!
//! When the terminal receives the BEL control character (0x07) and audible
//! alerts are enabled, call [`ring_bell`] to produce the platform alert.
//!
//! # Platform implementations
//! - **macOS**: calls `NSBeep()` from AppKit via a bare FFI link — no
//!   extra crates needed, the framework is always present.
//! - **Linux**: writes a BEL byte to `/dev/tty` (which the kernel routes to
//!   the PC speaker or the sound system as configured by the user).
//! - **Windows**: calls `MessageBeep(0)` from `user32.dll`.
//! - **Other**: writes BEL to stderr, which usually triggers the parent
//!   terminal's own bell if running inside one.

/// Ring the audible terminal bell using the best available mechanism for
/// the current platform.
pub fn ring_bell() {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: NSBeep() is a simple C function with no preconditions.
        #[link(name = "AppKit", kind = "framework")]
        extern "C" {
            fn NSBeep();
        }
        unsafe { NSBeep() };
        return;
    }

    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        // Try writing to /dev/tty first; fall through to stderr on failure.
        if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
            let _ = tty.write_all(b"\x07");
            return;
        }
        // Fallthrough: write BEL to stderr.
    }

    #[cfg(target_os = "windows")]
    {
        #[link(name = "user32")]
        extern "system" {
            fn MessageBeep(utype: u32) -> i32;
        }
        // MB_ICONASTERISK (0x40) → default system sound.
        unsafe {
            MessageBeep(0x00000040);
        }
        return;
    }

    // Generic fallback: BEL to stderr.
    #[allow(unreachable_code)]
    {
        use std::io::Write;
        let _ = std::io::stderr().write_all(b"\x07");
    }
}
