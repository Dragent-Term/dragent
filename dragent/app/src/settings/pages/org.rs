//! Organisation settings: members & invitations, teams, billing & usage (live,
//! the same data and actions as the web dashboard's Organisation section).

use warpui::color::ColorU;
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Flex, MainAxisAlignment,
    MainAxisSize, ParentElement, Shrinkable, Text, Wrap,
};
use warpui::fonts::FamilyId;
use warpui::{AppContext, SingletonEntity};
use warpui_core::elements::WrapFill;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::auth::AuthManager;
use crate::shared_input::{shared_input, SharedInputOpts, SharedInputWidth};
use crate::theme::Theme;

use super::super::account_store::{mc_to_credits, AccountData, AccountStore};
use super::super::action::{SettingsAction, SettingsSection};
use super::super::org_store::{Invitation, Member, OrgRole, OrgStore, Plan, Team};
use super::super::settings_view::SettingsView;
use super::super::widgets::{
    accent_outline_button, alert_banner, card_rows, circular_avatar, danger_button, empty_state,
    group_label, link_button, page_header, primary_button, secondary_button, segmented_control,
    setting_row, toggle_row, usage_percent_row, with_alpha, SegmentOption,
};
use super::account::format_unix_datetime_local;

// ── Pages ─────────────────────────────────────────────────────────────────────

pub fn render_members(view: &SettingsView, theme: &Theme, app: &AppContext) -> Box<dyn Element> {
    org_page(
        view,
        theme,
        app,
        "Members",
        "Invite teammates, assign roles, and manage who can access this organisation.",
        |view, theme, store, account| members_body(view, theme, store, account),
    )
}

pub fn render_teams(view: &SettingsView, theme: &Theme, app: &AppContext) -> Box<dyn Element> {
    org_page(
        view,
        theme,
        app,
        "Teams",
        "Group the organisation's members into teams.",
        |view, theme, store, _| teams_body(view, theme, store),
    )
}

pub fn render_billing(view: &SettingsView, theme: &Theme, app: &AppContext) -> Box<dyn Element> {
    org_page(
        view,
        theme,
        app,
        "Billing & Usage",
        "The organisation's plan, your usage this cycle, and credits.",
        |view, theme, store, account| billing_body(view, theme, store, account),
    )
}

/// Shared frame: sign-in and loading states, the active-organisation card
/// with its switcher, invitations addressed to the user, and messages.
fn org_page(
    view: &SettingsView,
    theme: &Theme,
    app: &AppContext,
    title: &str,
    caption: &str,
    body: impl FnOnce(&SettingsView, &Theme, &OrgStore, Option<&AccountData>) -> Vec<Box<dyn Element>>,
) -> Box<dyn Element> {
    let ui = view.ui_font;
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(4.0)
        .with_child(page_header(theme, ui, title, caption));

    if crate::api::is_local_mode() {
        return col
            .with_child(super::super::widgets::needs_account_state(theme, ui))
            .finish();
    }

    if !AuthManager::as_ref(app).is_logged_in() {
        return col
            .with_child(empty_state(
                theme,
                ui,
                "Sign in to manage your organisation",
                "Organisations, teammates, and billing belong to your Dragent account.",
            ))
            .with_child(
                Container::new(primary_button(
                    theme,
                    ui,
                    view.view_id,
                    view.hover("org_signin"),
                    "Sign in with browser",
                    SettingsAction::SignIn,
                ))
                .with_padding_top(12.0)
                .finish(),
            )
            .finish();
    }

    let store = OrgStore::as_ref(app);
    let account = AccountStore::as_ref(app).data.as_ref();

    if store.organizations.is_none() {
        col = col.with_children(messages(view, theme, store));
        let (headline, detail) = if store.loading || store.error.is_none() {
            ("Loading your organisation…", "")
        } else {
            ("Couldn't load your organisation", "Check your connection and try again.")
        };
        col = col.with_child(empty_state(theme, ui, headline, detail));
        if !store.loading {
            col = col.with_child(
                Container::new(secondary_button(
                    theme,
                    ui,
                    view.view_id,
                    view.hover("org_retry"),
                    "Try again",
                    SettingsAction::OrgRefresh,
                ))
                .with_padding_top(12.0)
                .finish(),
            );
        }
        return col.finish();
    }

    col = col
        .with_child(org_context(view, theme, store))
        .with_children(my_invitations(view, theme, store))
        .with_children(messages(view, theme, store));

    if store.active.is_none() {
        return col
            .with_child(empty_state(
                theme,
                ui,
                "You're not in an organisation",
                "Create one above, or accept an invitation from a teammate.",
            ))
            .finish();
    }

    col.with_children(body(view, theme, store, account)).finish()
}

