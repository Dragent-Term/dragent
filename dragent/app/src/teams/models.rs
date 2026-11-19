//! Shared model selection for coordination leads and workers.

//! at 3a2f0efb. CLI availability still depends on the installed version and account.
use crate::{
    theme::Theme,
    workspace::{InputEditor, PromptAgent},
};
use std::{cell::RefCell, collections::HashMap};
use warpui::elements::{
    ClippedScrollStateHandle, CrossAxisAlignment, Element, Flex, ParentElement,
};
use warpui::fonts::FamilyId;
use warpui::{AppContext, EntityId, SingletonEntity, ViewHandle};
use warpui_core::elements::MouseStateHandle;

pub(crate) fn choices(agent: PromptAgent, app: &AppContext) -> Vec<(String, String)> {
    // Hosted Dragent model IDs must never be offered as arbitrary CLI IDs.
    if agent != PromptAgent::Dragent {
        return cli_choices(agent);
    }
    dragent_choices(agent, app)
}

/// A terminal agent's models (id, label): the list the agent itself reports
/// when it has answered (see [`super::live_models`]), else the bundled
/// catalog. Needs no window.
pub(crate) fn cli_choices(agent: PromptAgent) -> Vec<(String, String)> {
    let key = if agent == PromptAgent::Cursor { "cursor-agent" } else { agent.command_key() };
    choices_for_key(key)
}

/// [`cli_choices`] by the agent's command (`claude`, `codex`, `opencode`…).
/// Never waits: the agent's own list when known (this run or saved), else
/// the bundled catalog.
pub(crate) fn choices_for_key(key: &str) -> Vec<(String, String)> {
    let key = if key == "cursor" { "cursor-agent" } else { key };
    if let Some(live) = super::live_models::models(key) {
        return live;
    }
    {
        static CATALOG: std::sync::OnceLock<HashMap<String, Vec<CatalogModel>>> =
            std::sync::OnceLock::new();
        let catalog = CATALOG.get_or_init(|| {
            serde_json::from_str(include_str!("cli_models.json"))
                .expect("bundled CLI model catalog must be valid")
        });
        return catalog
            .get(key)
            .into_iter()
            .flatten()
            .map(|model| (model.id.clone(), model.label.clone()))
            .collect();
    }
}

fn dragent_choices(agent: PromptAgent, app: &AppContext) -> Vec<(String, String)> {
    let store = crate::settings::AccountStore::handle(app);
    let mut entries: Vec<_> = store
        .as_ref(app)
        .data
        .as_ref()
        .into_iter()
        .flat_map(|data| &data.models)
        .filter(|model| {
            model.is_active
                && match agent {
                    PromptAgent::Dragent => true,
                    PromptAgent::Claude => model.provider == "anthropic",
                    PromptAgent::Codex => model.provider == "openai",
                    PromptAgent::Gemini => model.provider == "google",
                    // Other CLIs use their own catalogs / qualified identifiers.
                    _ => false,
                }
        })
        .map(|model| (model.id.clone(), model.name.clone()))
        .collect();
    if agent == PromptAgent::Dragent {
        let settings = crate::settings::AppSettings::global();
        entries.extend(
            settings
                .custom_models
                .iter()
                .map(|m| (m.model_id.clone(), m.name.clone())),
        );
        entries.push((crate::settings::AUTO_MODEL_ID.into(), "Auto".into()));
        if settings.has_any_user_credential() {
            entries.retain(|(id, _)| {
                id == crate::settings::AUTO_MODEL_ID || settings.has_user_credential_for_model(id)
            });
        }
    }
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    entries.dedup_by(|a, b| a.0 == b.0);
    entries
}

