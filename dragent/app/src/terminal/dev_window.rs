//! Dev-only window with a single [`TerminalView`] pane.
//!
//! Enable with `DRAGENT_DEV_TERMINAL=1 cargo run -p dragent --features performance-harness`.

use std::path::PathBuf;

use dirs;

use warpui::elements::{ChildView, Element};
use warpui::{AppContext, Entity, TypedActionView, View, ViewContext};

use crate::terminal::view::TerminalView;

pub struct DevTerminalRoot {
    terminal: warpui::ViewHandle<TerminalView>,
}

impl DevTerminalRoot {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let cwd = dirs::home_dir()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));
        let terminal = ctx.add_typed_action_view(|ctx| TerminalView::new(cwd, None, ctx));
        Self { terminal }
    }
}

impl Entity for DevTerminalRoot {
    type Event = ();
}

impl View for DevTerminalRoot {
    fn ui_name() -> &'static str {
        "DevTerminalRoot"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        ChildView::new(&self.terminal).finish()
    }
}

impl TypedActionView for DevTerminalRoot {
    type Action = ();
}

pub fn open_dev_window(ctx: &mut warpui::AppContext) {
    ctx.add_window(warpui::AddWindowOptions::default(), DevTerminalRoot::new);
}