// ── Active organisation, switcher, create ────────────────────────────────────

fn org_context(view: &SettingsView, theme: &Theme, store: &OrgStore) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let role = store.active_role();

    let (name, subtitle) = match &store.active {
        Some(org) => {
            let plan = store
                .billing
                .as_ref()
                .map(|b| format!("{} plan", capitalize(if b.tier.is_empty() { "free" } else { &b.tier })))
                .unwrap_or_else(|| "Loading plan…".into());
            (org.name.clone(), format!("{}  \u{00B7}  {plan}", role.label()))
        }
        None => ("No organisation".to_string(), "Create or join one to collaborate.".to_string()),
    };

    let identity = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(14.0)
        .with_child(circular_avatar(theme, ui, &name))
        .with_child(
            Shrinkable::new(
                1.0,
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_spacing(3.0)
                    .with_child(label(ui, &name, 15.5, theme.main_text()))
                    .with_child(label(ui, &subtitle, 12.5, theme.sub_text()))
                    .finish(),
            )
            .finish(),
        )
        .finish();

    let actions = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(secondary_button(
            theme,
            ui,
            id,
            view.hover("org_refresh"),
            if store.loading { "Refreshing…" } else { "Refresh" },
            SettingsAction::OrgRefresh,
        ))
        .with_child(secondary_button(
            theme,
            ui,
            id,
            view.hover("org_switcher"),
            if view.org.switcher_open { "Close" } else { "Switch organisation" },
            SettingsAction::OrgToggleSwitcher,
        ))
        .finish();

    let head = Container::new(
        Wrap::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(16.0)
            .with_run_spacing(12.0)
            .with_child(WrapFill::new(220.0, identity).finish())
            .with_child(actions)
            .finish(),
    )
    .with_uniform_padding(16.0)
    .finish();

    if !view.org.switcher_open && store.active.is_some() {
        return card_rows(theme, vec![head]);
    }

    let mut rows = vec![head];
    for org in store.organizations.iter().flatten() {
        let current = store.active.as_ref().is_some_and(|a| a.id == org.id);
        let trailing = if current {
            pill(ui, "Current", theme.accent)
        } else if store.is_busy("switch") {
            label(ui, "Switching…", 12.5, theme.sub_text())
        } else {
            secondary_button(
                theme,
                ui,
                id,
                view.hover(&format!("org_switch_{}", org.id)),
                "Switch",
                SettingsAction::OrgSwitch(org.id.clone()),
            )
        };
        rows.push(list_row(
            two_line(theme, ui, &org.name, OrgRole::parse(&org.role).label()),
            trailing,
        ));
    }
    rows.push(create_org_row(view, theme, store));
    card_rows(theme, rows)
}

fn create_org_row(view: &SettingsView, theme: &Theme, store: &OrgStore) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let editor = view.org.editors.as_ref().map(|e| &e.new_org_name);
    let content: Box<dyn Element> = match editor {
        Some(editor) if view.org.create_open || store.active.is_none() => Wrap::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_run_spacing(8.0)
            .with_child(WrapFill::new(220.0, input(theme, editor)).finish())
            .with_child(primary_button(
                theme,
                ui,
                id,
                view.hover("org_create"),
                if store.is_busy("org_create") { "Creating…" } else { "Create organisation" },
                SettingsAction::OrgCreate,
            ))
            .with_child(secondary_button(
                theme,
                ui,
                id,
                view.hover("org_create_cancel"),
                "Cancel",
                SettingsAction::OrgToggleCreate,
            ))
            .finish(),
        _ => accent_outline_button(
            theme,
            ui,
            id,
            view.hover("org_create_open"),
            "+ New organisation",
            SettingsAction::OrgToggleCreate,
        ),
    };
    Container::new(content)
        .with_padding_left(16.0)
        .with_padding_right(16.0)
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)
        .finish()
}