pub(crate) fn display_label(
    agent: PromptAgent,
    selected: Option<&str>,
    app: &AppContext,
) -> String {
    let default = crate::settings::AppSettings::global()
        .selected_model
        .clone();
    let selected = selected.or_else(|| (agent == PromptAgent::Dragent).then_some(default.as_str()));
    let Some(id) = selected else {
        return "Default".into();
    };
    choices(agent, app)
        .into_iter()
        .find(|(value, _)| value == id)
        .map(|(_, label)| label)
        .unwrap_or_else(|| {
            if id == crate::settings::AUTO_MODEL_ID {
                "Auto".into()
            } else {
                id.to_string()
            }
        })
}

#[derive(serde::Deserialize)]
struct CatalogModel {
    id: String,
    label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelTab {
    Favorites,
    Agent(PromptAgent),
}

pub(crate) struct ModelPanel<A> {
    pub tab: ModelTab,
    pub agents: Vec<(PromptAgent, Vec<(String, String)>)>,
    pub on_tab: std::rc::Rc<dyn Fn(ModelTab) -> A>,
    pub on_pick: std::rc::Rc<dyn Fn(PromptAgent, Option<String>) -> A>,
    pub on_favorite: std::rc::Rc<dyn Fn(crate::settings::AgentModelFavorite) -> A>,
}

pub(crate) fn prompt_panel(
    current: PromptAgent,
    installed: &std::collections::HashSet<PromptAgent>,
    app: &AppContext,
    tab: ModelTab,
) -> ModelPanel<crate::workspace::WorkspaceAction> {
    use crate::workspace::WorkspaceAction as A;
    ModelPanel {
        tab,
        agents: PromptAgent::ALL
            .into_iter()
            .filter(|agent| {
                *agent == current
                    || *agent == PromptAgent::Dragent
                    || (installed.contains(agent) && crate::cli::model::flag(*agent).is_some())
            })
            .map(|agent| (agent, choices(agent, app)))
            .collect(),
        on_tab: std::rc::Rc::new(|tab| A::BrowsePromptModelTab { tab }),
        on_pick: std::rc::Rc::new(|agent, model| A::PickPromptAgentModel { agent, model }),
        on_favorite: std::rc::Rc::new(|favorite| A::TogglePromptModelFavorite { favorite }),
    }
}

pub(crate) fn toggle_favorite(favorite: crate::settings::AgentModelFavorite) {
    let mut settings = crate::settings::AppSettings::load();
    if let Some(index) = settings
        .agent_model_favorites
        .iter()
        .position(|saved| saved.agent == favorite.agent && saved.model == favorite.model)
    {
        settings.agent_model_favorites.remove(index);
    } else {
        settings.agent_model_favorites.push(favorite);
    }
    settings.save();
}

#[derive(Clone)]
struct Row {
    agent: PromptAgent,
    id: Option<String>,
    label: String,
}

fn is_favorite(row: &Row, favorites: &[crate::settings::AgentModelFavorite]) -> bool {
    favorites.iter().any(|saved| {
        saved.agent == row.agent.command_key() && Some(saved.model.as_str()) == row.id.as_deref()
    })
}

fn visible_rows(
    tab: ModelTab,
    browsed: PromptAgent,
    available: &[(String, String)],
    allowed: &[PromptAgent],
    favorites: &[crate::settings::AgentModelFavorite],
    query: &str,
) -> Vec<Row> {
    let mut rows: Vec<Row> = if tab == ModelTab::Favorites {
        favorites
            .iter()
            .filter_map(|favorite| {
                let a = crate::automations::executor::prompt_agent(&favorite.agent)?;
                if !allowed.contains(&a) {
                    return None;
                }
                Some(Row {
                    agent: a,
                    id: Some(favorite.model.clone()),
                    label: favorite.label.clone(),
                })
            })
            .collect()
    } else {
        let mut rows = vec![Row {
            agent: browsed,
            id: None,
            label: if browsed == PromptAgent::Dragent {
                "Current model"
            } else {
                "Default"
            }
            .into(),
        }];
        rows.extend(available.iter().map(|(id, label)| Row {
            agent: browsed,
            id: Some(id.clone()),
            label: label.clone(),
        }));
        rows
    };
    let needle = query.trim().to_lowercase();
    rows.retain(|row| {
        format!(
            "{} {} {}",
            row.label,
            row.id.as_deref().unwrap_or(""),
            row.agent.label()
        )
        .to_lowercase()
        .contains(&needle)
    });
    // Preserve manual IDs without confusing them with the agent's known list.
    let custom = query.trim();
    let valid = if browsed == PromptAgent::Dragent {
        !custom.is_empty()
            && custom
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-._:/@[]".contains(c))
    } else {
        crate::cli::model::check(browsed.command_key(), custom).is_ok()
    };
    if rows.is_empty() && tab != ModelTab::Favorites && valid {
        rows.push(Row {
            agent: browsed,
            id: Some(custom.into()),
            label: format!("Use {custom}"),
        });
    }
    if tab != ModelTab::Favorites {
        rows.sort_by_key(|row| !is_favorite(row, &favorites));
    }
    rows
}


/// icon tabs, divided search, favorites first, 28px rows, shortcuts and stars.
#[allow(clippy::too_many_arguments)]
pub(crate) fn picker<A: warpui::Action + Clone + 'static>(
    theme: &Theme,
    ui: FamilyId,
    view: EntityId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    scroll: &ClippedScrollStateHandle,
    editor: &ViewHandle<InputEditor>,
    query: &str,
    agent: PromptAgent,
    selected: Option<&str>,
    choices: &[(String, String)],
    pick: impl Fn(Option<String>) -> A,
    dismiss: A,
    clear_search: A,
    panel: Option<ModelPanel<A>>,
) -> Box<dyn Element> {
    use crate::icons::{AppIcon, render_icon};
    use warpui::elements::{
        ConstrainedBox, Container, DispatchEventResult, Empty, EventHandler, Expanded, Text,
    };
    use warpui_core::{
        elements::Hoverable,
        platform::Cursor,
        scene::{Border, CornerRadius, Radius},
        text_layout::ClipConfig,
    };
    let mouse = |key: String| states.borrow_mut().entry(key).or_default().clone();
    let tab = panel
        .as_ref()
        .map(|panel| panel.tab)
        .unwrap_or(ModelTab::Agent(agent));
    let favorites = crate::settings::AppSettings::global()
        .agent_model_favorites
        .clone();
    let browsed = match tab {
        ModelTab::Agent(agent) => agent,
        ModelTab::Favorites => agent,
    };
    let available = panel
        .as_ref()
        .and_then(|panel| panel.agents.iter().find(|(a, _)| *a == browsed))
        .map(|(_, models)| models.as_slice())
        .unwrap_or(choices);
    let allowed: Vec<_> = panel
        .as_ref()
        .map(|panel| panel.agents.iter().map(|(agent, _)| *agent).collect())
        .unwrap_or_else(|| vec![agent]);
    let rows = visible_rows(tab, browsed, available, &allowed, &favorites, query);
    let choose = |row: &Row| {
        panel
            .as_ref()
            .map(|panel| (panel.on_pick)(row.agent, row.id.clone()))
            .unwrap_or_else(|| pick(row.id.clone()))
    };
    let row_actions: Vec<A> = rows.iter().take(9).map(&choose).collect();
    let mut body = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    let mut tab_actions = Vec::new();
    if let Some(panel) = &panel {
        let tabs = std::iter::once(ModelTab::Favorites)
            .chain(panel.agents.iter().map(|(a, _)| ModelTab::Agent(*a)))
            .collect::<Vec<_>>();
        let mut bar = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(2.0);
        for entry in &tabs {
            let active = *entry == tab;
            let theme = *theme;
            let entry = *entry;
            let action = (panel.on_tab)(entry);
            tab_actions.push((entry, action.clone()));
            let tab_button = Hoverable::new(mouse(format!("model-tab-{entry:?}")), move |state| {
                let icon = match entry {
                    ModelTab::Favorites => render_icon(
                        if active {
                            AppIcon::Asset("icons/star-filled.svg")
                        } else {
                            AppIcon::Star
                        },
                        if active {
                            theme.main_text()
                        } else {
                            theme.disabled_text()
                        },
                        14.0,
                    ),
                    ModelTab::Agent(agent) => crate::workspace::prompt_agent_image(
                        agent,
                        &theme,
                        16.0,
                        if active { 1.0 } else { 0.7 },
                    ),
                };
                let button = Container::new(
                    ConstrainedBox::new(warpui::elements::Align::new(icon).finish())
                        .with_width(28.0)
                        .with_height(28.0)
                        .finish(),
                )
                .with_background_color(if state.is_hovered() {
                    theme.surface_2()
                } else {
                    warpui::color::ColorU::transparent_black()
                })
                .with_border(
                    Border::bottom(if active { 2.0 } else { 0.0 })
                        .with_border_color(theme.main_text()),
                )
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(if active {
                    0.0
                } else {
                    6.0
                })))
                .finish();
                crate::machine_ui::with_tooltip(
                    &theme,
                    ui,
                    button,
                    state.is_hovered(),
                    match entry {
                        ModelTab::Favorites => "Favorites",
                        ModelTab::Agent(agent) => agent.label(),
                    },
                )
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view, action.clone()))
            .finish();
            bar = bar.with_child(tab_button);
        }
        body = body.with_child(
            Container::new(bar.finish())
                .with_padding_left(6.0)
                .with_padding_right(6.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
                .finish(),
        );
    }
    let clear = Hoverable::new(mouse("model-clear".into()), move |state| {
        render_icon(
            AppIcon::XClose,
            if state.is_hovered() {
                theme.main_text()
            } else {
                theme.disabled_text()
            },
            12.0,
        )
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view, clear_search.clone()))
    .finish();
    let search = crate::shared_input::shared_input(
        theme,
        editor,
        crate::shared_input::SharedInputOpts {
            leading_icon: Some(crate::shared_input::SharedInputIcon {
                icon: AppIcon::Search,
                size: 14.0,
            }),
            trailing: (!query.is_empty()).then_some(clear),
            height: Some(32.0),
            radius: 0.0,
            background: Some(warpui::color::ColorU::transparent_black()),
            border: Some(warpui::color::ColorU::transparent_black()),
            pad_left: 10.0,
            pad_right: 10.0,
            pad_top: 0.0,
            pad_bottom: 0.0,
            ..Default::default()
        },
    );
    body = body.with_child(
        Container::new(search)
            .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
            .finish(),
    );
    let mut list = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(1.0);
    let starred = rows.iter().any(|row| is_favorite(row, &favorites));
    let mut last_starred = None;
    for (index, row) in rows.iter().enumerate() {
        let favorited = is_favorite(row, &favorites);
        if tab != ModelTab::Favorites && starred && last_starred != Some(favorited) {
            list = list.with_child(crate::workspace::prompt_menu_caption(
                theme,
                ui,
                if favorited { "Favorites" } else { "All models" },
            ));
            last_starred = Some(favorited);
        }
        let active = row.agent == agent && row.id.as_deref() == selected;
        let name = row.label.clone();
        let from_agent = (tab == ModelTab::Favorites).then(|| row.agent.label().to_string());
        let theme = *theme;
        let star_action = panel.as_ref().and_then(|panel| {
            row.id.as_ref().map(|id| {
                (panel.on_favorite)(crate::settings::AgentModelFavorite {
                    agent: row.agent.command_key().into(),
                    model: id.clone(),
                    label: row.label.clone(),
                })
            })
        });
        let star_mouse = mouse(format!(
            "model-star-{}-{:?}",
            row.agent.command_key(),
            row.id
        ));
        let element = Hoverable::new(
            mouse(format!(
                "model-row-{}-{:?}",
                row.agent.command_key(),
                row.id
            )),
            move |state| {
                let mut line = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(
                        Expanded::new(
                            1.0,
                            Text::new(name.clone(), ui, crate::theme::text_size(13.0))
                                .with_color(theme.main_text())
                                .soft_wrap(false)
                                .with_clip(ClipConfig::ellipsis())
                                .finish(),
                        )
                        .finish(),
                    );
                if let Some(agent) = &from_agent {
                    line = line.with_child(
                        Text::new(agent.clone(), ui, crate::theme::text_size(11.0))
                            .with_color(theme.sub_text())
                            .finish(),
                    );
                }
                if index < 9 {
                    line = line.with_child(
                        Container::new(
                            Text::new(
                                format!(
                                    "{}{}",
                                    if cfg!(target_os = "macos") {
                                        "⌘"
                                    } else {
                                        "Ctrl "
                                    },
                                    index + 1
                                ),
                                ui,
                                crate::theme::text_size(10.0),
                            )
                            .with_color(theme.sub_text())
                            .finish(),
                        )
                        .with_background_color(theme.surface_2())
                        .with_padding_left(3.0)
                        .with_padding_right(3.0)
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                        .finish(),
                    );
                }
                line = line.with_child(
                    ConstrainedBox::new(if active {
                        render_icon(AppIcon::Check, theme.main_text(), 14.0)
                    } else {
                        Empty::new().finish()
                    })
                    .with_width(20.0)
                    .finish(),
                );
                let star_action = star_action.clone();
                let star = EventHandler::new(
                    Hoverable::new(star_mouse.clone(), move |state| {
                        ConstrainedBox::new(
                            warpui::elements::Align::new(render_icon(
                                if favorited {
                                    AppIcon::Asset("icons/star-filled.svg")
                                } else {
                                    AppIcon::Star
                                },
                                if favorited {
                                    theme.ui_yellow()
                                } else if state.is_hovered() {
                                    theme.main_text()
                                } else {
                                    theme.disabled_text()
                                },
                                12.0,
                            ))
                            .finish(),
                        )
                        .with_width(20.0)
                        .with_height(20.0)
                        .finish()
                    })
                    .with_cursor(Cursor::PointingHand)
                    .finish(),
                )
                .on_left_mouse_down(move |ctx, _, _| {
                    if let Some(action) = &star_action {
                        ctx.dispatch_typed_action_for(view, action.clone());
                    }
                    DispatchEventResult::StopPropagation
                })
                .finish();
                line = line.with_child(star);
                Container::new(
                    ConstrainedBox::new(line.finish())
                        .with_height(28.0)
                        .finish(),
                )
                .with_background_color(if state.is_hovered() {
                    theme.surface_2()
                } else {
                    warpui::color::ColorU::transparent_black()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
                .with_padding_left(8.0)
                .with_padding_right(4.0)
                .finish()
            },
        )
        .with_cursor(Cursor::PointingHand);
        let action = choose(row);
        let element = element
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view, action.clone()))
            .finish();
        list = list.with_child(element);
    }
    if rows.is_empty() {
        list = list.with_child(crate::workspace::prompt_menu_caption(
            theme,
            ui,
            if tab == ModelTab::Favorites && query.trim().is_empty() {
                "No favorite models"
            } else {
                "No matching models"
            },
        ));
    }
    body = body.with_child(
        Container::new(crate::workspace::prompt_menu_list(
            theme,
            scroll,
            list.finish(),
            200.0,
        ))
        .with_padding_left(4.0)
        .with_padding_right(4.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .finish(),
    );
    let theme = *theme;
    let content = ConstrainedBox::new(
        Container::new(body.finish())
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
            .finish(),
    )
    .with_width(296.0)
    .finish();
    EventHandler::new(content)
        .on_keydown(move |ctx, _, key| {
            let action = if key.key == "escape" {
                Some(dismiss.clone())
            } else if key.key == "enter" {
                row_actions.first().cloned()
            } else if (key.cmd || key.ctrl) && !key.alt {
                key.key
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0 && *n <= row_actions.len())
                    .and_then(|n| row_actions.get(n - 1))
                    .cloned()
            } else if key.key == "tab" && !tab_actions.is_empty() {
                let current = tab_actions
                    .iter()
                    .position(|(value, _)| *value == tab)
                    .unwrap_or(0);
                Some(
                    tab_actions[(current + if key.shift { tab_actions.len() - 1 } else { 1 })
                        % tab_actions.len()]
                    .1
                    .clone(),
                )
            } else {
                None
            };
            if let Some(action) = action {
                ctx.dispatch_typed_action_for(view, action);
                DispatchEventResult::StopPropagation
            } else {
                DispatchEventResult::PropagateToParent
            }
        })
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AgentModelFavorite as Favorite;

    #[test]
    fn cli_catalog_preserves_agent_specific_ids_and_recommended_order() {
        let catalog: HashMap<String, Vec<CatalogModel>> =
            serde_json::from_str(include_str!("cli_models.json")).unwrap();
        assert_eq!(catalog["claude"][0].id, "fable");
        assert!(catalog["claude"].iter().any(|m| m.id == "sonnet"));
        assert!(catalog["opencode"].iter().all(|m| m.id.contains('/')));
        for (key, models) in &catalog {
            let key = key.as_str();
            for model in models {
                assert!(
                    crate::cli::model::check(key, &model.id).is_ok(),
                    "{key}: {}",
                    model.id
                );
            }
        }
    }

    #[test]
    fn favorites_are_scoped_to_the_agent_and_never_change_model_ids() {
        let favorites = vec![Favorite {
            agent: "codex".into(),
            model: "same-id".into(),
            label: "Codex model".into(),
        }];
        let models = vec![
            ("first".into(), "First".into()),
            ("same-id".into(), "Same".into()),
        ];
        let claude = visible_rows(
            ModelTab::Agent(PromptAgent::Claude),
            PromptAgent::Claude,
            &models,
            &[PromptAgent::Claude],
            &favorites,
            "",
        );
        assert_eq!(claude[1].id.as_deref(), Some("first"));
        let codex = visible_rows(
            ModelTab::Agent(PromptAgent::Codex),
            PromptAgent::Codex,
            &models,
            &[PromptAgent::Codex],
            &favorites,
            "",
        );
        assert_eq!(codex[0].id.as_deref(), Some("same-id"));
        let tab = visible_rows(
            ModelTab::Favorites,
            PromptAgent::Claude,
            &models,
            &[PromptAgent::Claude],
            &favorites,
            "",
        );
        assert!(
            tab.is_empty(),
            "a task picker must not select a different agent's favorites"
        );
        let tab = visible_rows(
            ModelTab::Favorites,
            PromptAgent::Claude,
            &models,
            &[PromptAgent::Claude, PromptAgent::Codex],
            &favorites,
            "",
        );
        assert_eq!(tab[0].agent, PromptAgent::Codex);
        assert_eq!(tab[0].id.as_deref(), Some("same-id"));
    }

    #[test]
    fn search_selects_the_displayed_model_and_rejects_shell_syntax() {
        let models = vec![("provider/exact-id".into(), "Friendly name".into())];
        let rows = visible_rows(
            ModelTab::Agent(PromptAgent::OpenCode),
            PromptAgent::OpenCode,
            &models,
            &[PromptAgent::OpenCode],
            &[],
            "friendly",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id.as_deref(), Some("provider/exact-id"));
        let rows = visible_rows(
            ModelTab::Agent(PromptAgent::Claude),
            PromptAgent::Claude,
            &[],
            &[PromptAgent::Claude],
            &[],
            "opus;touch /tmp/x",
        );
        assert!(rows.is_empty());
    }
}
