//! "Give feedback": a dialog for telling the team what's working and what
//! isn't, with an optional reply-to email, screenshots, and redacted logs.
//! Submissions go to `POST /api/v1/feedback`; a GitHub issue is offered for
//! anything that should be tracked in public.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use warpui::color::ColorU;
use warpui::elements::{
    ChildView, ConstrainedBox, Container, CrossAxisAlignment, Element, Flex, MainAxisAlignment,
    MainAxisSize, ParentElement, Text, Wrap,
};
use warpui::fonts::FamilyId;
use warpui::r#async::Timer;
use warpui::{EntityId, UpdateView, ViewContext, ViewHandle};
use warpui_core::elements::{DispatchEventResult, EventHandler, Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon};
use crate::theme::{scaled, Theme};

use super::action::WorkspaceAction;
use super::view::WorkspaceView;
use super::{InputEditor, InputMode};

type States = RefCell<HashMap<String, MouseStateHandle>>;

const MAX_FILES: usize = 10;
const MAX_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
const PANEL_WIDTH: f32 = 470.0;

pub(crate) struct FeedbackImage {
    name: String,
    mime: &'static str,
    data: Vec<u8>,
}

pub(crate) struct FeedbackDraft {
    details_editor: ViewHandle<InputEditor>,
    contact_editor: ViewHandle<InputEditor>,
    include_logs: bool,
    images: Vec<FeedbackImage>,
    submitting: bool,
    /// Sent: the dialog thanks the user, then closes itself.
    sent: bool,
    error: Option<String>,
    contact_error: Option<String>,
}

/// Feedback text followed by the context we append, one fact per line.
pub(crate) fn build_content(
    feedback: &str,
    contact: &str,
    account: &str,
    organisation: &str,
    version: &str,
    platform: &str,
    diagnostic_logs: bool,
) -> String {
    let mut meta = Vec::new();
    if !contact.trim().is_empty() {
        meta.push(format!("Reply-to: {}", contact.trim()));
    }
    if !account.is_empty() {
        meta.push(format!("Account: {account}"));
    }
    if !organisation.is_empty() {
        meta.push(format!("Organisation: {organisation}"));
    }
    meta.push(format!("App version: {version}"));
    meta.push(format!("OS: {platform}"));
    if diagnostic_logs {
        meta.push("Logs: included (redacted)".into());
    }
    format!("{}\n\n{}", feedback.trim(), meta.join("\n"))
}

/// Empty is fine (the field is optional); otherwise it must look like an email.
pub(crate) fn contact_error(contact: &str) -> Option<&'static str> {
    let contact = contact.trim();
    if contact.is_empty() {
        return None;
    }
    let valid = contact.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty()
            && domain.contains('.')
            && !domain.starts_with('.')
            && !domain.ends_with('.')
            && !contact.chars().any(char::is_whitespace)
    });
    (!valid).then_some("Please enter a valid email address.")
}

pub(crate) fn platform_name() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