/// Invitations addressed to the signed-in user, from any organisation.
fn my_invitations(view: &SettingsView, theme: &Theme, store: &OrgStore) -> Vec<Box<dyn Element>> {
    let ui = view.ui_font;
    let id = view.view_id;
    store
        .my_invitations
        .iter()
        .filter(|inv| inv.is_open())
        .map(|inv| {
            let key = format!("respond:{}", inv.id);
            let buttons: Box<dyn Element> = if store.is_busy(&key) {
                label(ui, "Working…", 12.5, theme.sub_text())
            } else {
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_spacing(8.0)
                    .with_child(primary_button(
                        theme,
                        ui,
                        id,
                        view.hover(&format!("inv_accept_{}", inv.id)),
                        "Accept",
                        SettingsAction::OrgRespondInvitation { id: inv.id.clone(), accept: true },
                    ))
                    .with_child(secondary_button(
                        theme,
                        ui,
                        id,
                        view.hover(&format!("inv_reject_{}", inv.id)),
                        "Decline",
                        SettingsAction::OrgRespondInvitation { id: inv.id.clone(), accept: false },
                    ))
                    .finish()
            };
            let text = format!(
                "You've been invited to join an organisation as {}.",
                OrgRole::parse(&inv.role).label()
            );
            Container::new(tinted(
                theme.accent,
                list_row(label(ui, &text, 13.0, theme.main_text()), buttons),
            ))
            .with_padding_top(10.0)
            .finish()
        })
        .collect()
}

fn messages(view: &SettingsView, theme: &Theme, store: &OrgStore) -> Vec<Box<dyn Element>> {
    let ui = view.ui_font;
    let mut out = Vec::new();
    let dismiss = || {
        link_button(
            theme,
            ui,
            view.view_id,
            view.hover("org_dismiss"),
            "Dismiss",
            SettingsAction::OrgDismissMessage,
        )
    };
    if let Some(error) = &store.error {
        out.push(
            Container::new(tinted(
                theme.ui_red(),
                list_row(label(ui, error, 12.5, theme.main_text()), dismiss()),
            ))
            .with_padding_top(10.0)
            .finish(),
        );
    } else if let Some(notice) = &store.notice {
        out.push(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_child(alert_banner(theme, ui, notice))
                    .finish(),
            )
            .with_padding_top(10.0)
            .finish(),
        );
    }
    out
}

// ── Members & invitations ─────────────────────────────────────────────────────

fn members_body(
    view: &SettingsView,
    theme: &Theme,
    store: &OrgStore,
    account: Option<&AccountData>,
) -> Vec<Box<dyn Element>> {
    let ui = view.ui_font;
    let id = view.view_id;
    let role = store.active_role();
    let my_email = account.map(|a| a.user.email.to_lowercase()).unwrap_or_default();
    let members = store.members.as_deref().unwrap_or_default();
    let mut out = Vec::new();

    out.push(group_label(theme, ui, &format!("Members  \u{00B7}  {}", members.len())));
    if members.is_empty() {
        out.push(muted(ui, theme, if store.loading { "Loading members…" } else { "No members found." }));
    } else {
        let rows = members
            .iter()
            .map(|m| member_row(view, theme, store, m, role, !my_email.is_empty() && m.email.to_lowercase() == my_email))
            .collect();
        out.push(card_rows(theme, rows));
    }

    if role != OrgRole::Owner {
        let armed = view.org.confirm.as_deref() == Some("leave");
        out.push(
            Container::new(card_rows(
                theme,
                vec![setting_row(
                    theme,
                    ui,
                    "Leave organisation",
                    Some(if armed {
                        "Click again to confirm. You'll lose access until someone invites you back."
                    } else {
                        "You'll lose access to its members, teams, and shared credits."
                    }),
                    if store.is_busy("leave") {
                        label(ui, "Leaving…", 12.5, theme.sub_text())
                    } else {
                        danger_button(
                            theme,
                            ui,
                            id,
                            view.hover("org_leave"),
                            if armed { "Confirm leave" } else { "Leave" },
                            SettingsAction::OrgLeave,
                        )
                    },
                )],
            ))
            .with_padding_top(12.0)
            .finish(),
        );
    }

    out.push(group_label(theme, ui, "Invitations"));
    if !store.is_paid() {
        out.push(upgrade_gate(view, theme, "invite teammates", role == OrgRole::Owner));
        return out;
    }
    if !role.can_manage() {
        out.push(muted(
            ui,
            theme,
            "Only an admin or owner can invite members or view sent invitations.",
        ));
        return out;
    }

    let mut invite_rows = Vec::new();
    if let Some(editors) = &view.org.editors {
        let form = Wrap::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_run_spacing(8.0)
            .with_child(WrapFill::new(240.0, input(theme, &editors.invite_email)).finish())
            .with_child(role_picker(view, theme, "invite_role", &view.org.invite_role, |r| {
                SettingsAction::OrgSetInviteRole(r.to_string())
            }))
            .with_child(primary_button(
                theme,
                ui,
                id,
                view.hover("org_invite"),
                if store.is_busy("invite") { "Sending…" } else { "Send invite" },
                SettingsAction::OrgInvite,
            ))
            .finish();
        invite_rows.push(
            Container::new(form)
                .with_uniform_padding(14.0)
                .finish(),
        );
    }
    let sent = store.invitations.as_deref().unwrap_or_default();
    if sent.is_empty() {
        invite_rows.push(
            Container::new(label(ui, "No invitations sent yet.", 12.5, theme.sub_text()))
                .with_uniform_padding(16.0)
                .finish(),
        );
    }
    for inv in sent {
        invite_rows.push(invitation_row(view, theme, store, inv));
    }
    out.push(card_rows(theme, invite_rows));
    out
}

