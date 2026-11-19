//! Keeps the desktop answering its phones while it's in the background.
//!
//! macOS slows down an app the person isn't looking at ("App Nap"): its
//! timers fire late and its threads run rarely. The phone link is served from
//! background threads that wait on short timeouts, so a napping desktop
//! answered a returning phone late or not at all, and the phone sat on
//! "loading" until the desktop window was brought to the front. While a
//! paired phone may be talking to this desktop we tell macOS the app is
//! doing work the person asked for; the system can still sleep when idle.
//!
//! Other platforms don't nap background apps this way: nothing to do there.

/// Held while phones are served; dropping it lets the app nap again.
pub(crate) struct Responsive {
    #[cfg(target_os = "macos")]
    activity: Option<mac::Activity>,
}

/// Start keeping the app responsive, for `reason` (shown by `pmset -g assertions`).
pub(crate) fn hold(reason: &str) -> Responsive {
    #[cfg(target_os = "macos")]
    {
        Responsive { activity: mac::Activity::begin(reason) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = reason;
        Responsive {}
    }
}

impl Responsive {
    /// Whether macOS accepted the request (always false off macOS).
    #[cfg(test)]
    pub(crate) fn active(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.activity.is_some()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::rc::Retained;
    use objc2::runtime::{NSObjectProtocol, ProtocolObject};
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};

    pub(super) struct Activity {
        token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    }

    // The token is only handed back to `endActivity:`, which Foundation
    // documents as callable from any thread.
    unsafe impl Send for Activity {}

    impl Activity {
        pub(super) fn begin(reason: &str) -> Option<Self> {
            let info = NSProcessInfo::processInfo();
            let token = info.beginActivityWithOptions_reason(
                NSActivityOptions::UserInitiatedAllowingIdleSystemSleep,
                &NSString::from_str(reason),
            );
            Some(Self { token })
        }
    }

    impl Drop for Activity {
        fn drop(&mut self) {
            // SAFETY: the token came from beginActivityWithOptions:reason:.
            unsafe { NSProcessInfo::processInfo().endActivity(&self.token) };
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn holding_and_dropping_works_on_every_platform() {
        let held = super::hold("Dragent test");
        assert_eq!(held.active(), cfg!(target_os = "macos"));
        drop(held);
    }
}
