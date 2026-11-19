//! Mouse-reporting state and PTY event encoding.
//!
//! When the running program enables mouse reporting (via `?1000h` etc.) the
//! terminal emulator must translate OS mouse events into the protocol bytes the
//! program expects and write them to the PTY master. This module tracks the
//! active reporting mode and handles the encoding.
//!
//! # Supported modes
//! | DEC mode | Name             | Description                                     |
//! |----------|------------------|-------------------------------------------------|
//! | 1000     | Normal           | Button press/release only                       |
//! | 1002     | ButtonMotion     | + motion while a button is held                 |
//! | 1003     | AnyMotion        | + all motion (hover)                            |
//! | 1004     | FocusTracking    | Focus in/out (separate from button modes)       |
//! | 1006     | SGR              | Extended coordinate encoding (SGR)              |
//! | 1015     | URXVT            | URXVT decimal encoding                          |
//! | 1016     | SGRPixels        | SGR with pixel coordinates                      |

/// Active mouse-reporting mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum MouseReportingMode {
    /// Mouse reporting is disabled (default).
    #[default]
    None,
    /// Report button press and release events only (X10 / Normal mode).
    Normal,
    /// Report button events and motion while at least one button is pressed.
    ButtonMotion,
    /// Report all button events and all pointer motion.
    AnyMotion,
}

/// Extended coordinate encoding used when reporting mouse events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum MouseEncoding {
    /// Legacy X10 encoding — coordinates clamped to 223.
    #[default]
    X10,
    /// SGR `<button;col;row M/m` — unbounded coordinates.
    Sgr,
    /// URXVT decimal `[button;col;rowM` — unbounded, no release distinction.
    Urxvt,
}

/// A single mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left = 0,
    Middle = 1,
    Right = 2,
    /// Scroll wheel up.
    WheelUp = 64,
    /// Scroll wheel down.
    WheelDown = 65,
    /// No button (motion event).
    None = 3,
}

impl MouseButton {
    /// Raw button code for X10/SGR/URXVT wire format.
    pub fn code(self) -> u8 {
        self as u8
    }
}

/// A mouse event to be encoded and written to the PTY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub button: MouseButton,
    /// 0-based column.
    pub col: usize,
    /// 0-based row.
    pub row: usize,
    pub modifiers: MouseModifiers,
    pub kind: MouseEventKind,
}

/// Which OS-level action triggered the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    Press,
    Release,
    Motion,
}

/// Keyboard modifier flags active during the mouse event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MouseModifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl MouseModifiers {
    /// Modifier bits added to the button code (bits 2–4).
    pub fn bits(self) -> u8 {
        let mut b = 0u8;
        if self.shift {
            b |= 0x04;
        }
        if self.alt {
            b |= 0x08;
        }
        if self.ctrl {
            b |= 0x10;
        }
        b
    }
}

/// Encode a mouse event as the bytes that should be written to the PTY master.
///
/// Returns `None` if the current mode does not report this kind of event.
pub fn encode_mouse_event(
    event: MouseEvent,
    mode: MouseReportingMode,
    encoding: MouseEncoding,
) -> Option<Vec<u8>> {
    // Check whether this event type is reported by the current mode.
    match mode {
        MouseReportingMode::None => return None,
        MouseReportingMode::Normal => {
            if event.kind == MouseEventKind::Motion {
                return None;
            }
        }
        MouseReportingMode::ButtonMotion => {
            if event.kind == MouseEventKind::Motion && event.button == MouseButton::None {
                return None;
            }
        }
        MouseReportingMode::AnyMotion => {} // everything reported
    }

    let btn_code = event.button.code().wrapping_add(event.modifiers.bits())
        + if event.kind == MouseEventKind::Motion {
            32
        } else {
            0
        };

    // 1-based coordinates for the wire format.
    let col = event.col + 1;
    let row = event.row + 1;

    match encoding {
        MouseEncoding::X10 => {
            // Clamp to 223 (max representable in a single byte offset from 32).
            let col_b = (col.min(223) as u8).wrapping_add(32);
            let row_b = (row.min(223) as u8).wrapping_add(32);
            let btn_b = btn_code.wrapping_add(32);
            Some(vec![0x1b, b'[', b'M', btn_b, col_b, row_b])
        }
        MouseEncoding::Sgr => {
            let trailer = match event.kind {
                MouseEventKind::Release => b'm',
                _ => b'M',
            };
            Some(format!("\x1b[<{};{};{}{}", btn_code, col, row, trailer as char).into_bytes())
        }
        MouseEncoding::Urxvt => {
            Some(format!("\x1b[{};{};{}M", btn_code + 32, col, row).into_bytes())
        }
    }
}

/// Combined mouse state held by the emulator.
#[derive(Debug, Clone, Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct MouseState {
    pub reporting: MouseReportingMode,
    pub encoding: MouseEncoding,
    pub focus_tracking: bool,
}

impl MouseState {
    pub fn set_mode(&mut self, mode: u16, enable: bool) {
        match mode {
            1000 => {
                self.reporting = if enable {
                    MouseReportingMode::Normal
                } else {
                    MouseReportingMode::None
                };
            }
            1002 => {
                self.reporting = if enable {
                    MouseReportingMode::ButtonMotion
                } else {
                    MouseReportingMode::None
                };
            }
            1003 => {
                self.reporting = if enable {
                    MouseReportingMode::AnyMotion
                } else {
                    MouseReportingMode::None
                };
            }
            1004 => self.focus_tracking = enable,
            1006 => {
                if enable {
                    self.encoding = MouseEncoding::Sgr;
                } else {
                    self.encoding = MouseEncoding::X10;
                }
            }
            1015 => {
                if enable {
                    self.encoding = MouseEncoding::Urxvt;
                } else {
                    self.encoding = MouseEncoding::X10;
                }
            }
            _ => {}
        }
    }

    /// Encode and return bytes to write to the PTY for the given event.
    pub fn encode(&self, event: MouseEvent) -> Option<Vec<u8>> {
        encode_mouse_event(event, self.reporting, self.encoding)
    }
}