fn member_row(
    view: &SettingsView,
    theme: &Theme,
    store: &OrgStore,
    member: &Member,
    my_role: OrgRole,
    is_me: bool,
) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let their_role = OrgRole::parse(&member.role);
    let title = if is_me { format!("{} (you)", member.email) } else { member.email.clone() };
    let joined = if member.created_at > 0 {
        format!("Joined {}", format_date(member.created_at))
    } else {
        String::new()
    };
    let identity = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(12.0)
        .with_child(circular_avatar(theme, ui, &member.email))
        .with_child(Shrinkable::new(1.0, two_line(theme, ui, &title, &joined)).finish())
        .finish();

    let busy = store.is_busy(&format!("role:{}", member.user_id))
        || store.is_busy(&format!("remove:{}", member.user_id));
    let trailing: Box<dyn Element> = if their_role == OrgRole::Owner {
        pill(ui, "Owner", theme.accent)
    } else if busy {
        label(ui, "Saving…", 12.5, theme.sub_text())
    } else if my_role.can_manage() && !is_me {
        let uid = member.user_id.clone();
        let armed = view.org.confirm.as_deref() == Some(format!("remove:{uid}").as_str());
        Flex::row()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(role_picker(view, theme, &format!("member_{uid}"), &member.role, move |r| {
                SettingsAction::OrgSetMemberRole { user_id: uid.clone(), role: r.to_string() }
            }))
            .with_child(danger_button(
                theme,
                ui,
                id,
                view.hover(&format!("org_remove_{}", member.user_id)),
                if armed { "Confirm remove" } else { "Remove" },
                SettingsAction::OrgRemoveMember(member.user_id.clone()),
            ))
            .finish()
    } else {
        pill(ui, their_role.label(), theme.sub_text())
    };
    list_row(identity, trailing)
}

fn invitation_row(view: &SettingsView, theme: &Theme, store: &OrgStore, inv: &Invitation) -> Box<dyn Element> {
    let ui = view.ui_font;
    let mut detail = format!(
        "{}  \u{00B7}  {}",
        OrgRole::parse(&inv.role).label(),
        capitalize(inv.display_status())
    );
    if inv.is_open() && inv.expires_at > 0 {
        detail.push_str(&format!("  \u{00B7}  expires {}", format_date(inv.expires_at)));
    }
    let key = format!("cancel:{}", inv.id);
    let trailing = if store.is_busy(&key) {
        label(ui, "Cancelling…", 12.5, theme.sub_text())
    } else if inv.is_open() {
        secondary_button(
            theme,
            ui,
            view.view_id,
            view.hover(&format!("inv_cancel_{}", inv.id)),
            "Cancel",
            SettingsAction::OrgCancelInvitation(inv.id.clone()),
        )
    } else {
        pill(ui, &capitalize(inv.display_status()), status_color(theme, inv.display_status()))
    };
    list_row(two_line(theme, ui, &inv.email, &detail), trailing)
}

/// Member / Admin choice (owners are made by ownership transfer, not here).
fn role_picker(
    view: &SettingsView,
    theme: &Theme,
    key: &str,
    current: &str,
    action: impl Fn(&str) -> SettingsAction,
) -> Box<dyn Element> {
    let options = ["member", "admin"]
        .into_iter()
        .map(|role| SegmentOption {
            label: OrgRole::parse(role).label().to_string(),
            selected: current == role,
            hover: view.hover(&format!("role_{key}_{role}")),
            action: action(role),
        })
        .collect();
    segmented_control(theme, view.ui_font, view.view_id, options)
}

// ── Teams ─────────────────────────────────────────────────────────────────────

