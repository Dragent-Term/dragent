//! Settings → Machines: the page header, a search + Add machine toolbar, and
//! machines grouped by whether they are connected right now.
use super::super::machines_store::{endpoint_label, MachinesStore};
use super::super::widgets::page_header;
use super::{MachineAction, SettingsView};
use crate::icons::{render_icon, AppIcon};
use crate::machine_ui::{self as mui, Size, Variant};
use crate::settings::SettingsAction;
use crate::theme::Theme;
use dragent_host::machine_registry::Machine;
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Expanded, Flex, ParentElement,
};
use warpui_core::elements::Hoverable;
use warpui_core::platform::Cursor;

impl SettingsView {
    pub(super) fn render_machine_list(&self, theme: &Theme, store: &MachinesStore) -> Box<dyn Element> {
        let ui = self.ui_font;
        let mut page = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(page_header(
                theme,
                ui,
                "Machines",
                "Connect servers over SSH to open projects and run terminals and agents on them.",
            ));
        if let Some(error) = store.load_error.as_ref().or(self.machines.notice.as_ref()) {
            page = page.with_child(
                Container::new(mui::notice(
                    theme,
                    ui,
                    mui::Tone::Error,
                    "Something went wrong",
                    Some(error.clone()),
                    None,
                ))
                .with_padding_bottom(20.0)
                .finish(),
            );
        }

        let add = mui::button(
            theme,
            ui,
            self.view_id,
            self.hover("machines:add"),
            Some(AppIcon::Plus),
            "Add machine",
            Variant::Primary,
            Size::Md,
            true,
            SettingsAction::Machine(MachineAction::Add),
        );
        if store.registry.machines.is_empty() {
            return page
                .with_child(mui::empty_state(
                    theme,
                    ui,
                    AppIcon::Server,
                    "No machines yet",
                    "Add a server you can reach over SSH. Its projects then open like any other project.",
                    Some(add),
                ))
                .with_child(
                    super::super::settings_tasks_hosts::render_host_settings_card(self, theme, &[]),
                )
                .finish();
        }

        let search = crate::shared_input::shared_input(
            theme,
            &self.machines.fields["search"],
            crate::shared_input::SharedInputOpts {
                leading_icon: Some(crate::shared_input::SharedInputIcon {
                    icon: AppIcon::Search,
                    size: 14.0,
                }),
                height: Some(mui::CONTROL_HEIGHT),
                radius: mui::RADIUS_CONTROL,
                pad_left: 10.0,
                ..Default::default()
            },
        );
        page = page.with_child(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(12.0)
                    .with_child(ConstrainedBox::new(search).with_width(320.0).finish())
                    .with_child(Expanded::new(1.0, mui::spacer(0.0)).finish())
                    .with_child(add)
                    .finish(),
            )
            .with_padding_bottom(8.0)
            .finish(),
        );

        let query = self.machines.query.trim().to_lowercase();
        let mut machines: Vec<&Machine> = store
            .registry
            .machines
            .iter()
            .filter(|m| {
                query.is_empty()
                    || [
                        m.name(),
                        m.connection.host.clone(),
                        m.connection.user.clone().unwrap_or_default(),
                    ]
                    .iter()
                    .any(|field| field.to_lowercase().contains(&query))
            })
            .collect();
        machines.sort_by_key(|m| m.name().to_lowercase());
        if machines.is_empty() {
            return page
                .with_child(mui::spacer(16.0))
                .with_child(mui::empty_state(
                    theme,
                    ui,
                    AppIcon::Search,
                    "No matching machines",
                    "Try a different name, host, or username.",
                    None,
                ))
                .finish();
        }
        let (connected, other): (Vec<&Machine>, Vec<&Machine>) = machines
            .into_iter()
            .partition(|m| store.connection(&m.id).is_some_and(|c| c.active()));
        let other_label = if connected.is_empty() { "Saved machines" } else { "Other machines" };
        // Remote servers list: the saved-machine rows below are
        // the add/remove surface; overrides come from HostSettings.
        let picker_entries: Vec<(String, String)> = store
            .registry
            .machines
            .iter()
            .map(|m| {
                (
                    m.id.clone(),
                    crate::settings::settings_tasks_hosts::display_name_for(&m.id, &m.name()),
                )
            })
            .collect();
        page = page.with_child(
            super::super::settings_tasks_hosts::render_host_settings_card(self, theme, &picker_entries),
        );
        for (label, group) in [("Connected", connected), (other_label, other)] {
            if group.is_empty() {
                continue;
            }
            let rows = group
                .iter()
                .map(|machine| self.render_machine_row(theme, store, machine))
                .collect();
            page = page
                .with_child(
                    Container::new(mui::eyebrow(ui, theme, label))
                        .with_padding_left(2.0)
                        .with_padding_top(20.0)
                        .with_padding_bottom(10.0)
                        .finish(),
                )
                .with_child(mui::list_card(theme, rows));
        }
        page.with_child(mui::spacer(24.0)).finish()
    }

    fn render_machine_row(&self, theme: &Theme, store: &MachinesStore, machine: &Machine) -> Box<dyn Element> {
        let ui = self.ui_font;
        let theme = *theme;
        let status = store.status(&machine.id);
        let name = crate::settings::settings_tasks_hosts::display_name_for(&machine.id, &machine.name());
        let mut detail = endpoint_label(machine);
        if let Some(env) = store.connection(&machine.id).and_then(|c| c.environment.as_ref()) {
            detail = format!("{detail}  ·  {}", mui::platform_label(&env.os, &env.arch));
        }
        let projects = store.projects_for(&machine.id).len();
        let view_id = self.view_id;
        let action = SettingsAction::Machine(MachineAction::Open(machine.id.clone()));
        Hoverable::new(self.hover(&format!("machines:row:{}", machine.id)), move |state| {
            let hovered = state.is_hovered();
            let mut trailing = Vec::new();
            if projects > 0 {
                trailing.push(mui::text(
                    ui,
                    mui::count(projects, "project", "projects"),
                    12.0,
                    theme.sub_text(),
                ));
            }
            trailing.push(mui::status_pill(&theme, ui, status));
            trailing.push(render_icon(
                AppIcon::ChevronRight,
                if hovered { theme.sub_text() } else { theme.disabled_text() },
                13.0,
            ));
            mui::inset_hover(
                &theme,
                hovered,
                mui::list_row(
                    ui,
                    &theme,
                    Some(mui::machine_avatar(&theme, status, 36.0)),
                    mui::medium(ui, name.clone(), 13.5, theme.main_text()),
                    Some(detail.clone()),
                    trailing,
                ),
            )
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
        .finish()
    }
}
