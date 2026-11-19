//! Whether an agent pane's terminal is drawing right now, for status shown
//! outside the pane (sidebar, tab strip).
//!
//! Agents report working/finished through their hooks. A conversation whose
//! agent loop runs where those hooks aren't installed (Claude Code's
//! background sessions viewed with `claude agents` / `claude attach`, an
//! agent started before Dragent could add them) reports nothing; for those
//! the terminal's own output is the only evidence, read the same way the
//! chat view reads it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Weak;

use parking_lot::FairMutex;
use uuid::Uuid;

use crate::terminal::model::TerminalSessionCore;

type Core = Weak<FairMutex<TerminalSessionCore>>;

thread_local! {
    static PANES: RefCell<HashMap<Uuid, Core>> = RefCell::new(HashMap::new());
}

pub(crate) fn register(pane: Uuid, core: Core) {
    PANES.with(|panes| {
        let mut panes = panes.borrow_mut();
        panes.retain(|_, core| core.strong_count() > 0);
        panes.insert(pane, core);
    });
}

/// The pane took another id (its saved session was restored into it).
pub(crate) fn rebind(old: Uuid, new: Uuid) {
    PANES.with(|panes| {
        let mut panes = panes.borrow_mut();
        if let Some(core) = panes.remove(&old) {
            panes.insert(new, core);
        }
    });
}

/// Whether `pane`'s terminal is streaming output now (an agent drawing its
/// working state), `None` when the pane isn't known here.
pub(crate) fn busy(pane: Uuid) -> Option<bool> {
    let core = PANES.with(|panes| panes.borrow().get(&pane).and_then(Weak::upgrade))?;
    let (output_at, run_since) = {
        let core = core.lock();
        (core.output_at, core.output_run_since)
    };
    Some(crate::terminal::view::terminal_cli_chat::busy_from_output(
        web_time::Instant::now(),
        output_at,
        run_since,
        None,
    ))
}