fn teams_body(view: &SettingsView, theme: &Theme, store: &OrgStore) -> Vec<Box<dyn Element>> {
    let ui = view.ui_font;
    let id = view.view_id;
    let role = store.active_role();
    let mut out = Vec::new();

    if !store.is_paid() {
        out.push(group_label(theme, ui, "Teams"));
        out.push(upgrade_gate(view, theme, "create teams", role == OrgRole::Owner));
        return out;
    }

    let teams: &[Team] = store.teams.as_deref().unwrap_or_default();
    if role.can_manage() {
        if let Some(editors) = &view.org.editors {
            out.push(group_label(theme, ui, "New team"));
            let form = Wrap::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_run_spacing(8.0)
                .with_child(WrapFill::new(240.0, input(theme, &editors.team_name)).finish())
                .with_child(primary_button(
                    theme,
                    ui,
                    id,
                    view.hover("team_create"),
                    if store.is_busy("team_create") { "Creating…" } else { "Create team" },
                    SettingsAction::TeamCreate,
                ))
                .finish();
            out.push(card_rows(
                theme,
                vec![Container::new(form).with_uniform_padding(14.0).finish()],
            ));
        }
    }

    out.push(group_label(theme, ui, &format!("Teams  \u{00B7}  {}", teams.len())));
    if teams.is_empty() {
        out.push(empty_state(
            theme,
            ui,
            "No teams yet",
            if role.can_manage() {
                "Create a team to group members of this organisation."
            } else {
                "An admin or owner can create teams for this organisation."
            },
        ));
        return out;
    }
    let rows = teams
        .iter()
        .map(|team| {
            let key = format!("team:{}", team.id);
            let trailing = if store.is_busy(&key) {
                label(ui, "Deleting…", 12.5, theme.sub_text())
            } else if role.can_manage() {
                let armed = view.org.confirm.as_deref() == Some(key.as_str());
                danger_button(
                    theme,
                    ui,
                    id,
                    view.hover(&format!("team_delete_{}", team.id)),
                    if armed { "Confirm delete" } else { "Delete" },
                    SettingsAction::TeamDelete(team.id.clone()),
                )
            } else {
                warpui::elements::Empty::new().finish()
            };
            list_row(two_line(theme, ui, &team.name, &team.slug), trailing)
        })
        .collect();
    out.push(card_rows(theme, rows));
    out
}

// ── Billing & usage ───────────────────────────────────────────────────────────

fn billing_body(
    view: &SettingsView,
    theme: &Theme,
    store: &OrgStore,
    account: Option<&AccountData>,
) -> Vec<Box<dyn Element>> {
    let ui = view.ui_font;
    let mut out = vec![group_label(theme, ui, "Plan")];
    out.extend(plan_section(view, theme, store));
    out.push(group_label(theme, ui, "Your usage"));
    out.push(usage_card(view, theme, account));
    out.push(group_label(theme, ui, "Credits"));
    out.extend(credits_section(view, theme, store, account));
    out
}