impl WorkspaceView {
    pub(crate) fn handle_feedback_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::OpenFeedback => self.open_feedback(ctx),
            WorkspaceAction::FocusFeedbackDetails => {
                if let Some(draft) = &self.feedback {
                    let editor = draft.details_editor.clone();
                    ctx.focus(&editor);
                }
            }
            WorkspaceAction::CloseFeedback => {
                if self.feedback.as_ref().is_some_and(|d| d.submitting) {
                    return;
                }
                self.feedback = None;
            }
            WorkspaceAction::ToggleFeedbackDiagnostics => {
                if let Some(draft) = self.feedback.as_mut() {
                    draft.include_logs = !draft.include_logs;
                }
            }
            WorkspaceAction::AttachFeedbackImages => {
                let (tx, rx) = async_channel::bounded::<Vec<std::path::PathBuf>>(1);
                ctx.spawn_stream_local(
                    rx,
                    |view, paths: Vec<std::path::PathBuf>, ctx| view.receive_feedback_images(paths, ctx),
                    |_, _| {},
                );
                #[cfg(not(target_family = "wasm"))]
                crate::agent::attachments::pick_files_async(tx);
                #[cfg(target_family = "wasm")]
                drop(tx);
            }
            WorkspaceAction::RemoveFeedbackAttachment { index } => {
                if let Some(draft) = self.feedback.as_mut() {
                    if *index < draft.images.len() && !draft.submitting {
                        draft.images.remove(*index);
                        draft.error = None;
                    }
                }
            }
            WorkspaceAction::SubmitFeedback => self.submit_feedback(ctx),
            _ => return,
        }
        ctx.notify();
    }

    fn open_feedback(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(draft) = &self.feedback {
            let editor = draft.details_editor.clone();
            ctx.focus(&editor);
            return;
        }
        let theme = self.theme;
        let ui_font = self.ui_font;
        let details_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "Tell us what's working well and what we should fix…",
                ui_font,
                crate::theme::text_size(13.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::MultiLine {
                    key: "feedback_details".into(),
                    max_height: 180,
                },
                ed_ctx.focus_handle(),
            )
        });
        let contact_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "Email for follow-up (optional)",
                ui_font,
                crate::theme::text_size(13.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "feedback_contact".into(),
                },
                ed_ctx.focus_handle(),
            )
        });
        // Typing clears the message about the previous attempt.
        for editor in [&details_editor, &contact_editor] {
            ctx.subscribe_to_view(editor, |view, _, _event, ctx| {
                if let Some(draft) = view.feedback.as_mut() {
                    if draft.error.is_some() || draft.contact_error.is_some() {
                        draft.error = None;
                        draft.contact_error = None;
                        ctx.notify();
                    }
                }
            });
        }
        ctx.focus(&details_editor);
        self.feedback = Some(FeedbackDraft {
            details_editor,
            contact_editor,
            include_logs: false,
            images: Vec::new(),
            submitting: false,
            sent: false,
            error: None,
            contact_error: None,
        });
    }

    fn receive_feedback_images(&mut self, paths: Vec<std::path::PathBuf>, ctx: &mut ViewContext<Self>) {
        let Some(draft) = self.feedback.as_mut() else {
            return;
        };
        for path in paths {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            let Some(mime) = crate::agent::attachments::media_type_for(&path) else {
                draft.error = Some(format!("{name} isn't an image (png, jpg, webp, gif)."));
                continue;
            };
            if draft.images.len() >= MAX_FILES {
                draft.error = Some(format!("Only {MAX_FILES} files can be added."));
                break;
            }
            match std::fs::read(&path) {
                Ok(data) => draft.images.push(FeedbackImage { name, mime, data }),
                Err(error) => draft.error = Some(format!("Couldn't read {name}: {error}")),
            }
        }
        ctx.notify();
    }

    fn submit_feedback(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(draft) = self.feedback.as_ref() else {
            return;
        };
        if draft.submitting {
            return;
        }
        let details = ctx.update_view(&draft.details_editor, |e, _| e.get_text().to_string());
        let contact = ctx.update_view(&draft.contact_editor, |e, _| e.get_text().to_string());
        let include_logs = draft.include_logs;
        let draft = self.feedback.as_mut().expect("checked above");
        if details.trim().is_empty() {
            draft.error = Some("Write a few words of feedback first.".into());
            return;
        }
        if let Some(error) = contact_error(&contact) {
            draft.contact_error = Some(error.into());
            return;
        }
        if !crate::auth::AuthManager::as_ref(ctx).is_logged_in() {
            draft.error = Some(
                "Sign in to send feedback here, or report it on GitHub.".into(),
            );
            return;
        }

        let mut parts = Vec::new();
        for (index, image) in draft.images.iter().enumerate() {
            parts.push(crate::api::MultipartPart::File {
                name: format!("file{index}"),
                filename: image.name.clone(),
                mime: image.mime.into(),
                data: image.data.clone(),
            });
        }
        if include_logs {
            parts.push(crate::api::MultipartPart::File {
                name: format!("file{}", parts.len()),
                filename: "dragent-diagnostics.log".into(),
                mime: "text/plain".into(),
                data: crate::diagnostics::recent_log().into_bytes(),
            });
        }
        if parts.len() > MAX_FILES {
            draft.error = Some(format!(
                "That's more than {MAX_FILES} files. Remove a few to continue."
            ));
            return;
        }
        let total: usize = parts
            .iter()
            .map(|part| match part {
                crate::api::MultipartPart::File { data, .. } => data.len(),
                crate::api::MultipartPart::Field { .. } => 0,
            })
            .sum();
        if total > MAX_PAYLOAD_BYTES {
            draft.error =
                Some("Attachments add up to more than 8 MB. Remove some to continue.".into());
            return;
        }

        use warpui::SingletonEntity;
        let account = crate::settings::AccountStore::as_ref(ctx)
            .data
            .as_ref()
            .map(|d| d.user.email.clone())
            .unwrap_or_default();
        let organisation = crate::settings::OrgStore::as_ref(ctx)
            .active
            .as_ref()
            .map(|o| format!("{} ({})", o.name, o.id))
            .unwrap_or_default();
        let content = build_content(
            &details,
            &contact,
            &account,
            &organisation,
            env!("CARGO_PKG_VERSION"),
            &platform_name(),
            include_logs,
        );
        parts.insert(
            0,
            crate::api::MultipartPart::Field {
                name: "content".into(),
                value: content,
            },
        );

        draft.submitting = true;
        draft.error = None;
        let (tx, rx) = mpsc::channel();
        crate::bg::detach(move || {
            #[cfg(not(target_family = "wasm"))]
            let result = crate::api::API_CLIENT.post_multipart("/api/v1/feedback", &parts, 60);
            #[cfg(target_family = "wasm")]
            let result: Result<serde_json::Value, crate::api::ApiError> = {
                let _ = parts;
                Err(crate::api::ApiError::Network("unavailable".into()))
            };
            let _ = tx.send(result.map(|_| ()).map_err(|e| e.user_message()));
        });
        ctx.spawn(Timer::after(Duration::from_millis(50)), move |view, _, ctx| {
            view.poll_feedback(rx, ctx)
        });
    }

    fn poll_feedback(&mut self, rx: mpsc::Receiver<Result<(), String>>, ctx: &mut ViewContext<Self>) {
        match rx.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {
                ctx.spawn(Timer::after(Duration::from_millis(50)), move |view, _, ctx| {
                    view.poll_feedback(rx, ctx)
                });
            }
            Err(mpsc::TryRecvError::Disconnected) | Ok(Err(_)) => {
                if let Some(draft) = self.feedback.as_mut() {
                    draft.submitting = false;
                    draft.error =
                        Some("Your feedback didn't go through. Check your connection and retry.".into());
                }
                ctx.notify();
            }
            Ok(Ok(())) => {
                if let Some(draft) = self.feedback.as_mut() {
                    draft.submitting = false;
                    draft.sent = true;
                }
                ctx.spawn(Timer::after(Duration::from_millis(1800)), |view, _, ctx| {
                    if view.feedback.as_ref().is_some_and(|d| d.sent) {
                        view.feedback = None;
                        ctx.notify();
                    }
                });
                ctx.notify();
            }
        }
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

