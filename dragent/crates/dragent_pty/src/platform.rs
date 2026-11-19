//! Session platform detection (native shell environment).

/// Where the interactive shell runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPlatform {
    Native,
}

impl SessionPlatform {
    pub fn detect() -> Self {
        Self::Native
    }

    /// Bytes to send after a command line when submitting to the PTY.
    pub fn command_submit_bytes(self) -> &'static [u8] {
        match self {
            // PTY line discipline expects carriage return as Enter on all desktops.
            SessionPlatform::Native => b"\r",
        }
    }

    pub fn default_line_ending(self) -> LineEnding {
        match self {
            SessionPlatform::Native => LineEnding::platform_native(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn platform_native() -> Self {
        if cfg!(windows) {
            Self::CrLf
        } else {
            Self::Lf
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }
}
