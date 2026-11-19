//! Computer use for agents (`dragent computer`, the `computer_*` MCP tools,
//! Dragent's own agent). Settings → Computer use decides first: off refuses
//! everything but the permission check, apps outside the allowed list are
//! refused, and clicks, typing, keys and values wait for the person's Allow.
//! The work itself runs off the UI thread.

use std::collections::VecDeque;
use std::time::Duration;

use dragent_computer::{Preview, Request as ComputerRequest};
use dragent_host::cli_requests::Response;
use serde_json::json;
use warpui::elements::{
    Container, CrossAxisAlignment, Element, Flex, MouseStateHandle, ParentElement,
};
use warpui::fonts::FamilyId;
use warpui::{EntityId, SingletonEntity, ViewContext};
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::view::WorkspaceView;
use super::WorkspaceAction;
use crate::cli::inbox::CliInbox;
use crate::icons::AppIcon;
use crate::machine_ui as mui;
use crate::theme::Theme;

/// An action waiting for the person's answer.
pub(crate) struct ComputerAsk {
    id: String,
    scope: String,
    request: ComputerRequest,
    pub(crate) preview: Preview,
    /// The tab the agent runs in.
    pub(crate) caller: String,
}

impl ComputerAsk {
    pub(crate) fn request_id(&self) -> &str {
        &self.id
    }
}

/// After this, an unanswered request is turned down: the agent stopped
/// waiting by then.
const ASK_FOR: Duration = Duration::from_secs(110);

const TURNED_OFF: &str =
    "Computer use is off in Dragent. The person can turn it on in Settings → Computer use.";

fn respond(id: &str, result: Result<dragent_computer::Outcome, String>) {
    CliInbox::respond(
        id,
        &match result {
            Ok(outcome) => {
                Response::with_data(json!({ "output": outcome.text, "result": outcome.data }))
            }
            Err(error) => {
                let mut response = Response::failed(error.clone());
                response.data = Some(dragent_computer::errors::details(&error));
                response
            }
        },
    );
}

/// Run `job` on a background thread and hand back its result.
async fn off_thread<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = futures::channel::oneshot::channel();
    crate::bg::detach(move || {
        let _ = tx.send(job());
    });
    rx.await.ok()
}