fn plan_section(view: &SettingsView, theme: &Theme, store: &OrgStore) -> Vec<Box<dyn Element>> {
    let ui = view.ui_font;
    let id = view.view_id;
    let is_owner = store.active_role() == OrgRole::Owner;
    let Some(billing) = &store.billing else {
        return vec![muted(ui, theme, "Loading plan…")];
    };
    if !billing.enabled {
        return vec![muted(
            ui,
            theme,
            "Organisation billing isn't available on this deployment right now.",
        )];
    }

    if billing.is_paid {
        let mut details = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(4.0)
            .with_child(label(ui, "Current plan", 12.0, theme.sub_text()))
            .with_child(label(ui, &capitalize(&billing.tier), 22.0, theme.main_text()));
        if let Some(renews) = billing.subscription_renews_at.as_deref().and_then(format_rfc3339) {
            details = details.with_child(label(ui, &format!("Renews {renews}"), 12.0, theme.sub_text()));
        }
        let status = if billing.subscription_status.is_empty() {
            "active"
        } else {
            billing.subscription_status.as_str()
        };
        let mut rows = vec![list_row(
            details.finish(),
            pill(ui, &capitalize(status), status_color(theme, status)),
        )];
        rows.push(
            Container::new(label(
                ui,
                "Every member gets their own AI-credit allowance at this tier automatically as they join or leave.",
                12.0,
                theme.sub_text(),
            ))
            .with_padding_left(16.0)
            .with_padding_right(16.0)
            .with_padding_top(12.0)
            .with_padding_bottom(12.0)
            .finish(),
        );
        if is_owner {
            rows.push(setting_row(
                theme,
                ui,
                "Subscription",
                Some("Update the payment method, download invoices, or cancel."),
                accent_outline_button(
                    theme,
                    ui,
                    id,
                    view.hover("billing_portal"),
                    if store.is_busy("portal") { "Opening…" } else { "Manage subscription" },
                    SettingsAction::BillingOpenPortal,
                ),
            ));
        }
        return vec![card_rows(theme, rows)];
    }

    if !is_owner {
        return vec![muted(
            ui,
            theme,
            "This organisation is on the Free plan. Ask an owner to upgrade — only the owner can start a subscription.",
        )];
    }

    let purchasable: Vec<&Plan> = store.plans.iter().filter(|p| p.purchasable).collect();
    if purchasable.is_empty() {
        return vec![muted(ui, theme, "No plans are available to purchase right now.")];
    }
    let monthly_for = |tier: &str| {
        purchasable
            .iter()
            .find(|p| p.tier == tier && !p.is_annual())
            .copied()
    };
    let best_saving = purchasable
        .iter()
        .filter(|p| p.is_annual())
        .filter_map(|p| monthly_for(&p.tier).map(|m| p.annual_saving_percent(m)))
        .max()
        .unwrap_or(0);
    let annual_label = if best_saving > 0 {
        format!("Annual  \u{00B7}  save {best_saving}%")
    } else {
        "Annual".to_string()
    };
    let interval = segmented_control(
        theme,
        ui,
        id,
        vec![
            SegmentOption {
                label: "Monthly".into(),
                selected: !view.org.show_annual,
                hover: view.hover("billing_monthly"),
                action: SettingsAction::BillingShowAnnual(false),
            },
            SegmentOption {
                label: annual_label,
                selected: view.org.show_annual,
                hover: view.hover("billing_annual"),
                action: SettingsAction::BillingShowAnnual(true),
            },
        ],
    );
    let intro = Wrap::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(16.0)
        .with_run_spacing(10.0)
        .with_child(
            WrapFill::new(
                240.0,
                label(
                    ui,
                    "Upgrade to invite teammates and create teams. Every current and future member becomes a billed seat at this tier.",
                    12.5,
                    theme.sub_text(),
                ),
            )
            .finish(),
        )
        .with_child(interval)
        .finish();

    let mut grid = Wrap::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(12.0)
        .with_run_spacing(12.0);
    let shown = purchasable.iter().filter(|p| p.is_annual() == view.org.show_annual);
    for plan in shown {
        let busy = store.is_busy(&format!("subscribe:{}", plan.id));
        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(6.0)
            .with_child(label(ui, &capitalize(&plan.tier), 14.0, theme.main_text()))
            .with_child(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_cross_axis_alignment(CrossAxisAlignment::End)
                    .with_spacing(4.0)
                    .with_child(label(ui, &usd(plan.monthly_cents()), 24.0, theme.main_text()))
                    .with_child(label(ui, "/ seat / mo", 12.0, theme.sub_text()))
                    .finish(),
            );
        let fine_print = if plan.is_annual() {
            let saving = monthly_for(&plan.tier).map(|m| plan.annual_saving_percent(m)).unwrap_or(0);
            if saving > 0 {
                format!("{} billed yearly  \u{00B7}  save {saving}%", usd(plan.price_cents))
            } else {
                format!("{} billed yearly", usd(plan.price_cents))
            }
        } else {
            "Billed monthly".to_string()
        };
        body = body
            .with_child(label(ui, &fine_print, 11.5, theme.sub_text()))
            .with_child(
                Container::new(primary_button(
                    theme,
                    ui,
                    id,
                    view.hover(&format!("subscribe_{}", plan.id)),
                    if busy { "Opening checkout…" } else { "Subscribe" },
                    SettingsAction::BillingSubscribe(plan.id.clone()),
                ))
                .with_padding_top(8.0)
                .finish(),
            );
        grid = grid.with_child(tile(theme, body.finish()));
    }

    vec![intro_container(intro), grid.finish()]
}

fn usage_card(view: &SettingsView, theme: &Theme, account: Option<&AccountData>) -> Box<dyn Element> {
    let ui = view.ui_font;
    let Some(data) = account else {
        return muted(ui, theme, "Loading usage…");
    };
    let meter = |name: &str, resets_at: i64| {
        if resets_at > 0 {
            format!("{name}  \u{00B7}  resets {}", format_unix_datetime_local(resets_at))
        } else {
            name.to_string()
        }
    };
    let mut rows = Vec::new();
    if data.usage.paid.is_paid || data.user.tier != "free" {
        let p = &data.usage.paid;
        rows.push(usage_percent_row(
            theme,
            ui,
            &meter("Plan allowance", p.resets_at),
            p.used_mc,
            p.cap_mc.max(1),
            theme.accent,
        ));
    }
    let f = &data.usage.free;
    rows.push(usage_percent_row(
        theme,
        ui,
        &meter("Free allowance", f.resets_at),
        f.used_mc,
        f.cap_mc.max(1),
        theme.accent,
    ));
    rows.push(
        Container::new(label(
            ui,
            "Your plan allowance is spent first, then the free allowance, then credits.",
            12.0,
            theme.sub_text(),
        ))
        .with_padding_left(16.0)
        .with_padding_right(16.0)
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)
        .finish(),
    );
    card_rows(theme, rows)
}

