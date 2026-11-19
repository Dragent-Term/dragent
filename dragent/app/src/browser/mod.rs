//! Built-in browser panes. A browser opens like a terminal (header `+`,
//! sidebar new-tab menu, splits) and shows a page with the system web engine.
//!
//! Memory: a pane starts its engine only when it is first shown, a hidden
//! engine is released after [`engines::IDLE_RELEASE`], and at most
//! [`engines::MAX_HIDDEN`] hidden engines stay alive at once. Pages the user
//! has typed into are kept until the pane closes.
pub mod agent;
mod app_links;
mod design;
#[cfg(not(target_family = "wasm"))]
mod favicon;
mod find;
mod history;
mod native;
mod surface;
mod user_agent;
mod view;

pub use view::{BrowserAction, BrowserEvent, BrowserView};

/// Browser shortcuts, active while a browser pane (or its page) has focus.
pub fn register_bindings(ctx: &mut warpui::AppContext) {
    use warpui::keymap::macros::*;
    use warpui::keymap::FixedBinding;
    let context = id!("BrowserView");
    ctx.register_fixed_bindings([
        FixedBinding::new(
            "cmdorctrl-l",
            BrowserAction::FocusAddressBar,
            context.clone(),
        ),
        FixedBinding::new("cmdorctrl-r", BrowserAction::Reload, context.clone()),
        FixedBinding::new(
            "cmdorctrl-shift-R",
            BrowserAction::HardReload,
            context.clone(),
        ),
        FixedBinding::new("cmdorctrl-[", BrowserAction::Back, context.clone()),
        FixedBinding::new("cmdorctrl-]", BrowserAction::Forward, context.clone()),
        FixedBinding::new("cmdorctrl-f", BrowserAction::Find, context.clone()),
        FixedBinding::new("cmdorctrl-g", BrowserAction::FindNext(1), context.clone()),
        FixedBinding::new("cmdorctrl-shift-G", BrowserAction::FindNext(-1), context.clone()),
        FixedBinding::new("cmdorctrl-=", BrowserAction::Zoom(1), context.clone()),
        FixedBinding::new("cmdorctrl--", BrowserAction::Zoom(-1), context.clone()),
        FixedBinding::new("cmdorctrl-0", BrowserAction::Zoom(0), context.clone()),
        FixedBinding::new("cmdorctrl-shift-M", BrowserAction::ToggleMobile, context.clone()),
        FixedBinding::new("cmdorctrl-shift-D", BrowserAction::ToggleDesign, context.clone()),
        // A real keystroke, not page script, sends design notes to an agent.
        FixedBinding::new("cmdorctrl-enter", BrowserAction::SendDesign, context.clone()),
        FixedBinding::new("escape", BrowserAction::Escape, context),
    ]);
}

pub(crate) mod engines {
    use std::cell::RefCell;
    use std::rc::{Rc, Weak};
    use std::time::Duration;

    use super::surface::SurfaceState;

    /// A hidden page's engine is released after this long.
    pub const IDLE_RELEASE: Duration = Duration::from_secs(5 * 60);
    /// Hidden engines kept alive beyond the visible ones.
    pub const MAX_HIDDEN: usize = 2;

    thread_local! {
        static LIVE: RefCell<Vec<Weak<RefCell<SurfaceState>>>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn register(surface: &Rc<RefCell<SurfaceState>>) {
        LIVE.with(|live| {
            let mut live = live.borrow_mut();
            let known = live.iter().any(|entry| {
                entry
                    .upgrade()
                    .is_some_and(|entry| Rc::ptr_eq(&entry, surface))
            });
            if !known {
                live.push(Rc::downgrade(surface));
            }
        });
    }

    /// Before starting another engine, release the least recently seen hidden
    /// ones beyond the budget. Visible and edited pages are never released.
    pub(super) fn make_room(starting: &Rc<RefCell<SurfaceState>>) {
        LIVE.with(|live| {
            let mut live = live.borrow_mut();
            live.retain(|surface| {
                surface.upgrade().is_some_and(|surface| {
                    surface.try_borrow().map_or(true, |s| s.engine.is_some())
                })
            });
            let mut hidden: Vec<Rc<RefCell<SurfaceState>>> = live
                .iter()
                .filter_map(Weak::upgrade)
                .filter(|surface| !Rc::ptr_eq(surface, starting))
                .filter(|surface| {
                    surface
                        .try_borrow()
                        .is_ok_and(|s| s.engine.is_some() && s.placement.is_none() && !s.keep_alive)
                })
                .collect();
            if hidden.len() <= MAX_HIDDEN {
                return;
            }
            hidden.sort_by_key(|surface| surface.borrow().last_visible);
            for surface in hidden.iter().take(hidden.len() - MAX_HIDDEN) {
                let engine = surface.borrow_mut().engine.take();
                drop(engine);
            }
        });
    }
}

pub(crate) mod ports {
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;

    use dragent_workspace_model::BrowserUrl;

    /// Ports that common development servers listen on. 5000 and 7000 are
    /// left out: macOS uses them for AirPlay.
    const DEV_PORTS: &[u16] = &[
        3000, 3001, 3002, 4000, 4200, 4321, 5173, 5174, 6006, 8000, 8080, 8081, 8787, 8888, 1420,
        9000,
    ];

    pub fn is_listening(host: &str, port: u16) -> bool {
        let Ok(addresses) = (host, port).to_socket_addrs() else {
            return false;
        };
        addresses
            .into_iter()
            .any(|address| TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_ok())
    }

    pub fn running_local_servers() -> Vec<BrowserUrl> {
        DEV_PORTS
            .iter()
            .filter(|port| is_listening("localhost", **port))
            .filter_map(|port| BrowserUrl::parse(&format!("http://localhost:{port}")).ok())
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn detects_a_listening_loopback_port() {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            assert!(is_listening("127.0.0.1", port));
            drop(listener);
            assert!(!is_listening("127.0.0.1", port));
        }
    }
}
