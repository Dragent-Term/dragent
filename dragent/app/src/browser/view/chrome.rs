//! The browser pane's single bar and its ⋯ menu. The bar doubles as the pane
//! header: navigation, the address, page actions, then the pane's own split
//! and close actions, so a browser pane never stacks two bars.

use dragent_workspace_model::BrowserUrl;
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildAnchor, ChildView, ConstrainedBox, Container, CrossAxisAlignment,
    DispatchEventResult, Element, Empty, EventHandler, Expanded, Flex, MainAxisAlignment,
    MainAxisSize, OffsetPositioning, ParentElement, PositionedElementAnchor,
    PositionedElementOffsetBounds, SavePosition, Shrinkable, Text,
};
use warpui::geometry::vector::vec2f;
use warpui_core::elements::{Clipped, Hoverable};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use super::{BrowserAction, BrowserView, MenuPage, Phase};
use crate::icons::{render_icon, AppIcon};
use crate::theme::{text_size, PANEL_HEADER_HEIGHT};

/// The loading line sits inside the bar so the page never moves when a load
/// starts or finishes.
const PROGRESS_HEIGHT: f32 = 2.0;
const PILL_HEIGHT: f32 = 26.0;
const MENU_WIDTH: f32 = 248.0;
/// Visited pages listed in the menu's History page.
const HISTORY_ROWS: usize = 12;

impl BrowserView {
    fn menu_anchor(&self) -> String {
        format!("browser-menu-{:?}", self.view_id)
    }

    fn address_anchor(&self) -> String {
        format!("browser-address-{}", self.session.pane_id)
    }