fn credits_section(
    view: &SettingsView,
    theme: &Theme,
    store: &OrgStore,
    account: Option<&AccountData>,
) -> Vec<Box<dyn Element>> {
    let ui = view.ui_font;
    let id = view.view_id;
    let mut out = Vec::new();

    if let Some(data) = account {
        let balance = mc_to_credits(data.usage.deposit_balance_mc);
        let org_name = store.active.as_ref().map(|a| a.name.as_str()).unwrap_or("this organisation");
        let balance_note =
            format!("Credits belong to {org_name}, never expire, and are separate from your plan.");
        out.push(card_rows(
            theme,
            vec![
                setting_row(
                    theme,
                    ui,
                    "Credit balance",
                    Some(balance_note.as_str()),
                    label(
                        ui,
                        &format!("{} credits", group_thousands(balance as i64)),
                        13.0,
                        if balance > 0 { theme.main_text() } else { theme.sub_text() },
                    ),
                ),
                toggle_row(
                    theme,
                    ui,
                    id,
                    view.hover("credit_usage"),
                    "Use credits when limits are reached",
                    Some("Turns on automatically when you buy credits. When off, requests stop once your allowance is used up."),
                    data.usage.allow_credit_usage,
                    SettingsAction::ToggleCreditUsage,
                ),
            ],
        ));
    }

    let packs: Vec<_> = store.packs.iter().filter(|p| p.purchasable).collect();
    if packs.is_empty() {
        out.push(
            Container::new(muted(ui, theme, "Credit packs aren't available right now."))
                .with_padding_top(12.0)
                .finish(),
        );
        return out;
    }
    out.push(intro_container(label(
        ui,
        "Buy credits — 1 credit = 1¢ of model usage. Larger packs include bonus credits. Checkout opens in your browser; your balance updates here when the payment goes through.",
        12.5,
        theme.sub_text(),
    )));
    let mut grid = Wrap::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(12.0)
        .with_run_spacing(12.0);
    for pack in packs {
        let busy = store.is_busy(&format!("buy:{}", pack.id));
        let bonus = pack.bonus_percent();
        let mut top = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(label(ui, &usd(pack.price_cents), 22.0, theme.main_text()));
        if bonus > 0 {
            top = top.with_child(pill(ui, &format!("+{bonus}% bonus"), theme.ui_green()));
        }
        let body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(6.0)
            .with_child(top.finish())
            .with_child(label(
                ui,
                &format!("{} credits", group_thousands(pack.credits() as i64)),
                13.0,
                theme.sub_text(),
            ))
            .with_child(
                Container::new(primary_button(
                    theme,
                    ui,
                    id,
                    view.hover(&format!("buy_{}", pack.id)),
                    if busy { "Opening checkout…" } else { "Buy credits" },
                    SettingsAction::BuyCredits(pack.id.clone()),
                ))
                .with_padding_top(8.0)
                .finish(),
            )
            .finish();
        grid = grid.with_child(tile(theme, body));
    }
    out.push(grid.finish());
    out
}

/// Shown instead of a collaboration form on a Free organisation, which the
/// server refuses (402) — the constraint is stated up front.
fn upgrade_gate(view: &SettingsView, theme: &Theme, what: &str, is_owner: bool) -> Box<dyn Element> {
    let ui = view.ui_font;
    let text = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(4.0)
        .with_child(label(
            ui,
            "This organisation is on the Free plan, which is local-only.",
            13.0,
            theme.main_text(),
        ))
        .with_child(label(
            ui,
            &if is_owner {
                format!("Upgrade to a paid plan to {what}.")
            } else {
                format!("Ask an owner to upgrade the plan to {what}.")
            },
            12.5,
            theme.sub_text(),
        ))
        .finish();
    let trailing = if is_owner {
        primary_button(
            theme,
            ui,
            view.view_id,
            view.hover(&format!("upgrade_{what}")),
            "See plans",
            SettingsAction::SelectSection(SettingsSection::Billing),
        )
    } else {
        warpui::elements::Empty::new().finish()
    };
    tinted(theme.accent, list_row(text, trailing))
}

