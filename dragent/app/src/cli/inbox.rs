//! The open app's side of [`super::requests`]: picks up command-line
//! requests and offers each to the windows; the first to take it does it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use warpui::r#async::Timer;
use warpui::{Entity, ModelContext, SingletonEntity};

use super::requests::{self, Request, Response};

const POLL: Duration = Duration::from_secs(1);
/// Nobody took a request in this long: no window is open to do it.
const UNTAKEN: Duration = Duration::from_secs(3);

pub enum CliInboxEvent {
    /// Request `id` is waiting; take it with [`CliInbox::take`].
    Request(String),
}

pub struct CliInbox {
    pending: HashMap<String, (Request, Instant)>,
    /// A window is listening. Until then requests wait in the folder: an
    /// app started by `dragent open` takes them once its window is up.
    window_ready: bool,
    /// The organisation the `dragent` launchers were last written for.
    launchers_for: Option<String>,
}

impl Entity for CliInbox {
    type Event = CliInboxEvent;
}

impl SingletonEntity for CliInbox {}

impl CliInbox {
    /// An inert inbox for native UI tests: no launchers, agent configuration,
    /// filesystem polling, or messages to a running app.
    #[cfg(test)]
    pub(crate) fn preview(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            pending: HashMap::new(),
            window_ready: false,
            launchers_for: None,
        }
    }

    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        ctx.spawn(Timer::after(POLL), |this, _, ctx| this.poll(ctx));
        let mut this = Self {
            pending: HashMap::new(),
            window_ready: false,
            launchers_for: None,
        };
        this.ensure_launchers();
        // Other agents' copy of Dragent's skill, per the user's setting.
        let outcome = super::agent_skill::sync();
        if !outcome.written.is_empty() {
            log::info!("[cli] dragent skill written to {:?}", outcome.written);
        }
        // And the connected services' tools in their MCP settings.
        crate::integrations::agents::sync_in_background();
        this
    }

    /// `dragent` works from the first launch, in the organisation selected
    /// now: written at startup and again after a switch, pointing at this
    /// copy of the app (which may have moved or been updated).
    fn ensure_launchers(&mut self) {
        let organisation = dragent_paths::organisation_id();
        if self.launchers_for.as_deref() == Some(organisation.as_str()) {
            return;
        }
        match super::install_launchers() {
            Ok(_) => self.launchers_for = Some(organisation),
            Err(error) => log::warn!("[cli] command-line launcher: {error}"),
        }
    }

    fn poll(&mut self, ctx: &mut ModelContext<Self>) {
        self.ensure_launchers();
        self.answer_identity(ctx);
        crate::integrations::remote::refresh_if_stale();
        let folder = requests::folder();
        let untaken: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, (_, since))| since.elapsed() > UNTAKEN)
            .map(|(id, _)| id.clone())
            .collect();
        for id in untaken {
            self.pending.remove(&id);
            requests::respond(
                &folder,
                &id,
                &Response::failed("Dragent has no window open. Open one and try again."),
            );
        }
        let claimed = if self.window_ready {
            requests::claim_all(&folder)
        } else {
            Vec::new()
        };
        for (id, request) in claimed {
            self.pending.insert(id.clone(), (request, Instant::now()));
            ctx.emit(CliInboxEvent::Request(id));
        }
        ctx.spawn(Timer::after(POLL), |this, _, ctx| this.poll(ctx));
    }

    fn answer_identity(&self, ctx: &ModelContext<Self>) {
        use crate::auth::{AuthManager, AuthStatus};
        let AuthStatus::LoggedIn(user) = AuthManager::as_ref(ctx).status() else {
            // A signed-out development copy must not answer for another,
            // signed-in instance. Leave discovery to that instance.
            return;
        };
        let organisation = crate::organisation_context::OrganisationContext::as_ref(ctx);
        let Some(active) = organisation.active().filter(|active| {
            !organisation.is_switching()
                && user.user_id.as_deref() == Some(active.user_id.as_str())
        }) else {
            return;
        };
        let data = serde_json::json!({
            "signed_in": true,
            "user_id": active.user_id,
            "organisation": active.name,
            "organisation_id": active.organisation_id,
            "context": active.context,
        });
        for folder in [requests::identity_folder(), requests::scoped_identity_folder(&active.context)] {
            for (id, request) in requests::claim_all(&folder) {
                let response = if matches!(request, Request::Whoami) {
                    Response::with_data(data.clone())
                } else {
                    Response::failed("Only identity discovery is supported here.")
                };
                requests::respond(&folder, &id, &response);
            }
        }
    }

    /// A window subscribed and can take requests.
    pub fn window_ready(&mut self) {
        self.window_ready = true;
    }

    /// The request, for the window that will answer it (once).
    pub fn take(&mut self, id: &str) -> Option<Request> {
        self.pending.remove(id).map(|(request, _)| request)
    }

    pub fn respond(id: &str, response: &Response) {
        requests::respond(&requests::folder(), id, response);
    }
}
