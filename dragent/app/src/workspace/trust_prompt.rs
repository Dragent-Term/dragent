//! Workspace trust gate shown before project files and executable tooling load.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use warpui::elements::{Align, Container, CrossAxisAlignment, Element, Flex, ParentElement, Text};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::EntityId;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::icons::{render_icon, AppIcon};
use crate::theme::{scaled, Theme};

use super::auth_modals::modal_scrim;
use super::WorkspaceAction;

type States = RefCell<HashMap<String, MouseStateHandle>>;

pub fn render(
    root: &Path,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let name = root
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("this folder");
    let panel = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(14.0)
            .with_child(
                Container::new(render_icon(AppIcon::FolderOpen, theme.accent, 21.0))
                    .with_uniform_padding(9.0)
                    .with_background_color(theme.surface_2())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
                    .finish(),
            )
            .with_child(
                Text::new(format!("Do you trust ‘{name}’?"), ui, crate::theme::text_size(18.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_child(
                Text::new(
                    "Trust allows Dragent to read project files, start language servers, and run project tooling. Only continue when you know the source of this folder.",
                    ui,
                    crate::theme::text_size(11.5),
                )
                .with_color(theme.sub_text())
                .soft_wrap(true)
                .finish(),
            )
            .with_child(
                Container::new(
                    Text::new(root.display().to_string(), ui, crate::theme::text_size(10.0))
                        .with_color(theme.disabled_text())
                        .soft_wrap(true)
                        .finish(),
                )
                .with_uniform_padding(9.0)
                .with_background_color(theme.surface_1())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                .finish(),
            )
            .with_child(
                Flex::row()
                    .with_spacing(8.0)
                    .with_child(button("Not now", false, WorkspaceAction::DismissWorkspaceTrust, theme, ui, id, states))
                    .with_child(button("Trust folder", true, WorkspaceAction::TrustPendingWorkspace, theme, ui, id, states))
                    .finish(),
            )
            .finish(),
    )
    .with_uniform_padding(22.0)
    // Trust is a modal decision surface above arbitrary terminal/editor
    // content. `panel_background()` is glass under image themes and allowed
    // that content to remain visible through the sheet.
    .with_background_color(theme.solid_bg())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(13.0)))
    .finish();
    modal_scrim(panel, WorkspaceAction::DismissWorkspaceTrust, id)
}

#[allow(clippy::too_many_arguments)]
fn button(
    label: &str,
    primary: bool,
    action: WorkspaceAction,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let state = states
        .borrow_mut()
        .entry(format!("trust_{label}"))
        .or_default()
        .clone();
    let label = label.to_string();
    Box::new(
        Hoverable::new(state, move |state| {
            Container::new(
                Align::new(
                    Text::new(label.clone(), ui, crate::theme::text_size(11.0))
                        .with_color(if primary {
                            theme.accent_text()
                        } else {
                            theme.main_text()
                        })
                        .with_style(Properties {
                            weight: if primary {
                                Weight::Semibold
                            } else {
                                Weight::Normal
                            },
                            ..Default::default()
                        })
                        .finish(),
                )
                .finish(),
            )
            .with_padding_left(13.0)
            .with_padding_right(13.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .with_background_color(if primary {
                theme.accent
            } else if state.is_hovered() {
                theme.surface_2()
            } else {
                theme.surface_1()
            })
            .with_border(Border::all(1.0).with_border_color(if primary {
                theme.accent
            } else {
                theme.outline()
            }))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .finish()
        })
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, action.clone())),
    )
}