pub(crate) fn render_feedback_modal(
    draft: &FeedbackDraft,
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let mouse = |key: &str| -> MouseStateHandle {
        states.borrow_mut().entry(format!("feedback-{key}")).or_default().clone()
    };
    if draft.sent {
        let panel = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(10.0)
                .with_child(render_icon(AppIcon::Check, theme.ui_green(), 22.0))
                .with_child(
                    Text::new("Thank you!".to_string(), ui, crate::theme::text_size(16.0))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_child(
                    Text::new("Your feedback is on its way to the team.".to_string(), ui, crate::theme::text_size(12.5))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
        .with_uniform_padding(28.0)
        .finish();
        let panel = ConstrainedBox::new(panel).with_width(PANEL_WIDTH).finish();
        return super::auth_modals::modal_scrim_wide(panel, WorkspaceAction::CloseFeedback, view_id);
    }
    let field = |child: Box<dyn Element>, invalid: bool| {
        Container::new(child)
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(if invalid {
                theme.ui_red()
            } else {
                theme.outline()
            }))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .finish()
    };

    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(12.0)
        .with_child(
            Text::new("Share feedback".to_string(), ui, crate::theme::text_size(17.0))
                .with_color(theme.main_text())
                .finish(),
        )
        .with_child(field(
            // The editor keeps its own height, pinned to the top of a taller
            // box: stretched to the box it centres its first line, which put
            // the caret in the middle while the placeholder sat at the top.
            // Clicks in the empty space below still focus it.
            EventHandler::new(
                ConstrainedBox::new(
                    Flex::column()
                        .with_main_axis_size(MainAxisSize::Min)
                        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                        .with_child(ChildView::new(&draft.details_editor).finish())
                        .finish(),
                )
                .with_min_height(110.0)
                .finish(),
            )
            .on_left_mouse_down(move |ectx, _, _| {
                ectx.dispatch_typed_action_for(view_id, WorkspaceAction::FocusFeedbackDetails);
                DispatchEventResult::StopPropagation
            })
            .finish(),
            false,
        ))
        .with_child(field(
            ChildView::new(&draft.contact_editor).finish(),
            draft.contact_error.is_some(),
        ));
    if let Some(error) = &draft.contact_error {
        body = body.with_child(
            Text::new(error.clone(), ui, crate::theme::text_size(11.5))
                .with_color(theme.ui_red())
                .finish(),
        );
    }

    // Optional log attachment.
    let checked = draft.include_logs;
    let theme_owned = *theme;
    let checkbox = Hoverable::new(mouse("logs"), move |_| {
        let bx = Container::new(if checked {
            render_icon(AppIcon::Check, theme_owned.accent_text(), 11.0)
        } else {
            warpui::elements::Empty::new().finish()
        })
        .with_background_color(if checked {
            theme_owned.accent
        } else {
            ColorU::transparent_black()
        })
        .with_border(Border::all(1.0).with_border_color(if checked {
            theme_owned.accent
        } else {
            theme_owned.outline()
        }))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .with_uniform_padding(1.5)
        .finish();
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(ConstrainedBox::new(bx).with_width(15.0).with_height(15.0).finish())
            .with_child(
                Text::new("Attach recent app logs".to_string(), ui, crate::theme::text_size(12.5))
                    .with_color(theme_owned.main_text())
                    .finish(),
            )
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(view_id, WorkspaceAction::ToggleFeedbackDiagnostics)
    })
    .finish();
    body = body.with_child(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(4.0)
            .with_child(checkbox)
            .with_child(
                Container::new(
                    Text::new(
                        "Attaches recent app logs with sensitive details stripped out.".to_string(),
                        ui,
                        crate::theme::text_size(11.0),
                    )
                    .with_color(theme.disabled_text())
                    .finish(),
                )
                .with_padding_left(23.0)
                .finish(),
            )
            .finish(),
    );

    // Public alternative: a GitHub issue.
    let github = Hoverable::new(mouse("github"), move |state| {
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(
                Text::new("Report it on GitHub instead".to_string(), ui, crate::theme::text_size(11.5))
                    .with_color(if state.is_hovered() {
                        theme_owned.main_text()
                    } else {
                        theme_owned.sub_text()
                    })
                    .finish(),
            )
            .with_child(render_icon(
                AppIcon::Asset("icons/external-link.svg"),
                theme_owned.sub_text(),
                11.0,
            ))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(
            view_id,
            WorkspaceAction::OpenExternalUrl {
                url: super::org_menu::REPORT_ISSUE_URL.into(),
            },
        )
    })
    .finish();
    body = body.with_child(github);

    if !draft.images.is_empty() {
        let mut chips = Wrap::row().with_spacing(6.0).with_run_spacing(6.0);
        for (index, image) in draft.images.iter().enumerate() {
            let name = image.name.clone();
            let submitting = draft.submitting;
            chips = chips.with_child(
                Hoverable::new(mouse(&format!("image-{index}")), move |state| {
                    Container::new(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(6.0)
                            .with_child(render_icon(
                                AppIcon::Asset("icons/image.svg"),
                                theme_owned.sub_text(),
                                12.0,
                            ))
                            .with_child(
                                ConstrainedBox::new(
                                    Text::new(name.clone(), ui, crate::theme::text_size(11.5))
                                        .with_color(theme_owned.main_text())
                                        .soft_wrap(false)
                                        .with_clip(ClipConfig::ellipsis())
                                        .finish(),
                                )
                                .with_max_width(150.0)
                                .finish(),
                            )
                            .with_child(render_icon(
                                AppIcon::XClose,
                                if state.is_hovered() && !submitting {
                                    theme_owned.main_text()
                                } else {
                                    theme_owned.disabled_text()
                                },
                                10.0,
                            ))
                            .finish(),
                    )
                    .with_background_color(theme_owned.surface_2())
                    .with_border(Border::all(1.0).with_border_color(theme_owned.outline()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                    .with_padding_left(7.0)
                    .with_padding_right(7.0)
                    .with_padding_top(4.0)
                    .with_padding_bottom(4.0)
                    .finish()
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ectx, _, _| {
                    ectx.dispatch_typed_action_for(
                        view_id,
                        WorkspaceAction::RemoveFeedbackAttachment { index },
                    )
                })
                .finish(),
            );
        }
        body = body.with_child(chips.finish());
    }

    if let Some(error) = &draft.error {
        body = body.with_child(
            Text::new(error.clone(), ui, crate::theme::text_size(12.0))
                .with_color(theme.ui_red())
                .finish(),
        );
    }

    let attach = footer_button(
        theme,
        ui,
        mouse("attach"),
        Some(AppIcon::Asset("icons/paperclip.svg")),
        "Add screenshot",
        false,
        (!draft.submitting).then_some(WorkspaceAction::AttachFeedbackImages),
        view_id,
    );
    let send = footer_button(
        theme,
        ui,
        mouse("send"),
        None,
        if draft.submitting { "Submitting…" } else { "Submit" },
        true,
        (!draft.submitting).then_some(WorkspaceAction::SubmitFeedback),
        view_id,
    );
    body = body.with_child(
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(attach)
                .with_child(send)
                .finish(),
        )
        .with_padding_top(4.0)
        .finish(),
    );

    let panel = Container::new(body.finish())
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
        .with_uniform_padding(20.0)
        .finish();
    let panel = ConstrainedBox::new(panel).with_width(PANEL_WIDTH).finish();
    super::auth_modals::modal_scrim_wide(panel, WorkspaceAction::CloseFeedback, view_id)
}

#[allow(clippy::too_many_arguments)]
fn footer_button(
    theme: &Theme,
    ui: FamilyId,
    mouse: MouseStateHandle,
    icon: Option<AppIcon>,
    label: &'static str,
    primary: bool,
    action: Option<WorkspaceAction>,
    view_id: EntityId,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    let enabled = action.is_some();
    let button = Hoverable::new(mouse, move |state| {
        let hovered = state.is_hovered() && enabled;
        let (bg, fg, border) = if primary {
            (
                if hovered {
                    ColorU::new(
                        theme_owned.accent.r.saturating_add(15),
                        theme_owned.accent.g.saturating_add(15),
                        theme_owned.accent.b.saturating_add(15),
                        theme_owned.accent.a,
                    )
                } else {
                    theme_owned.accent
                },
                theme_owned.accent_text(),
                theme_owned.accent,
            )
        } else {
            (
                if hovered {
                    theme_owned.surface_2()
                } else {
                    ColorU::transparent_black()
                },
                if hovered {
                    theme_owned.main_text()
                } else {
                    theme_owned.sub_text()
                },
                theme_owned.outline(),
            )
        };
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0);
        if let Some(icon) = icon {
            row = row.with_child(render_icon(icon, fg, 13.0));
        }
        row = row.with_child(Text::new(label.to_string(), ui, crate::theme::text_size(12.5)).with_color(fg).finish());
        Container::new(row.finish())
            .with_background_color(bg)
            .with_border(Border::all(1.0).with_border_color(border))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(7.0)
            .with_padding_bottom(7.0)
            .finish()
    });
    match action {
        Some(action) => button
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, action.clone()))
            .finish(),
        None => button.finish(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_carries_the_context_we_append() {
        let content = build_content(
            "  The diff view is great.  ",
            " me@example.com ",
            "account@example.com",
            "Acme (org-1)",
            "1.2.3",
            "macos aarch64",
            true,
        );
        assert_eq!(
            content,
            "The diff view is great.\n\nReply-to: me@example.com\nAccount: account@example.com\nOrganisation: Acme (org-1)\nApp version: 1.2.3\nOS: macos aarch64\nLogs: included (redacted)"
        );
        let minimal = build_content("x", "", "", "", "1", "linux x86_64", false);
        assert_eq!(minimal, "x\n\nApp version: 1\nOS: linux x86_64");
    }

    #[test]
    fn contact_is_optional_but_validated() {
        assert_eq!(contact_error(""), None);
        assert_eq!(contact_error("  "), None);
        assert_eq!(contact_error("a@b.co"), None);
        for bad in ["a", "a@", "@b.co", "a@b", "a b@c.co"] {
            assert!(contact_error(bad).is_some(), "{bad}");
        }
    }
}
