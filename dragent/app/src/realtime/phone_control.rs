//! Who controls a terminal's size when a phone and the desktop both show it.
//!
//! A terminal has one PTY size. When the phone attaches and resizes it, the
//! desktop pane would draw the phone-sized grid (squashed and distorted). So:
//!
//! - **Phone in control**: the phone resized the terminal. The desktop pane
//!   covers it with a frosted card ("Your phone is using this terminal"),
//!   pauses desktop typing and stops sending its own size.
//! - **Held**: the phone left (detached, lease expired, lost) but the PTY is
//!   still at the phone's size. The card offers "Restore".
//! - **Desktop in control**: the person took control back on the desktop. The
//!   PTY is resized to the pane, the companion host releases the phone's
//!   handles and tells the phone, which leaves the tab. The state then
//!   clears; opening the tab on the phone again takes the terminal over.
//!
//! Keyed by the host terminal session id, process-wide. Holds no content.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// The control state of one terminal session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    /// The phone sized the terminal and is using it.
    Phone { phone_name: String, cols: u16, rows: u16 },
    /// The phone left the terminal at its size.
    Held { cols: u16, rows: u16 },
    /// The desktop took control back; phone resizes are refused.
    Desktop,
}

fn map() -> &'static Mutex<HashMap<String, Control>> {
    static MAP: OnceLock<Mutex<HashMap<String, Control>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

static GENERATION: AtomicU64 = AtomicU64::new(0);

fn listeners() -> &'static Mutex<Vec<async_channel::Sender<()>>> {
    static LISTENERS: OnceLock<Mutex<Vec<async_channel::Sender<()>>>> = OnceLock::new();
    LISTENERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// A wakeup on every change, for a terminal pane to redraw its cover. One
/// queued wakeup stands for any number of changes.
pub fn subscribe() -> async_channel::Receiver<()> {
    let (tx, rx) = async_channel::bounded(1);
    listeners().lock().unwrap_or_else(|e| e.into_inner()).push(tx);
    rx
}

fn wake() {
    listeners()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|tx| !matches!(tx.try_send(()), Err(async_channel::TrySendError::Closed(()))));
}

/// Bumps on every change, so views can tell when to re-render.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

fn set(session: &str, next: Option<Control>) {
    let mut m = map().lock().unwrap_or_else(|e| e.into_inner());
    let changed = match &next {
        Some(c) => m.insert(session.to_string(), c.clone()).as_ref() != Some(c),
        None => m.remove(session).is_some(),
    };
    drop(m);
    if changed {
        GENERATION.fetch_add(1, Ordering::AcqRel);
        wake();
    }
}

/// The current state of `session`, if anyone but the desktop pane sized it.
pub fn get(session: &str) -> Option<Control> {
    map().lock().unwrap_or_else(|e| e.into_inner()).get(session).cloned()
}

/// Whether the desktop pane must not send its own size or typing.
pub fn phone_sized(session: &str) -> bool {
    matches!(get(session), Some(Control::Phone { .. } | Control::Held { .. }))
}

/// A phone asks to resize `session`. Refused (false) while the desktop holds
/// control; otherwise the phone is in control at that size.
pub fn phone_resize(session: &str, phone_name: &str, cols: u16, rows: u16) -> bool {
    if get(session) == Some(Control::Desktop) {
        return false;
    }
    set(
        session,
        Some(Control::Phone {
            phone_name: phone_name.to_string(),
            cols,
            rows,
        }),
    );
    true
}

/// The phone opens the tab: lifts a desktop hold left by a take-back.
pub fn phone_claim(session: &str) {
    if get(session) == Some(Control::Desktop) {
        set(session, None);
    }
}

/// The phone stopped using `session` (detach, expiry, lost channel). A
/// terminal it had sized stays "held" at that size until restored.
pub fn phone_left(session: &str) {
    if let Some(Control::Phone { cols, rows, .. }) = get(session) {
        set(session, Some(Control::Held { cols, rows }));
    }
}

/// The desktop takes control back. The caller resizes the PTY to the pane.
/// While the phone is still attached its resizes are refused; once it has
/// left there is nothing to guard and the state clears.
pub fn desktop_take_back(session: &str) {
    match get(session) {
        Some(Control::Phone { .. }) => set(session, Some(Control::Desktop)),
        Some(Control::Held { .. }) => set(session, None),
        _ => {}
    }
}

/// Forget `session` (the terminal closed).
pub fn forget(session: &str) {
    set(session, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(tag: &str) -> String {
        format!("phone-control-test-{tag}-{}", std::process::id())
    }

    #[test]
    fn the_phone_takes_control_by_resizing() {
        let id = s("resize");
        assert!(!phone_sized(&id));
        assert!(phone_resize(&id, "iPhone", 40, 20));
        assert_eq!(
            get(&id),
            Some(Control::Phone { phone_name: "iPhone".into(), cols: 40, rows: 20 })
        );
        assert!(phone_sized(&id));
        forget(&id);
    }

    #[test]
    fn take_back_refuses_phone_resizes_until_the_phone_claims() {
        let id = s("takeback");
        assert!(phone_resize(&id, "iPhone", 40, 20));
        desktop_take_back(&id);
        assert_eq!(get(&id), Some(Control::Desktop));
        assert!(!phone_sized(&id), "the desktop pane sizes the terminal again");
        assert!(!phone_resize(&id, "iPhone", 40, 20), "phone layout changes don't steal it back");
        phone_claim(&id);
        assert!(phone_resize(&id, "iPhone", 40, 20), "an explicit claim does");
        forget(&id);
    }

    #[test]
    fn a_phone_that_leaves_holds_the_size_until_restored() {
        let id = s("held");
        assert!(phone_resize(&id, "iPhone", 40, 20));
        phone_left(&id);
        assert_eq!(get(&id), Some(Control::Held { cols: 40, rows: 20 }));
        assert!(phone_sized(&id));
        desktop_take_back(&id);
        assert_eq!(get(&id), None, "restored: nothing left to guard");
        forget(&id);
    }

    #[test]
    fn a_change_wakes_subscribers_and_dropped_ones_are_forgotten() {
        let id = s("wake");
        let rx = subscribe();
        drop(subscribe());
        assert!(phone_resize(&id, "iPhone", 40, 20));
        assert!(rx.try_recv().is_ok());
        forget(&id);
    }

    #[test]
    fn changes_bump_the_generation_and_repeats_do_not() {
        let id = s("gen");
        let g0 = generation();
        assert!(phone_resize(&id, "iPhone", 40, 20));
        let g1 = generation();
        assert!(g1 > g0);
        assert!(phone_resize(&id, "iPhone", 40, 20));
        assert_eq!(generation(), g1, "same state, no re-render");
        forget(&id);
    }
}