impl WorkspaceView {
    pub(crate) fn computer_tool_for_cli(
        &mut self,
        id: &str,
        pane: Option<&str>,
        terminal: Option<&str>,
        tool: &str,
        arguments: &serde_json::Value,
        ctx: &mut ViewContext<Self>,
    ) -> Option<Response> {
        let request = match ComputerRequest::from_tool(tool, arguments) {
            Ok(request) => request,
            Err(error) => return Some(Response::failed(error)),
        };
        let settings = crate::settings::AppSettings::global().computer_use.clone();
        if !request.is_diagnostic() && !settings.enabled {
            return Some(Response::failed(TURNED_OFF));
        }
        let caller = self
            .cli_caller_tab(pane, terminal, ctx)
            .and_then(|(_, tab_id)| {
                self.surfaces()
                    .flat_map(|(_, panes)| panes.iter())
                    .find(|tab| tab.id == tab_id)
                    .map(|tab| {
                        tab.custom_label
                            .clone()
                            .unwrap_or_else(|| tab.title.clone())
                    })
            })
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "An agent".into());
        let scope = format!(
            "{}/{}",
            dragent_paths::organisation_id(),
            pane.or(terminal).unwrap_or("terminal")
        );
        let preview_scope = scope.clone();
        let id = id.to_string();
        let preview_request = request.clone();
        ctx.spawn(
            off_thread(move || dragent_computer::preview_for(&preview_scope, preview_request)),
            move |view, preview, ctx| {
                let preview = match preview {
                    Some(Ok(preview)) => preview,
                    Some(Err(error)) => return respond(&id, Err(error)),
                    None => return respond(&id, Err("Computer use stopped unexpectedly.".into())),
                };
                let settings = crate::settings::AppSettings::global().computer_use.clone();
                if !request.is_diagnostic() && !settings.enabled {
                    return respond(&id, Err(TURNED_OFF.into()));
                }
                if let Err(error) = check_access(&settings, &request, &preview, true) {
                    return respond(&id, Err(error));
                }
                let trusted = view.computer_trusted_apps.contains(&preview.app_id);
                if request.changes_things() && settings.require_confirmation && !trusted {
                    view.ask_before_computer_action(id, scope, request, preview, caller, ctx);
                } else {
                    view.run_computer_request(id, scope, request, preview, trusted, ctx);
                }
            },
        );
        None
    }

    fn ask_before_computer_action(
        &mut self,
        id: String,
        scope: String,
        request: ComputerRequest,
        preview: Preview,
        caller: String,
        ctx: &mut ViewContext<Self>,
    ) {
        self.computer_asks.push_back(ComputerAsk {
            id: id.clone(),
            scope,
            request,
            preview,
            caller,
        });
        ctx.spawn(
            warpui::r#async::Timer::after(ASK_FOR),
            move |view, _, ctx| {
                if let Some(position) = view.computer_asks.iter().position(|ask| ask.id == id) {
                    view.computer_asks.remove(position);
                    respond(
                        &id,
                        Err("No one answered in time, so nothing was done.".into()),
                    );
                    ctx.notify();
                }
            },
        );
        ctx.notify();
    }

    fn run_computer_request(
        &mut self,
        id: String,
        scope: String,
        request: ComputerRequest,
        preview: Preview,
        confirmed: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        if !scope.starts_with(&format!("{}/", dragent_paths::organisation_id()))
            || !crate::auth::AuthManager::as_ref(ctx).is_logged_in()
        {
            respond(
                &id,
                Err("The desktop account or organisation changed. Make a new request.".into()),
            );
            return;
        }
        let settings = crate::settings::AppSettings::global().computer_use.clone();
        if let Err(error) = check_access(&settings, &request, &preview, confirmed) {
            respond(&id, Err(error));
            return;
        }
        let active_session = crate::organisation_context::cached_active();
        ctx.spawn(
            off_thread(move || {
                let guard_request = request.clone();
                let guard_preview = preview.clone();
                let guard_scope = scope.clone();
                let mut outcome = dragent_computer::run_for_guarded(
                    &scope,
                    request.clone(),
                    Some(preview),
                    move || {
                        check_session(
                            &guard_scope,
                            active_session.as_ref(),
                            crate::organisation_context::cached_active().as_ref(),
                            &dragent_paths::organisation_id(),
                        )?;
                        let settings = crate::settings::AppSettings::global().computer_use.clone();
                        check_access(&settings, &guard_request, &guard_preview, confirmed)
                    },
                )?;
                if request.is_app_list() {
                    let settings = crate::settings::AppSettings::global().computer_use.clone();
                    if let Some(apps) = outcome.data["apps"].as_array_mut() {
                        apps.retain(|app| {
                            settings.is_app_allowed(app["name"].as_str().unwrap_or_default())
                                || settings.is_app_allowed(app["id"].as_str().unwrap_or_default())
                        });
                        outcome.text = apps
                            .iter()
                            .map(|app| {
                                format!(
                                    "{} — {} (pid {})",
                                    app["name"].as_str().unwrap_or_default(),
                                    app["id"].as_str().unwrap_or_default(),
                                    app["pid"]
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                    }
                }
                Ok(outcome)
            }),
            move |_, result, _| {
                respond(
                    &id,
                    result.unwrap_or_else(|| Err("Computer use stopped unexpectedly.".into())),
                );
            },
        );
    }

    /// The person's answer to the action shown: Allow, Allow for this app
    /// until this window closes, or Deny.
    pub(crate) fn answer_computer_ask(
        &mut self,
        request_id: &str,
        allow: bool,
        remember: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(ask) = take_answer(&mut self.computer_asks, request_id) else {
            return;
        };
        if !allow {
            respond(
                &ask.id,
                Err(format!(
                    "The person declined: {}. Nothing was done.",
                    ask.preview.action
                )),
            );
        } else {
            if remember {
                self.computer_trusted_apps
                    .insert(ask.preview.app_id.clone());
            }
            self.run_computer_request(ask.id, ask.scope, ask.request, ask.preview, true, ctx);
        }
        ctx.notify();
    }
}

pub(crate) type ComputerAsks = VecDeque<ComputerAsk>;

fn take_answer(asks: &mut ComputerAsks, request_id: &str) -> Option<ComputerAsk> {
    if asks.front().is_none_or(|ask| ask.id != request_id) {
        return None;
    }
    asks.pop_front()
}

fn check_session(
    scope: &str,
    expected: Option<&crate::organisation_context::ActiveOrganisation>,
    current: Option<&crate::organisation_context::ActiveOrganisation>,
    context: &str,
) -> Result<(), String> {
    let unchanged = expected.zip(current).is_some_and(|(expected, current)| {
        expected.user_id == current.user_id
            && expected.organisation_id == current.organisation_id
            && expected.context == current.context
            && current.context == context
    });
    if unchanged && scope.starts_with(&format!("{context}/")) {
        Ok(())
    } else {
        Err(
            "The desktop account or organisation changed or is still loading. Make a new request."
                .into(),
        )
    }
}

/// One gate for CLI, MCP and the native agent, rechecked after approval.
fn check_access(
    settings: &crate::settings::ComputerUseSettings,
    request: &ComputerRequest,
    preview: &Preview,
    confirmed: bool,
) -> Result<(), String> {
    if request.is_diagnostic() {
        return Ok(());
    }
    if !settings.enabled {
        return Err(TURNED_OFF.into());
    }
    if request.app().is_none() {
        return Ok(());
    }
    let app = if settings.is_app_allowed(&preview.app_name) {
        &preview.app_name
    } else {
        &preview.app_id
    };
    crate::settings::shortcut_settings::check_computer_use(
        settings,
        &crate::settings::ComputerUseRequest {
            app: app.clone(),
            detail: preview.action.clone(),
        },
        confirmed || !request.changes_things(),
    )
}

/// The confirmation for the first waiting action.
pub(crate) fn render_computer_ask(
    ask: &ComputerAsk,
    waiting: usize,
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    hover: &dyn Fn(&str) -> MouseStateHandle,
    scroll: &warpui::elements::ClippedScrollStateHandle,
) -> Box<dyn Element> {
    let app = &ask.preview.app_name;
    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
        .with_child(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(4.0)
                    .with_child(mui::text(ui, "It wants to", 12.0, theme.sub_text()))
                    .with_child(mui::semibold(ui, &ask.preview.action, 15.0, theme.main_text()))
                    .finish(),
            )
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
            .with_padding_left(14.0)
            .with_padding_right(14.0)
            .with_padding_top(12.0)
            .with_padding_bottom(12.0)
            .finish(),
        )
        .with_child(mui::text(
            ui,
            format!("Dragent does it as you would: the click or keys go to {app}. Choosing Allow for this app remembers approval until this Dragent window closes."),
            12.0,
            theme.sub_text(),
        ));
    if waiting > 1 {
        body = body.with_child(mui::text(
            ui,
            format!("{} more waiting after this.", waiting - 1),
            12.0,
            theme.sub_text(),
        ));
    }
    let answer = |allow: bool, remember: bool| WorkspaceAction::AnswerComputerAsk {
        request_id: ask.id.clone(),
        allow,
        remember,
    };
    let footer = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(mui::button(
            theme,
            ui,
            view_id,
            hover("computer-ask-deny"),
            None,
            "Deny",
            mui::Variant::Ghost,
            mui::Size::Md,
            true,
            answer(false, false),
        ))
        .with_child(
            warpui::elements::Expanded::new(1.0, warpui::elements::Empty::new().finish()).finish(),
        )
        .with_child(mui::button(
            theme,
            ui,
            view_id,
            hover("computer-ask-remember"),
            None,
            "Allow for this app",
            mui::Variant::Secondary,
            mui::Size::Md,
            true,
            answer(true, true),
        ))
        .with_child(mui::button(
            theme,
            ui,
            view_id,
            hover("computer-ask-allow"),
            None,
            "Allow",
            mui::Variant::Primary,
            mui::Size::Md,
            true,
            answer(true, false),
        ))
        .finish();
    let dismiss = answer(false, false);
    let dialog = mui::dialog(
        theme,
        ui,
        view_id,
        hover("computer-ask-close"),
        scroll,
        mui::dialog_title(
            theme,
            ui,
            AppIcon::Monitor,
            &format!("{} wants to use {app}", ask.caller),
            Some("Computer use"),
        ),
        body.finish(),
        Some(footer),
        520.0,
        Some(dismiss.clone()),
    );
    warpui::elements::EventHandler::new(dialog)
        .on_keydown(move |ctx, _, key| {
            if key.key == "escape" {
                ctx.dispatch_typed_action_for(view_id, dismiss.clone());
            }
            warpui::elements::DispatchEventResult::StopPropagation
        })
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_requests_reject_sign_out_and_account_or_organisation_changes() {
        let active = crate::organisation_context::ActiveOrganisation {
            user_id: "person".into(),
            organisation_id: "org".into(),
            name: "Team".into(),
            context: "local".into(),
        };
        let check = |current: Option<&crate::organisation_context::ActiveOrganisation>, context| {
            check_session("local/pane", Some(&active), current, context)
        };
        assert!(check(Some(&active), "local").is_ok());
        assert!(check(None, "local").is_err());
        assert!(check(Some(&active), "other").is_err());
        let mut changed = active.clone();
        changed.user_id = "someone-else".into();
        assert!(check(Some(&changed), "local").is_err());
        changed = active.clone();
        changed.organisation_id = "another-org".into();
        assert!(check(Some(&changed), "local").is_err());
        assert!(check_session("local/pane", None, None, "local").is_err());
        assert!(check_session("other/pane", Some(&active), Some(&active), "local").is_err());
    }
    fn preview() -> Preview {
        Preview {
            pid: Some(42),
            snapshot_id: None,
            target_window: None,
            app_name: "Sample".into(),
            app_id: "sample.app".into(),
            action: "Click Save".into(),
        }
    }
    #[test]
    fn settings_gate_reads_changes_and_revocation() {
        let preview = preview();
        let mut settings = crate::settings::ComputerUseSettings::default();
        let read = ComputerRequest::Windows {
            app: "Sample".into(),
        };
        let change = ComputerRequest::Key {
            app: "Sample".into(),
            key: "Enter".into(),
        };
        assert!(check_access(&settings, &ComputerRequest::Permissions, &preview, false).is_ok());
        assert!(check_access(&settings, &read, &preview, true).is_err());
        settings.enabled = true;
        assert!(check_access(&settings, &read, &preview, false).is_ok());
        assert!(check_access(&settings, &change, &preview, false).is_err());
        assert!(check_access(&settings, &change, &preview, true).is_ok());
        settings.allowed_apps = vec!["sample.app".into()];
        assert!(check_access(&settings, &change, &preview, true).is_ok());
        settings.allowed_apps = vec!["Different app".into()];
        assert!(check_access(&settings, &read, &preview, true).is_err());
        assert!(check_access(&settings, &change, &preview, true).is_err());
        settings.enabled = false;
        assert!(check_access(&settings, &change, &preview, true).is_err());
    }
}

/// Used by the sample-only UI capture runner.
pub(crate) fn render_computer_preview(
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    hover: &dyn Fn(&str) -> MouseStateHandle,
    scroll: &warpui::elements::ClippedScrollStateHandle,
) -> Box<dyn Element> {
    let ask = ComputerAsk {
        id: "preview".into(),
        scope: "preview".into(),
        request: ComputerRequest::Key {
            app: "TextEdit".into(),
            key: "cmd+s".into(),
        },
        preview: Preview {
            pid: None,
            snapshot_id: None,
            target_window: None,
            app_name: "TextEdit".into(),
            app_id: "com.apple.TextEdit".into(),
            action: "Press cmd+s".into(),
        },
        caller: "Dragent agent".into(),
    };
    render_computer_ask(&ask, 2, theme, ui, view_id, hover, scroll)
}

#[cfg(test)]
mod queue_tests {
    use super::*;
    #[test]
    fn an_expired_prompts_allow_button_cannot_approve_the_next_action() {
        let ask = |id: &str| ComputerAsk {
            id: id.into(),
            scope: "test".into(),
            request: ComputerRequest::Apps,
            preview: Preview {
                pid: None,
                snapshot_id: None,
                target_window: None,
                app_name: String::new(),
                app_id: String::new(),
                action: String::new(),
            },
            caller: "test".into(),
        };
        let mut asks = ComputerAsks::from([ask("expired"), ask("next")]);
        asks.pop_front();
        assert!(take_answer(&mut asks, "expired").is_none());
        assert_eq!(asks.front().unwrap().id, "next");
        assert_eq!(take_answer(&mut asks, "next").unwrap().id, "next");
        assert!(asks.is_empty());
    }
}