    /// Visited pages matching the address being typed, under the field:
    /// `(list, dismiss catcher, anchor)`.
    pub(super) fn suggestions_overlay(
        &self,
        app: &warpui::AppContext,
    ) -> Option<(Box<dyn Element>, Box<dyn Element>, OffsetPositioning)> {
        if !self.address_focused || self.address_matches.is_empty() {
            return None;
        }
        let width = app
            .element_position_by_id_at_last_frame(self.window_id, self.address_anchor())
            .map_or(420.0, |bounds| bounds.width().max(280.0));
        let mut list = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min);
        for (index, visit) in self.address_matches.iter().enumerate() {
            let Ok(url) = BrowserUrl::parse(&visit.url) else { continue };
            list = list.with_child(self.suggestion_row(index, &visit.title, url));
        }
        let list = ConstrainedBox::new(
            Container::new(list.finish())
                .with_uniform_padding(4.0)
                .with_background_color(self.theme.surface_2())
                .with_border(Border::all(1.0).with_border_color(self.theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .finish(),
        )
        .with_width(width)
        .finish();
        let view_id = self.view_id;
        let catcher = EventHandler::new(Empty::new().finish())
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(view_id, BrowserAction::Escape);
                DispatchEventResult::StopPropagation
            })
            .finish();
        let offset = OffsetPositioning::offset_from_save_position_element(
            &self.address_anchor(),
            vec2f(0.0, 4.0),
            PositionedElementOffsetBounds::WindowByPosition,
            PositionedElementAnchor::BottomLeft,
            ChildAnchor::TopLeft,
        );
        Some((list, catcher, offset))
    }

    /// One suggestion: the page's site and path, then its title.
    fn suggestion_row(&self, index: usize, title: &str, url: BrowserUrl) -> Box<dyn Element> {
        let theme = self.theme;
        let font = self.font;
        let view_id = self.view_id;
        let picked = self.address_pick == Some(index);
        let (site, rest) = site_and_rest(&url);
        let address = format!("{site}{rest}");
        let title = title.trim().to_string();
        Hoverable::new(self.mouse(&format!("suggestion-{index}")), move |state| {
            let mut row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(render_icon(AppIcon::Asset("icons/history.svg"), theme.sub_text(), 12.0))
                .with_child(
                    Shrinkable::new(
                        1.0,
                        Text::new(address.clone(), font, text_size(12.5))
                            .with_color(theme.main_text())
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    )
                    .finish(),
                );
            if !title.is_empty() {
                row = row.with_child(
                    Shrinkable::new(
                        1.0,
                        Text::new(format!("— {title}"), font, text_size(12.0))
                            .with_color(theme.sub_text())
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    )
                    .finish(),
                );
            }
            Container::new(row.finish())
                .with_horizontal_padding(8.0)
                .with_vertical_padding(5.0)
                .with_background_color(if picked || state.is_hovered() {
                    theme.sidebar_row_hover()
                } else {
                    ColorU::transparent_black()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        // On press, as menus do: the list closes as soon as the field loses
        // the keyboard, so waiting for the release could miss the click.
        .on_mouse_down(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, BrowserAction::Open(url.clone()));
        })
        .finish()
    }

    pub(super) fn has_page(&self) -> bool {
        self.phase == Phase::Page && !self.session.url.is_blank()
    }

    pub(super) fn render_bar(&self) -> Box<dyn Element> {
        let theme = self.theme;
        let page = self.has_page();
        let reload = if self.loading && self.phase == Phase::Page {
            AppIcon::XClose
        } else {
            AppIcon::RefreshCw
        };
        let divider = Container::new(
            ConstrainedBox::new(Empty::new().finish())
                .with_width(1.0)
                .with_height(16.0)
                .finish(),
        )
        .with_background_color(theme.outline())
        .with_margin_left(4.0)
        .with_margin_right(4.0)
        .finish();
        let menu_button = SavePosition::new(
            self.icon_button("menu", AppIcon::Ellipsis, true, BrowserAction::ToggleMenu),
            &self.menu_anchor(),
        )
        .finish();
        let leading = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(1.0)
            .with_child(self.icon_button(
                "back",
                AppIcon::ArrowLeft,
                page && self.can_go_back,
                BrowserAction::Back,
            ))
            .with_child(self.icon_button(
                "forward",
                AppIcon::ArrowRight,
                page && self.can_go_forward,
                BrowserAction::Forward,
            ))
            .with_child(self.icon_button(
                "reload",
                reload,
                !matches!(self.phase, Phase::Start),
                BrowserAction::ReloadOrStop,
            ))
            .with_child(
                Expanded::new(
                    1.0,
                    Container::new(
                        SavePosition::new(self.render_address(), &self.address_anchor()).finish(),
                    )
                        .with_margin_left(6.0)
                        .with_margin_right(6.0)
                        .finish(),
                )
                .finish(),
            )
            .finish();
        let controls = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(1.0)
            .with_child(self.icon_button(
                "external",
                AppIcon::Asset("icons/external-link.svg"),
                !self.session.url.is_blank(),
                BrowserAction::OpenExternally,
            ))
            .with_child(self.render_design_controls())
            .with_child(menu_button);
        let controls = controls
            .with_child(divider)
            .with_child(self.icon_button(
                "split",
                AppIcon::SplitHorizontal,
                true,
                BrowserAction::SplitPane,
            ))
            .with_child(self.icon_button("close", AppIcon::XClose, true, BrowserAction::ClosePane))
            .finish();
        let controls = SavePosition::new(
            controls,
            &format!("browser-controls-{}", self.session.pane_id),
        )
        .finish();
        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_child(Shrinkable::new(1.0, Clipped::new(leading).finish()).finish())
            .with_child(controls)
            .finish();
        let progress: Box<dyn Element> = if self.loading {
            Container::new(Empty::new().finish())
                .with_background_color(theme.accent)
                .finish()
        } else {
            Container::new(Empty::new().finish())
                .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
                .finish()
        };
        let bar = ConstrainedBox::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(
                    ConstrainedBox::new(
                        Container::new(Clipped::new(row).finish())
                            .with_padding_left(6.0)
                            .with_padding_right(6.0)
                            .finish(),
                    )
                    .with_height(PANEL_HEADER_HEIGHT - PROGRESS_HEIGHT)
                    .finish(),
                )
                .with_child(
                    ConstrainedBox::new(progress)
                        .with_height(PROGRESS_HEIGHT)
                        .finish(),
                )
                .finish(),
        )
        .with_height(PANEL_HEADER_HEIGHT)
        .finish();
        SavePosition::new(bar, &format!("browser-toolbar-{}", self.session.pane_id)).finish()
    }

    fn design_menu_anchor(&self) -> String {
        format!("browser-design-menu-{:?}", self.view_id)
    }

    /// Design, then while it is on: Draw, and "Send N to <agent> ▾" once
    /// there are notes. Sending lives here, in the app, not in the page.
    fn render_design_controls(&self) -> Box<dyn Element> {
        let count = self.design.notes.len();
        let label = if count == 0 {
            "⌖ Design".to_string()
        } else {
            format!("⌖ Design {count}")
        };
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(self.pill_button(
                "design",
                &label,
                self.design.enabled,
                self.has_page(),
                BrowserAction::ToggleDesign,
            ));
        if !self.design.enabled {
            return row.finish();
        }
        row = row.with_child(self.pill_button(
            "design-draw",
            "✎ Draw",
            false,
            !self.design.capturing,
            BrowserAction::DesignDraw,
        ));
        if count > 0 {
            row = row.with_child(self.render_send_button(count));
        }
        row.finish()
    }

    fn pill_button(
        &self,
        key: &str,
        label: &str,
        active: bool,
        enabled: bool,
        action: BrowserAction,
    ) -> Box<dyn Element> {
        let theme = self.theme;
        let font = self.font;
        let view_id = self.view_id;
        let label = label.to_string();
        let button = Hoverable::new(self.mouse(key), move |state| {
            Container::new(
                Text::new(label.clone(), font, text_size(11.5))
                    .with_color(if !enabled {
                        theme.disabled_text()
                    } else if active {
                        theme.accent_text()
                    } else {
                        theme.main_text()
                    })
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            )
            .with_horizontal_padding(8.0)
            .with_vertical_padding(4.0)
            .with_background_color(if active {
                theme.accent
            } else if enabled && state.is_hovered() {
                theme.surface_2()
            } else {
                theme.surface_1()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
        });
        if !enabled {
            return button.finish();
        }
        button
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
            .finish()
    }

    /// The shared agent chip (artwork and name of the session or agent the
    /// notes go to; opens the agent menu), then "Send N ⌘↵".
    fn render_send_button(&self, count: usize) -> Box<dyn Element> {
        let theme = self.theme;
        let font = self.font;
        let view_id = self.view_id;
        let chip = crate::workspace::agent_target_chip(
            &theme,
            font,
            self.design_target(),
            self.mouse("design-agents"),
            move |ctx| ctx.dispatch_typed_action_for(view_id, BrowserAction::ToggleDesignMenu),
        );
        let sending = !self.design.sending.is_empty();
        let label = if sending {
            "Sending…".to_string()
        } else {
            format!("Send {count}")
        };
        let shortcut = if cfg!(target_os = "macos") { "⌘↵" } else { "Ctrl↵" };
        let send = Hoverable::new(self.mouse("design-send"), move |state| {
            let foreground = theme.accent_text();
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(
                        Text::new(label.clone(), font, text_size(11.5))
                            .with_color(foreground)
                            .finish(),
                    )
                    .with_child(
                        Text::new(shortcut.to_string(), font, text_size(10.0))
                            .with_color(ColorU::new(foreground.r, foreground.g, foreground.b, 180))
                            .finish(),
                    )
                    .finish(),
            )
            .with_horizontal_padding(9.0)
            .with_vertical_padding(4.0)
            .with_background_color(if state.is_hovered() && !sending {
                ColorU::new(theme.accent.r, theme.accent.g, theme.accent.b, 215)
            } else {
                theme.accent
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, BrowserAction::SendDesign))
        .finish();
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(SavePosition::new(chip, &self.design_menu_anchor()).finish())
            .with_child(send)
            .finish()
    }

    /// The agent menu every agent picker uses: sessions running in this
    /// workspace, then installed agents that would start a new session.
    pub(super) fn design_menu_overlay(
        &self,
    ) -> Option<(Box<dyn Element>, Box<dyn Element>, OffsetPositioning)> {
        if !self.design_menu_open || !self.design.enabled {
            return None;
        }
        let view_id = self.view_id;
        let menu = crate::workspace::render_agent_target_menu(crate::workspace::AgentTargetMenuProps {
            theme: &self.theme,
            ui: self.font,
            states: &self.mouse,
            scroll: &self.design_scroll,
            search_editor: &self.design_search,
            search: &self.design_search_text,
            targets: &self.design.targets,
            selected: self.design.target.as_deref(),
            active: None,
            pick: std::rc::Rc::new(move |ctx, key| {
                ctx.dispatch_typed_action_for(view_id, BrowserAction::SelectDesignTarget(key))
            }),
            clear_search: std::rc::Rc::new(move |ctx| {
                ctx.dispatch_typed_action_for(view_id, BrowserAction::ClearDesignSearch)
            }),
        });
        let catcher = EventHandler::new(Empty::new().finish())
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(view_id, BrowserAction::CloseDesignMenu);
                DispatchEventResult::StopPropagation
            })
            .finish();
        let offset = OffsetPositioning::offset_from_save_position_element(
            &self.design_menu_anchor(),
            vec2f(0.0, 4.0),
            PositionedElementOffsetBounds::WindowByPosition,
            PositionedElementAnchor::BottomRight,
            ChildAnchor::TopRight,
        );
        Some((menu, catcher, offset))
    }

    /// The address pill: the site at rest (host emphasised, path dimmed),
    /// the full address while editing.
    fn render_address(&self) -> Box<dyn Element> {
        let theme = self.theme;
        let font = self.font;
        let view_id = self.view_id;
        let url = &self.session.url;
        let lead = if self.address_focused || url.is_blank() {
            render_icon(AppIcon::Search, theme.sub_text(), 12.0)
        } else if url.as_str().starts_with("https://") {
            render_icon(AppIcon::Asset("icons/lock.svg"), theme.sub_text(), 11.0)
        } else {
            render_icon(AppIcon::Globe, theme.sub_text(), 12.0)
        };
        let zoom = (self.zoom - 1.0).abs() > f64::EPSILON;
        let zoom_badge = zoom.then(|| {
            let label = format!("{:.0}%", self.zoom * 100.0);
            Hoverable::new(self.mouse("zoom-badge"), move |state| {
                Container::new(
                    Text::new(label.clone(), font, text_size(11.0))
                        .with_color(if state.is_hovered() {
                            theme.main_text()
                        } else {
                            theme.sub_text()
                        })
                        .finish(),
                )
                .with_horizontal_padding(6.0)
                .with_vertical_padding(1.0)
                .with_background_color(theme.surface_2())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(view_id, BrowserAction::Zoom(0))
            })
            .finish()
        });

        let editing = self.address_focused;
        let body: Box<dyn Element> = if editing {
            ChildView::new(&self.address).finish()
        } else if url.is_blank() {
            Text::new(
                "Search or enter an address".to_string(),
                font,
                text_size(12.5),
            )
            .with_color(theme.disabled_text())
            .soft_wrap(false)
            .finish()
        } else {
            let (site, rest) = site_and_rest(url);
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Text::new(site, font, text_size(12.5))
                        .with_color(theme.main_text())
                        .soft_wrap(false)
                        .with_clip(ClipConfig::ellipsis())
                        .finish(),
                )
                .with_child(
                    Shrinkable::new(
                        1.0,
                        Text::new(rest, font, text_size(12.5))
                            .with_color(theme.sub_text())
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    )
                    .finish(),
                )
                .finish()
        };
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(lead)
            .with_child(Expanded::new(1.0, body).finish());
        if let Some(badge) = zoom_badge {
            row = row.with_child(badge);
        }
        let row = row.finish();
        let field = move |hovered: bool, row: Box<dyn Element>| {
            ConstrainedBox::new(
                Container::new(row)
                    .with_padding_left(10.0)
                    .with_padding_right(6.0)
                    .with_background_color(if hovered || editing {
                        theme.surface_2()
                    } else {
                        theme.background
                    })
                    .with_border(Border::all(1.0).with_border_color(if editing {
                        theme.accent
                    } else {
                        theme.outline()
                    }))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                    .finish(),
            )
            .with_height(PILL_HEIGHT)
            .finish()
        };
        if editing {
            let suggesting = !self.address_matches.is_empty();
            return EventHandler::new(field(false, row))
                .on_keydown(move |ctx, _, key| match key.key.as_str() {
                    "enter" => {
                        ctx.dispatch_typed_action_for(view_id, BrowserAction::Go);
                        DispatchEventResult::StopPropagation
                    }
                    "down" if suggesting => {
                        ctx.dispatch_typed_action_for(view_id, BrowserAction::MoveSuggestion(1));
                        DispatchEventResult::StopPropagation
                    }
                    "up" if suggesting => {
                        ctx.dispatch_typed_action_for(view_id, BrowserAction::MoveSuggestion(-1));
                        DispatchEventResult::StopPropagation
                    }
                    "escape" => {
                        ctx.dispatch_typed_action_for(view_id, BrowserAction::Escape);
                        DispatchEventResult::StopPropagation
                    }
                    _ => DispatchEventResult::PropagateToParent,
                })
                .finish();
        }
        // At rest the pill is a button: one click selects the whole address.
        Hoverable::new(self.mouse("address"), move |state| {
            field(state.is_hovered(), row)
        })
        .with_cursor(Cursor::IBeam)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, BrowserAction::FocusAddressBar)
        })
        .finish()
    }

    /// `(menu, dismiss catcher, anchor)` while the ⋯ menu is open.
    pub(super) fn menu_overlay(
        &self,
    ) -> Option<(Box<dyn Element>, Box<dyn Element>, OffsetPositioning)> {
        if !self.menu_open {
            return None;
        }
        let theme = self.theme;
        let page = self.has_page();
        let has_address = !self.session.url.is_blank();
        let separator = || {
            Container::new(
                ConstrainedBox::new(Empty::new().finish())
                    .with_height(1.0)
                    .finish(),
            )
            .with_background_color(theme.outline())
            .with_margin_top(4.0)
            .with_margin_bottom(4.0)
            .finish()
        };
        let menu = match self.menu_page {
            MenuPage::Actions => self.actions_menu(page, has_address, &separator),
            MenuPage::History => self.history_menu(&separator),
            MenuPage::ConfirmClear => self.confirm_clear_menu(),
            MenuPage::Import => self.import_menu(&separator),
        };
        let menu = ConstrainedBox::new(
            Container::new(menu)
                .with_uniform_padding(5.0)
                .with_background_color(theme.surface_2())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .finish(),
        )
        .with_width(MENU_WIDTH)
        .finish();
        let view_id = self.view_id;
        let catcher = EventHandler::new(Empty::new().finish())
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(view_id, BrowserAction::CloseMenu);
                DispatchEventResult::StopPropagation
            })
            .finish();
        let offset = OffsetPositioning::offset_from_save_position_element(
            &self.menu_anchor(),
            vec2f(0.0, 4.0),
            PositionedElementOffsetBounds::WindowByPosition,
            PositionedElementAnchor::BottomRight,
            ChildAnchor::TopRight,
        );
        Some((menu, catcher, offset))
    }

    /// The ⋯ menu's main page.
    fn actions_menu(
        &self,
        page: bool,
        has_address: bool,
        separator: &dyn Fn() -> Box<dyn Element>,
    ) -> Box<dyn Element> {
        let mac = cfg!(target_os = "macos");
        let keys = |mac_keys: &'static str, other: &'static str| Some(if mac { mac_keys } else { other });
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(self.menu_row(
                "find",
                AppIcon::Search,
                "Find in page…",
                keys("⌘F", "Ctrl+F"),
                page,
                BrowserAction::Find,
            ))
            .with_child(self.zoom_row(page))
            .with_child(self.check_row(
                "mobile",
                "Mobile view",
                keys("⇧⌘M", "Ctrl+Shift+M"),
                page,
                self.mobile,
                BrowserAction::ToggleMobile,
            ))
            .with_child(separator())
            .with_child(self.menu_row(
                "copy",
                AppIcon::Copy,
                "Copy address",
                None,
                has_address,
                BrowserAction::CopyAddress,
            ))
            .with_child(self.menu_row(
                "external",
                AppIcon::Asset("icons/external-link.svg"),
                "Open in your browser",
                None,
                has_address,
                BrowserAction::OpenExternally,
            ))
            .with_child(self.menu_row(
                "screenshot",
                AppIcon::Asset("icons/image.svg"),
                "Take a screenshot",
                None,
                page,
                BrowserAction::Screenshot,
            ))
            .with_child(self.menu_row(
                "print",
                AppIcon::Asset("icons/printer.svg"),
                "Print…",
                None,
                page,
                BrowserAction::Print,
            ))
            .with_child(separator())
            .with_child(self.menu_row(
                "reload",
                AppIcon::RefreshCw,
                "Reload",
                keys("⌘R", "Ctrl+R"),
                page,
                BrowserAction::Reload,
            ))
            .with_child(self.menu_row(
                "hard",
                AppIcon::RotateCcw,
                "Reload without cache",
                keys("⇧⌘R", "Ctrl+Shift+R"),
                page,
                BrowserAction::HardReload,
            ))
            .with_child(separator())
            .with_child(self.menu_row(
                "history",
                AppIcon::Asset("icons/history.svg"),
                "History",
                Some("›"),
                true,
                BrowserAction::MenuPage(MenuPage::History),
            ))
            .with_child(self.menu_row(
                "downloads",
                AppIcon::Asset("icons/download.svg"),
                "Downloads",
                None,
                true,
                BrowserAction::ShowDownloads,
            ))
            .with_child(self.menu_row(
                "import",
                AppIcon::Asset("icons/key.svg"),
                "Import sign-ins…",
                Some("›"),
                cfg!(not(target_family = "wasm")),
                BrowserAction::MenuPage(MenuPage::Import),
            ))
            .with_child(self.menu_row(
                "clear-data",
                AppIcon::Asset("icons/trash-2.svg"),
                "Clear browsing data…",
                None,
                page,
                BrowserAction::MenuPage(MenuPage::ConfirmClear),
            ))
            .with_child(separator())
            .with_child(self.menu_row(
                "inspect",
                AppIcon::Code,
                "Inspect element",
                None,
                page,
                BrowserAction::DevTools,
            ))
            .finish()
    }

    /// Recently visited pages of this browser profile, newest first.
    fn history_menu(&self, separator: &dyn Fn() -> Box<dyn Element>) -> Box<dyn Element> {
        let visits = super::super::history::recent(&self.profile, HISTORY_ROWS);
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(self.menu_row(
                "history-back",
                AppIcon::Asset("icons/chevron-left.svg"),
                "History",
                None,
                true,
                BrowserAction::MenuPage(MenuPage::Actions),
            ))
            .with_child(separator());
        if visits.is_empty() {
            column = column.with_child(
                Container::new(
                    Text::new("Pages you visit show up here.".to_string(), self.font, text_size(12.0))
                        .with_color(self.theme.disabled_text())
                        .finish(),
                )
                .with_horizontal_padding(8.0)
                .with_vertical_padding(6.0)
                .finish(),
            );
        }
        for (index, visit) in visits.iter().enumerate() {
            let Ok(url) = BrowserUrl::parse(&visit.url) else {
                continue;
            };
            column = column.with_child(self.history_row(index, &visit.title, url));
        }
        if !visits.is_empty() {
            column = column.with_child(separator()).with_child(self.menu_row(
                "history-clear",
                AppIcon::Asset("icons/trash-2.svg"),
                "Clear history",
                None,
                true,
                BrowserAction::ClearHistory,
            ));
        }
        column.finish()
    }

    /// One visited page: its title over its site.
    fn history_row(&self, index: usize, title: &str, url: BrowserUrl) -> Box<dyn Element> {
        let theme = self.theme;
        let font = self.font;
        let view_id = self.view_id;
        let site = url.host_label();
        let title = if title.trim().is_empty() { site.clone() } else { title.trim().to_string() };
        Hoverable::new(self.mouse(&format!("history-{index}")), move |state| {
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_child(
                        Text::new(title.clone(), font, text_size(12.5))
                            .with_color(theme.main_text())
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    )
                    .with_child(
                        Text::new(site.clone(), font, text_size(11.0))
                            .with_color(theme.disabled_text())
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    )
                    .finish(),
            )
            .with_horizontal_padding(8.0)
            .with_vertical_padding(4.0)
            .with_background_color(if state.is_hovered() {
                theme.sidebar_row_hover()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, BrowserAction::Open(url.clone()));
            ctx.dispatch_typed_action_for(view_id, BrowserAction::CloseMenu);
        })
        .finish()
    }

    /// Other browsers' profiles to import sign-ins (cookies) from.
    fn import_menu(&self, separator: &dyn Fn() -> Box<dyn Element>) -> Box<dyn Element> {
        #[allow(unused_mut)]
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(self.menu_row(
                "import-back",
                AppIcon::Asset("icons/chevron-left.svg"),
                "Import sign-ins",
                None,
                true,
                BrowserAction::MenuPage(MenuPage::Actions),
            ))
            .with_child(separator());
        #[cfg(not(target_family = "wasm"))]
        {
            if self.import_sources.is_empty() {
                column = column.with_child(self.menu_note("No other browsers with sign-ins were found."));
            }
            for (index, source) in self.import_sources.iter().enumerate() {
                column = column.with_child(self.menu_row(
                    &format!("import-{index}"),
                    AppIcon::Globe,
                    &source.label(),
                    None,
                    true,
                    BrowserAction::ImportCookies(index),
                ));
            }
            column = column.with_child(separator()).with_child(self.menu_note(
                "Copies that browser's cookies into this organisation's browser, so you stay signed in. Nothing leaves this computer.",
            ));
        }
        column.finish()
    }

    /// A short line of explanation inside the menu.
    fn menu_note(&self, text: &str) -> Box<dyn Element> {
        Container::new(
            Text::new(text.to_string(), self.font, text_size(11.0))
                .with_color(self.theme.disabled_text())
                .finish(),
        )
        .with_horizontal_padding(8.0)
        .with_vertical_padding(4.0)
        .finish()
    }

    /// "Clear browsing data?" with what it removes, Cancel and Clear.
    fn confirm_clear_menu(&self) -> Box<dyn Element> {
        let theme = self.theme;
        let font = self.font;
        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(6.0)
                .with_child(
                    Text::new("Clear browsing data?".to_string(), font, text_size(13.0))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_child(
                    Text::new(
                        "Removes cookies, cached files, site data and history from this organisation's browser. You'll be signed out of sites.".to_string(),
                        font,
                        text_size(12.0),
                    )
                    .with_color(theme.sub_text())
                    .finish(),
                )
                .with_child(
                    Container::new(
                        Flex::row()
                            .with_main_axis_size(MainAxisSize::Max)
                            .with_main_axis_alignment(MainAxisAlignment::End)
                            .with_spacing(6.0)
                            .with_child(self.text_button(
                                "clear-cancel",
                                "Cancel",
                                false,
                                BrowserAction::MenuPage(MenuPage::Actions),
                            ))
                            .with_child(self.text_button(
                                "clear-confirm",
                                "Clear",
                                true,
                                BrowserAction::ClearBrowsingData,
                            ))
                            .finish(),
                    )
                    .with_margin_top(4.0)
                    .finish(),
                )
                .finish(),
        )
        .with_uniform_padding(8.0)
        .finish()
    }

    /// A menu row with a check mark while `on`.
    fn check_row(
        &self,
        key: &str,
        label: &str,
        shortcut: Option<&'static str>,
        enabled: bool,
        on: bool,
        action: BrowserAction,
    ) -> Box<dyn Element> {
        let icon = if on {
            AppIcon::Asset("icons/check.svg")
        } else {
            AppIcon::Asset("icons/smartphone.svg")
        };
        self.menu_row(key, icon, label, shortcut, enabled, action)
    }

    /// The find bar under the toolbar: query, "3 of 12", previous, next, close.
    pub(super) fn render_find_bar(&self) -> Box<dyn Element> {
        let theme = self.theme;
        let font = self.font;
        let view_id = self.view_id;
        let found = self.find_result.label(&self.find_text);
        let none = self.find_result.total == 0 && !self.find_text.is_empty();
        let field = EventHandler::new(
            ConstrainedBox::new(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(8.0)
                        .with_child(render_icon(AppIcon::Search, theme.sub_text(), 12.0))
                        .with_child(Expanded::new(1.0, ChildView::new(&self.find).finish()).finish())
                        .with_children(found.map(|found| {
                            Text::new(found, font, text_size(11.5))
                                .with_color(if none { theme.ansi_red() } else { theme.sub_text() })
                                .soft_wrap(false)
                                .finish()
                        }))
                        .finish(),
                )
                .with_padding_left(10.0)
                .with_padding_right(8.0)
                .with_background_color(theme.background)
                .with_border(Border::all(1.0).with_border_color(theme.accent))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                .finish(),
            )
            .with_height(PILL_HEIGHT)
            .with_max_width(360.0)
            .finish(),
        )
        .on_keydown(move |ctx, _, key| match key.key.as_str() {
            "enter" => {
                let step = if key.shift { -1 } else { 1 };
                ctx.dispatch_typed_action_for(view_id, BrowserAction::FindNext(step));
                DispatchEventResult::StopPropagation
            }
            "escape" => {
                ctx.dispatch_typed_action_for(view_id, BrowserAction::CloseFind);
                DispatchEventResult::StopPropagation
            }
            _ => DispatchEventResult::PropagateToParent,
        })
        .finish();
        let has_matches = self.find_result.total > 0;
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(2.0)
                .with_child(Shrinkable::new(1.0, field).finish())
                .with_child(self.icon_button(
                    "find-previous",
                    AppIcon::Asset("icons/chevron-up.svg"),
                    has_matches,
                    BrowserAction::FindNext(-1),
                ))
                .with_child(self.icon_button(
                    "find-next",
                    AppIcon::Asset("icons/chevron-down.svg"),
                    has_matches,
                    BrowserAction::FindNext(1),
                ))
                .with_child(self.icon_button("find-close", AppIcon::XClose, true, BrowserAction::CloseFind))
                .finish(),
        )
        .with_padding_left(8.0)
        .with_padding_right(6.0)
        .with_vertical_padding(4.0)
        .with_background_color(theme.surface_1())
        .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
        .finish()
    }

    fn menu_row(
        &self,
        key: &str,
        icon: AppIcon,
        label: &str,
        shortcut: Option<&'static str>,
        enabled: bool,
        action: BrowserAction,
    ) -> Box<dyn Element> {
        let theme = self.theme;
        let font = self.font;
        let view_id = self.view_id;
        let label = label.to_string();
        let row = Hoverable::new(self.mouse(&format!("menu-{key}")), move |state| {
            let color = if enabled {
                theme.main_text()
            } else {
                theme.disabled_text()
            };
            let mut row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(10.0)
                .with_child(render_icon(
                    icon,
                    if enabled {
                        theme.sub_text()
                    } else {
                        theme.disabled_text()
                    },
                    14.0,
                ))
                .with_child(
                    Expanded::new(
                        1.0,
                        Text::new(label.clone(), font, text_size(12.5))
                            .with_color(color)
                            .finish(),
                    )
                    .finish(),
                );
            if let Some(shortcut) = shortcut {
                row = row.with_child(
                    Text::new(shortcut.to_string(), font, text_size(11.5))
                        .with_color(theme.disabled_text())
                        .finish(),
                );
            }
            Container::new(row.finish())
                .with_horizontal_padding(8.0)
                .with_vertical_padding(6.0)
                .with_background_color(if enabled && state.is_hovered() {
                    theme.sidebar_row_hover()
                } else {
                    ColorU::transparent_black()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                .finish()
        });
        if !enabled {
            return row.finish();
        }
        row.with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(view_id, action.clone());
                if !matches!(action, BrowserAction::MenuPage(_)) {
                    ctx.dispatch_typed_action_for(view_id, BrowserAction::CloseMenu);
                }
            })
            .finish()
    }

    /// "Zoom  [−] 100% [+]" — stays open while adjusting.
    fn zoom_row(&self, enabled: bool) -> Box<dyn Element> {
        let theme = self.theme;
        let font = self.font;
        let percent = format!("{:.0}%", self.zoom * 100.0);
        let color = if enabled {
            theme.main_text()
        } else {
            theme.disabled_text()
        };
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(4.0)
                .with_child(
                    ConstrainedBox::new(
                        Align::new(render_icon(
                            AppIcon::Search,
                            if enabled {
                                theme.sub_text()
                            } else {
                                theme.disabled_text()
                            },
                            14.0,
                        ))
                        .finish(),
                    )
                    .with_width(14.0)
                    .finish(),
                )
                .with_child(
                    Expanded::new(
                        1.0,
                        Container::new(
                            Text::new("Zoom".to_string(), font, text_size(12.5))
                                .with_color(color)
                                .finish(),
                        )
                        .with_margin_left(6.0)
                        .finish(),
                    )
                    .finish(),
                )
                .with_child(self.icon_button(
                    "zoom-out",
                    AppIcon::Asset("icons/minus.svg"),
                    enabled,
                    BrowserAction::Zoom(-1),
                ))
                .with_child(
                    ConstrainedBox::new(
                        Align::new(
                            Text::new(percent, font, text_size(12.0))
                                .with_color(color)
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_width(42.0)
                    .finish(),
                )
                .with_child(self.icon_button(
                    "zoom-in",
                    AppIcon::Plus,
                    enabled,
                    BrowserAction::Zoom(1),
                ))
                .finish(),
        )
        .with_padding_left(8.0)
        .with_padding_right(2.0)
        .with_vertical_padding(2.0)
        .finish()
    }
}

/// `localhost:5173` and `/dashboard?tab=1` for display; the scheme is shown
/// by the lock or globe beside it.
fn site_and_rest(url: &BrowserUrl) -> (String, String) {
    let Ok(parsed) = url::Url::parse(url.as_str()) else {
        return (url.as_str().to_string(), String::new());
    };
    let site = url.host_label();
    let mut rest = parsed.path().to_string();
    if rest == "/" {
        rest.clear();
    }
    if let Some(query) = parsed.query() {
        rest.push('?');
        rest.push_str(query);
    }
    if let Some(fragment) = parsed.fragment() {
        rest.push('#');
        rest.push_str(fragment);
    }
    (site, rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_shows_site_then_path() {
        let split = |s: &str| site_and_rest(&BrowserUrl::from_address_bar(s).unwrap());
        assert_eq!(
            split("localhost:5173"),
            ("localhost:5173".into(), String::new())
        );
        assert_eq!(
            split("https://www.example.com/docs/a?x=1#top"),
            ("example.com".into(), "/docs/a?x=1#top".into())
        );
    }
}