// ── Building blocks ───────────────────────────────────────────────────────────

fn label(ui: FamilyId, text: &str, size: f32, color: ColorU) -> Box<dyn Element> {
    Text::new(text.to_string(), ui, crate::theme::text_size(size))
        .with_color(color)
        .finish()
}

fn muted(ui: FamilyId, theme: &Theme, text: &str) -> Box<dyn Element> {
    Container::new(label(ui, text, 13.0, theme.sub_text()))
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .finish()
}

fn two_line(theme: &Theme, ui: FamilyId, title: &str, detail: &str) -> Box<dyn Element> {
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(3.0)
        .with_child(label(ui, title, 13.5, theme.main_text()));
    if !detail.is_empty() {
        col = col.with_child(label(ui, detail, 12.0, theme.sub_text()));
    }
    col.finish()
}

/// Content on the left, controls on the right; controls drop below when narrow.
fn list_row(lead: Box<dyn Element>, trailing: Box<dyn Element>) -> Box<dyn Element> {
    Container::new(
        Wrap::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(16.0)
            .with_run_spacing(10.0)
            .with_child(WrapFill::new(220.0, lead).finish())
            .with_child(trailing)
            .finish(),
    )
    .with_padding_left(16.0)
    .with_padding_right(16.0)
    .with_padding_top(12.0)
    .with_padding_bottom(12.0)
    .finish()
}

fn pill(ui: FamilyId, text: &str, color: ColorU) -> Box<dyn Element> {
    Container::new(label(ui, text, 11.5, color))
        .with_background_color(with_alpha(color, 0.12))
        .with_border(Border::all(1.0).with_border_color(with_alpha(color, 0.3)))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_padding_left(9.0)
        .with_padding_right(9.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .finish()
}

/// A banner tinted with `color` (accent for invitations and gates, red for errors).
fn tinted(color: ColorU, child: Box<dyn Element>) -> Box<dyn Element> {
    Container::new(child)
        .with_background_color(with_alpha(color, 0.08))
        .with_border(Border::all(1.0).with_border_color(with_alpha(color, 0.3)))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .finish()
}

/// A fixed-width card in a plan or credit-pack grid.
fn tile(theme: &Theme, body: Box<dyn Element>) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(body)
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
            .with_uniform_padding(16.0)
            .finish(),
    )
    .with_width(236.0)
    .finish()
}

fn intro_container(child: Box<dyn Element>) -> Box<dyn Element> {
    Container::new(child)
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)
        .finish()
}

fn input(theme: &Theme, editor: &warpui::ViewHandle<crate::workspace::InputEditor>) -> Box<dyn Element> {
    shared_input(
        theme,
        editor,
        SharedInputOpts {
            width: SharedInputWidth::Fill,
            radius: 8.0,
            pad_left: 10.0,
            pad_right: 10.0,
            pad_top: 8.0,
            pad_bottom: 8.0,
            ..Default::default()
        },
    )
}

fn status_color(theme: &Theme, status: &str) -> ColorU {
    match status {
        "active" | "accepted" | "on_trial" => theme.ui_green(),
        "past_due" | "unpaid" | "paused" | "pending" => theme.ui_yellow(),
        "cancelled" | "canceled" | "expired" | "rejected" => theme.ui_red(),
        _ => theme.sub_text(),
    }
}

fn capitalize(s: &str) -> String {
    let s = s.replace('_', " ");
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn group_thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// "$20" for whole dollars, "$16.58" otherwise.
fn usd(cents: i64) -> String {
    let dollars = group_thousands(cents / 100);
    match cents % 100 {
        0 => format!("${dollars}"),
        rest => format!("${dollars}.{:02}", rest.abs()),
    }
}

fn format_date(unix_secs: i64) -> String {
    chrono::DateTime::from_timestamp(unix_secs, 0)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%b %-d, %Y").to_string())
        .unwrap_or_default()
}

fn format_rfc3339(s: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Local).format("%b %-d, %Y").to_string())
}

#[cfg(test)]
mod tests {
    use super::{capitalize, group_thousands, usd};

    #[test]
    fn money_and_numbers() {
        assert_eq!(usd(2_000), "$20");
        assert_eq!(usd(1_658), "$16.58");
        assert_eq!(usd(123_456_700), "$1,234,567");
        assert_eq!(group_thousands(5_750), "5,750");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(-1_000), "-1,000");
        assert_eq!(capitalize("past_due"), "Past due");
        assert_eq!(capitalize(""), "");
    }
}
