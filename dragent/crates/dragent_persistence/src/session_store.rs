use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::Utc;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use uuid::Uuid;

use crate::schema::{agent_blocks, app, blocks, pane_nodes, tabs, terminal_panes};
use crate::types::{
    PersistedAgentBlock, PersistedSession, RestoredAgentBlockLists, RestoredBlockLists,
    SerializedPaneLayout, SerializedSession, SerializedSplitDirection, SerializedTab, WindowChrome,
};
use crate::working_copy_store::{
    load_document_session, load_working_copies, save_document_session,
};
use crate::workspace_store::{
    load_canonical_workspace_events, load_recent_workspace_events, load_recent_workspace_work_items,
};

pub fn load_session(conn: &mut SqliteConnection) -> Result<Option<PersistedSession>> {
    let tab_count: i64 = tabs::table.count().get_result(conn).unwrap_or(0);
    let document = load_document_session(conn)?;
    if !document.belongs_to_organisation(&crate::current_org()) {
        return Ok(None);
    }
    let has_project =
        document.open_project_roots.iter().any(|root| root.is_dir()) || document.root.is_dir();
    if tab_count == 0 && !has_project {
        return Ok(None);
    }

    let app_record: (
        i32,
        Option<Vec<u8>>,
        i32,
        Option<f32>,
        i32,
        Option<f32>,
        Option<String>,
        i32,
    ) = app::table
        .select((
            app::active_tab_id,
            app::active_pane_uuid,
            app::left_panel_open,
            app::left_panel_width,
            app::right_panel_open,
            app::right_panel_width,
            app::layout_json,
            app::pane_layout_version,
        ))
        .first(conn)
        .context("read app")?;

    let tab_rows: Vec<(
        i32,
        i32,
        i32,
        String,
        Option<String>,
        Option<String>,
        i32,
        Option<Vec<u8>>,
    )> = tabs::table
        .select((
            tabs::id,
            tabs::logical_id,
            tabs::sort_index,
            tabs::title,
            tabs::custom_label,
            tabs::color,
            tabs::hidden_from_sidebar,
            tabs::owner_pane_uuid,
        ))
        .order(tabs::sort_index.asc())
        .load(conn)
        .context("load tabs")?;

    let mut restored_tabs = Vec::new();
    for (
        tab_row_id,
        logical_id,
        _sort,
        title,
        custom_label,
        color,
        hidden_from_sidebar,
        owner_pane_uuid,
    ) in tab_rows
    {
        let root =
            read_layout_tree(conn, tab_row_id, app_record.7 >= 1).context("read pane tree")?;
        restored_tabs.push(SerializedTab {
            id: logical_id as usize,
            title,
            custom_label,
            color,
            root_layout: root,
            hidden_from_sidebar: hidden_from_sidebar != 0,
            owner_pane_uuid: owner_pane_uuid.as_deref().map(bytes_to_uuid).transpose()?,
        });
    }
    // Version-0 databases carried split members as redundant Single rows but
    // had no persisted sidebar-visibility bit. Treat those redundant members
    // as split-only panes so upgrading does not suddenly create sidebar tabs.
    if app_record.7 == 0 {
        fn leaf_uuids(layout: &SerializedPaneLayout, out: &mut Vec<Uuid>) {
            match layout {
                SerializedPaneLayout::Single { uuid, .. } => out.push(*uuid),
                SerializedPaneLayout::Split {
                    left_or_top,
                    right_or_bottom,
                    ..
                } => {
                    leaf_uuids(left_or_top, out);
                    leaf_uuids(right_or_bottom, out);
                }
            }
        }
        let mut split_members = Vec::new();
        for tab in &restored_tabs {
            if matches!(tab.root_layout, SerializedPaneLayout::Split { .. }) {
                leaf_uuids(&tab.root_layout, &mut split_members);
            }
        }
        for tab in &mut restored_tabs {
            if let SerializedPaneLayout::Single { uuid, .. } = &tab.root_layout {
                if split_members.contains(uuid) {
                    tab.hidden_from_sidebar = true;
                }
            }
        }
    }

    let block_rows: Vec<(
        Vec<u8>,
        String,
        String,
        String,
        String,
        Option<i32>,
        Option<i64>,
        Option<i64>,
        i32,
        i32,
        i32,
        i64,
    )> = blocks::table
        .select((
            blocks::pane_uuid,
            blocks::command,
            blocks::cwd,
            blocks::output_json,
            blocks::status,
            blocks::exit_code,
            blocks::started_at_unix_ms,
            blocks::duration_ms,
            blocks::expanded,
            blocks::bookmarked,
            blocks::sort_order,
            blocks::created_at_unix_ms,
        ))
        .order((blocks::pane_uuid.asc(), blocks::sort_order.asc()))
        .load(conn)
        .context("load blocks")?;

    let mut block_lists: RestoredBlockLists = HashMap::new();
    for (
        pane_uuid_bytes,
        command,
        cwd,
        output_json,
        status,
        exit_code,
        started_at_unix_ms,
        duration_ms,
        expanded,
        bookmarked,
        _sort,
        created_at_unix_ms,
    ) in block_rows
    {
        let uuid = bytes_to_uuid(&pane_uuid_bytes)?;
        let lines = serde_json::from_str(&output_json).unwrap_or_default();
        let entry = block_lists.entry(uuid).or_default();
        entry.push(crate::types::PersistedBlock {
            command,
            cwd: cwd.into(),
            lines,
            status: match status.as_str() {
                "running" => crate::types::PersistedRunStatus::Running,
                "error" => crate::types::PersistedRunStatus::Error,
                _ => crate::types::PersistedRunStatus::Success,
            },
            exit_code,
            started_at_unix_ms,
            duration_ms: duration_ms.map(|v| v as u64),
            expanded: expanded != 0,
            bookmarked: bookmarked != 0,
            created_at_unix_ms,
        });
    }

    // ── Load agent blocks ──────────────────────────────────────────────────
    let agent_block_rows: Vec<(Vec<u8>, Vec<u8>, String, i32, i64)> = agent_blocks::table
        .select((
            agent_blocks::pane_uuid,
            agent_blocks::conversation_id,
            agent_blocks::kind_json,
            agent_blocks::sort_order,
            agent_blocks::created_at_unix_ms,
        ))
        .order((
            agent_blocks::pane_uuid.asc(),
            agent_blocks::sort_order.asc(),
        ))
        .load(conn)
        .context("load agent blocks")?;

    let mut agent_block_lists: RestoredAgentBlockLists = HashMap::new();
    for (pane_uuid_bytes, conv_id_bytes, kind_json, _sort, created_at_unix_ms) in agent_block_rows {
        let uuid = bytes_to_uuid(&pane_uuid_bytes)?;
        let conversation_id = bytes_to_uuid(&conv_id_bytes)?;
        let entry = agent_block_lists.entry(uuid).or_default();
        entry.push(PersistedAgentBlock {
            kind_json,
            conversation_id,
            created_at_unix_ms,
        });
    }
    // Restore only each pane's LATEST conversation — older ones are preserved
    // in SQLite (not deleted here) but are reachable only through the session
    // history browser (`conversation_summaries_for_pane`/`load_conversation`),
    // not auto-restored on every launch. Rows above are already ordered by
    // sort_order asc, so a pane's last element is its most recent block.
    for blocks in agent_block_lists.values_mut() {
        if let Some(latest_conversation_id) = blocks.last().map(|b| b.conversation_id) {
            blocks.retain(|b| b.conversation_id == latest_conversation_id);
        }
    }

    let active_tab_id = app_record.0 as usize;
    // If the previously-active tab was dropped, fall back to the first remaining one.
    let active_tab_id = if restored_tabs.iter().any(|t| t.id == active_tab_id) {
        active_tab_id
    } else {
        restored_tabs.first().map(|tab| tab.id).unwrap_or(0)
    };

    let active_pane_uuid = app_record.1.as_deref().map(bytes_to_uuid).transpose()?;
    let layouts: dragent_workspace_model::LayoutCollection = app_record
        .6
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .context("decode workspace layout instances")?
        .unwrap_or_default();
    layouts
        .validate()
        .context("validate restored workspace layout instances")?;

    let workspace_work_items = load_recent_workspace_work_items(conn)?;
    let mut canonical_workspace_events = Vec::new();
    for work_item in &workspace_work_items {
        canonical_workspace_events.extend(load_canonical_workspace_events(
            conn,
            work_item.project_id,
            work_item.id,
            0,
            usize::MAX,
        )?);
    }

    Ok(Some(PersistedSession {
        session: SerializedSession {
            tabs: restored_tabs,
            active_tab_id,
            active_pane_uuid,
            chrome: WindowChrome {
                left_panel_open: app_record.2 != 0,
                left_panel_width: app_record.3,
                right_panel_open: app_record.4 != 0,
                right_panel_width: app_record.5,
            },
            document,
            layouts,
        },
        blocks: block_lists,
        agent_blocks: agent_block_lists,
        working_copies: load_working_copies(conn)?,
        workspace_work_items,
        workspace_events: load_recent_workspace_events(conn)?,
        canonical_workspace_events,
    }))
}

fn read_layout_tree(
    conn: &mut SqliteConnection,
    tab_row_id: i32,
    current_direction_convention: bool,
) -> Result<SerializedPaneLayout> {
    let root_id: Option<i32> = pane_nodes::table
        .filter(pane_nodes::tab_id.eq(tab_row_id))
        .filter(pane_nodes::parent_id.is_null())
        .select(pane_nodes::id)
        .first(conn)
        .optional()
        .context("find root pane node")?;

    let Some(root_id) = root_id else {
        anyhow::bail!("tab {tab_row_id} has no root pane node");
    };

    read_node(conn, root_id, current_direction_convention)
}

fn read_node(
    conn: &mut SqliteConnection,
    node_id: i32,
    current_direction_convention: bool,
) -> Result<SerializedPaneLayout> {
    let (is_leaf, split_direction, _flex_size): (i32, Option<String>, Option<f32>) =
        pane_nodes::table
            .filter(pane_nodes::id.eq(node_id))
            .select((
                pane_nodes::is_leaf,
                pane_nodes::split_direction,
                pane_nodes::flex_size,
            ))
            .first(conn)
            .context("read pane node")?;

    if is_leaf != 0 {
        let (
            uuid_bytes,
            cwd,
            shell_title,
            _is_active,
            clear_gap_h,
            clear_gap_before,
            clear_gap_after,
            agent_id,
            browser_url,
        ): (
            Vec<u8>,
            String,
            Option<String>,
            i32,
            f32,
            Option<i32>,
            i32,
            Option<String>,
            Option<String>,
        ) = terminal_panes::table
            .filter(terminal_panes::pane_node_id.eq(node_id))
            .select((
                terminal_panes::uuid,
                terminal_panes::cwd,
                terminal_panes::shell_title,
                terminal_panes::is_active,
                terminal_panes::clear_gap_height_px,
                terminal_panes::clear_gap_before_block_id,
                terminal_panes::clear_gap_after_all,
                terminal_panes::agent_id,
                terminal_panes::browser_url,
            ))
            .first(conn)
            .context("read terminal pane")?;
        return Ok(SerializedPaneLayout::Single {
            uuid: bytes_to_uuid(&uuid_bytes)?,
            cwd: cwd.into(),
            shell_title,
            is_active: _is_active != 0,
            clear_gap_height_px: clear_gap_h,
            clear_gap_before_block_id: clear_gap_before,
            clear_gap_after_all: clear_gap_after != 0,
            agent_id,
            browser_url,
        });
    }

    let children: Vec<(i32, Option<f32>)> = pane_nodes::table
        .filter(pane_nodes::parent_id.eq(node_id))
        .select((pane_nodes::id, pane_nodes::flex_size))
        .order(pane_nodes::id.asc())
        .load(conn)
        .context("read split children")?;

    if children.len() != 2 {
        anyhow::bail!(
            "split node {node_id} expected 2 children, got {}",
            children.len()
        );
    }

    // Version 0 used the inverse terminology. Version 1 stores the engine's
    // current convention directly, so subsequent restarts never flip it.
    let direction = match (current_direction_convention, split_direction.as_deref()) {
        (true, Some("horizontal")) => SerializedSplitDirection::Horizontal,
        (true, _) => SerializedSplitDirection::Vertical,
        (false, Some("vertical")) => SerializedSplitDirection::Horizontal,
        (false, _) => SerializedSplitDirection::Vertical,
    };

    Ok(SerializedPaneLayout::Split {
        direction,
        left_or_top: Box::new(read_node(
            conn,
            children[0].0,
            current_direction_convention,
        )?),
        right_or_bottom: Box::new(read_node(
            conn,
            children[1].0,
            current_direction_convention,
        )?),
        // A split's divider position is stored on its first child. Reading the
        // parent node's flex size (normally NULL) silently reset every divider
        // to 350px after restart.
        size: children[0].1.unwrap_or(350.0),
    })
}

pub fn save_session_snapshot(
    conn: &mut SqliteConnection,
    session: &SerializedSession,
) -> Result<()> {
    anyhow::ensure!(
        session
            .document
            .belongs_to_organisation(&crate::database::connection_organisation(conn)?),
        "cannot save a workspace snapshot owned by another organisation"
    );
    conn.transaction(|conn| {
        diesel::delete(pane_nodes::table).execute(conn)?;
        diesel::delete(tabs::table).execute(conn)?;

        for (sort_index, tab) in session.tabs.iter().enumerate() {
            diesel::insert_into(tabs::table)
                .values((
                    tabs::logical_id.eq(tab.id as i32),
                    tabs::sort_index.eq(sort_index as i32),
                    tabs::title.eq(&tab.title),
                    tabs::custom_label.eq(&tab.custom_label),
                    tabs::color.eq(&tab.color),
                    tabs::hidden_from_sidebar.eq(tab.hidden_from_sidebar as i32),
                    tabs::owner_pane_uuid
                        .eq(tab.owner_pane_uuid.map(|uuid| uuid.as_bytes().to_vec())),
                ))
                .execute(conn)?;

            let tab_row_id: i32 = tabs::table
                .filter(tabs::logical_id.eq(tab.id as i32))
                .select(tabs::id)
                .first(conn)?;

            write_layout_tree(conn, tab_row_id, &tab.root_layout, None, None)?;
        }

        let active_pane_bytes = session.active_pane_uuid.map(|u| u.as_bytes().to_vec());

        session
            .layouts
            .validate()
            .context("validate workspace layout instances")?;
        let layout_json = if session.layouts.instances.is_empty() {
            None
        } else {
            Some(
                serde_json::to_string(&session.layouts)
                    .context("encode workspace layout instances")?,
            )
        };

        diesel::update(app::table.filter(app::id.eq(1)))
            .set((
                app::active_tab_id.eq(session.active_tab_id as i32),
                app::active_pane_uuid.eq(active_pane_bytes),
                app::left_panel_open.eq(session.chrome.left_panel_open as i32),
                app::left_panel_width.eq(session.chrome.left_panel_width),
                app::right_panel_open.eq(session.chrome.right_panel_open as i32),
                app::right_panel_width.eq(session.chrome.right_panel_width),
                app::layout_json.eq(layout_json),
                app::pane_layout_version.eq(1),
                app::updated_at.eq(Utc::now().to_rfc3339()),
            ))
            .execute(conn)?;

        save_document_session(conn, &session.document)?;
        crate::workspace_catalogue::record_document_workspaces(conn, &session.document)?;

        Ok(())
    })
}

pub fn save_window_frame(
    conn: &mut SqliteConnection,
    frame: &crate::types::SerializedWindowFrame,
) -> Result<()> {
    let json = serde_json::to_string(frame).context("encode window frame")?;
    diesel::update(app::table.filter(app::id.eq(1)))
        .set(app::window_frame_json.eq(Some(json)))
        .execute(conn)
        .context("save window frame")?;
    Ok(())
}

pub fn load_window_frame(
    conn: &mut SqliteConnection,
) -> Result<Option<crate::types::SerializedWindowFrame>> {
    let json: Option<String> = app::table
        .filter(app::id.eq(1))
        .select(app::window_frame_json)
        .first(conn)
        .optional()
        .context("read window frame")?
        .flatten();
    Ok(json.and_then(|json| serde_json::from_str(&json).ok()))
}

fn write_layout_tree(
    conn: &mut SqliteConnection,
    tab_row_id: i32,
    layout: &SerializedPaneLayout,
    parent_id: Option<i32>,
    flex_size: Option<f32>,
) -> Result<()> {
    match layout {
        SerializedPaneLayout::Single {
            uuid,
            cwd,
            shell_title,
            is_active,
            clear_gap_height_px,
            clear_gap_before_block_id,
            clear_gap_after_all,
            agent_id,
            browser_url,
        } => {
            diesel::insert_into(pane_nodes::table)
                .values((
                    pane_nodes::tab_id.eq(tab_row_id),
                    pane_nodes::parent_id.eq(parent_id),
                    pane_nodes::is_leaf.eq(1),
                    pane_nodes::split_direction.eq(None::<String>),
                    pane_nodes::flex_size.eq(flex_size),
                ))
                .execute(conn)?;

            let node_id: i32 = diesel::select(diesel::dsl::sql::<diesel::sql_types::Integer>(
                "last_insert_rowid()",
            ))
            .get_result(conn)?;

            diesel::insert_into(terminal_panes::table)
                .values((
                    terminal_panes::pane_node_id.eq(node_id),
                    terminal_panes::uuid.eq(uuid.as_bytes().to_vec()),
                    terminal_panes::cwd.eq(cwd.to_string_lossy().to_string()),
                    terminal_panes::shell_title.eq(shell_title),
                    terminal_panes::is_active.eq(*is_active as i32),
                    terminal_panes::clear_gap_height_px.eq(*clear_gap_height_px),
                    terminal_panes::clear_gap_before_block_id.eq(*clear_gap_before_block_id),
                    terminal_panes::clear_gap_after_all.eq(*clear_gap_after_all as i32),
                    terminal_panes::agent_id.eq(agent_id.clone()),
                    terminal_panes::browser_url.eq(browser_url.clone()),
                ))
                .execute(conn)?;
        }
        SerializedPaneLayout::Split {
            direction,
            left_or_top,
            right_or_bottom,
            size,
        } => {
            let split_direction = match direction {
                SerializedSplitDirection::Horizontal => "horizontal",
                SerializedSplitDirection::Vertical => "vertical",
            };

            diesel::insert_into(pane_nodes::table)
                .values((
                    pane_nodes::tab_id.eq(tab_row_id),
                    pane_nodes::parent_id.eq(parent_id),
                    pane_nodes::is_leaf.eq(0),
                    pane_nodes::split_direction.eq(split_direction),
                    pane_nodes::flex_size.eq(flex_size),
                ))
                .execute(conn)?;

            let node_id: i32 = diesel::select(diesel::dsl::sql::<diesel::sql_types::Integer>(
                "last_insert_rowid()",
            ))
            .get_result(conn)?;

            write_layout_tree(conn, tab_row_id, left_or_top, Some(node_id), Some(*size))?;
            write_layout_tree(conn, tab_row_id, right_or_bottom, Some(node_id), None)?;
        }
    }
    Ok(())
}

pub fn save_block(
    conn: &mut SqliteConnection,
    pane_uuid: Uuid,
    block: &crate::types::PersistedBlock,
    sort_order: i32,
) -> Result<()> {
    let output_json = serde_json::to_string(&block.lines).context("serialize block output")?;
    let status = match block.status {
        crate::types::PersistedRunStatus::Running => "running",
        crate::types::PersistedRunStatus::Success => "success",
        crate::types::PersistedRunStatus::Error => "error",
    };

    diesel::insert_into(blocks::table)
        .values((
            blocks::pane_uuid.eq(pane_uuid.as_bytes().to_vec()),
            blocks::command.eq(&block.command),
            blocks::cwd.eq(block.cwd.to_string_lossy().to_string()),
            blocks::output_json.eq(output_json),
            blocks::status.eq(status),
            blocks::exit_code.eq(block.exit_code),
            blocks::started_at_unix_ms.eq(block.started_at_unix_ms),
            blocks::duration_ms.eq(block.duration_ms.map(|v| v as i64)),
            blocks::expanded.eq(block.expanded as i32),
            blocks::bookmarked.eq(block.bookmarked as i32),
            blocks::sort_order.eq(sort_order),
            blocks::created_at_unix_ms.eq(block.created_at_unix_ms),
        ))
        .execute(conn)?;
    Ok(())
}

pub fn delete_blocks_for_pane(conn: &mut SqliteConnection, pane_uuid: Uuid) -> Result<()> {
    let uuid_bytes = pane_uuid.as_bytes().to_vec();
    diesel::delete(blocks::table.filter(blocks::pane_uuid.eq(uuid_bytes))).execute(conn)?;
    Ok(())
}

pub fn trim_blocks_for_pane(
    conn: &mut SqliteConnection,
    pane_uuid: Uuid,
    max_blocks: i64,
) -> Result<()> {
    let uuid_bytes = pane_uuid.as_bytes().to_vec();
    let ids: Vec<i32> = blocks::table
        .filter(blocks::pane_uuid.eq(&uuid_bytes))
        .order(blocks::sort_order.desc())
        .select(blocks::id)
        .load(conn)?;

    if ids.len() as i64 <= max_blocks {
        return Ok(());
    }

    let to_delete: Vec<i32> = ids.into_iter().skip(max_blocks as usize).collect();
    diesel::delete(blocks::table.filter(blocks::id.eq_any(to_delete))).execute(conn)?;
    Ok(())
}

pub fn import_legacy_json(conn: &mut SqliteConnection, json: &str) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct LegacyTab {
        id: usize,
        title: String,
        custom_label: Option<String>,
        color: Option<String>,
        root_layout: LegacyLayout,
    }

    #[derive(serde::Deserialize)]
    enum LegacyLayout {
        Single {
            cwd: std::path::PathBuf,
        },
        Split {
            direction: SerializedSplitDirection,
            left_or_top: Box<LegacyLayout>,
            right_or_bottom: Box<LegacyLayout>,
            size: f32,
        },
    }

    #[derive(serde::Deserialize)]
    struct LegacySession {
        tabs: Vec<LegacyTab>,
        active_tab_id: usize,
    }

    let legacy: LegacySession = serde_json::from_str(json).context("parse legacy session.json")?;

    fn convert(layout: LegacyLayout) -> SerializedPaneLayout {
        match layout {
            LegacyLayout::Single { cwd } => SerializedPaneLayout::Single {
                uuid: Uuid::new_v4(),
                cwd,
                shell_title: None,
                is_active: false,
                clear_gap_height_px: 0.0,
                clear_gap_before_block_id: None,
                clear_gap_after_all: false,
                agent_id: None,
                browser_url: None,
            },
            LegacyLayout::Split {
                direction,
                left_or_top,
                right_or_bottom,
                size,
            } => SerializedPaneLayout::Split {
                direction,
                left_or_top: Box::new(convert(*left_or_top)),
                right_or_bottom: Box::new(convert(*right_or_bottom)),
                size,
            },
        }
    }

    let session = SerializedSession {
        tabs: legacy
            .tabs
            .into_iter()
            .map(|t| SerializedTab {
                id: t.id,
                title: t.title,
                custom_label: t.custom_label,
                color: t.color,
                root_layout: convert(t.root_layout),
                hidden_from_sidebar: false,
                owner_pane_uuid: None,
            })
            .collect(),
        active_tab_id: legacy.active_tab_id,
        active_pane_uuid: None,
        chrome: WindowChrome::default(),
        document: Default::default(),
        layouts: Default::default(),
    };

    save_session_snapshot(conn, &session)
}

fn bytes_to_uuid(bytes: &[u8]) -> Result<Uuid> {
    let arr: [u8; 16] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid uuid bytes length {}", bytes.len()))?;
    Ok(Uuid::from_bytes(arr))
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use dragent_workspace_model::{
        LayoutChromeState, LayoutCollection, LayoutInstance, LayoutScope, Project, SurfaceId,
        SurfaceLayoutNode,
    };
    use tempfile::tempdir;

    fn session_with_layout() -> SerializedSession {
        let pane_id = Uuid::new_v4();
        let surface_id = SurfaceId::from_terminal_pane(pane_id);
        let project = Project::new(std::path::PathBuf::from("/tmp/layout-project"), 0);
        let layout = LayoutInstance::migrate_develop(
            LayoutScope::Project {
                project_id: project.id,
            },
            SurfaceLayoutNode::single(surface_id),
            vec![surface_id],
            LayoutChromeState::default(),
            Some(surface_id),
            10,
        )
        .unwrap();
        let mut layouts = LayoutCollection::default();
        layouts.upsert_and_activate(layout).unwrap();

        SerializedSession {
            tabs: vec![SerializedTab {
                id: 7,
                title: "Terminal".to_string(),
                custom_label: None,
                color: None,
                root_layout: SerializedPaneLayout::Single {
                    uuid: pane_id,
                    cwd: project.root,
                    shell_title: None,
                    is_active: true,
                    clear_gap_height_px: 0.0,
                    clear_gap_before_block_id: None,
                    clear_gap_after_all: false,
                    agent_id: None,
                    browser_url: None,
                },
                hidden_from_sidebar: false,
                owner_pane_uuid: Some(pane_id),
            }],
            active_tab_id: 7,
            active_pane_uuid: Some(pane_id),
            chrome: WindowChrome::default(),
            document: Default::default(),
            layouts,
        }
    }

    #[test]
    fn layout_instances_survive_sqlite_session_round_trip() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("layout.sqlite");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let expected = session_with_layout();

        save_session_snapshot(&mut connection, &expected).unwrap();
        let restored = load_session(&mut connection)
            .unwrap()
            .expect("saved session should restore")
            .session;

        assert_eq!(restored.layouts, expected.layouts);
        assert_eq!(restored.active_pane_uuid, expected.active_pane_uuid);
        assert_eq!(restored.tabs.len(), 1);
    }

    #[test]
    fn project_prompt_without_tabs_survives_session_round_trip() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("project-prompt.sqlite");
        let project = directory.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let mut expected = session_with_layout();
        expected.tabs.clear();
        expected.active_tab_id = 0;
        expected.active_pane_uuid = None;
        expected.layouts = LayoutCollection::default();
        expected.document.open_project_roots = vec![project.clone()];
        expected.document.project_start_visible = true;
        expected.document.prompt_text = "Continue the unfinished task".into();
        expected.document.prompt_agent = Some("Codex".into());

        save_session_snapshot(&mut connection, &expected).unwrap();
        let restored = load_session(&mut connection)
            .unwrap()
            .expect("selected project should restore without a terminal")
            .session;
        assert!(restored.tabs.is_empty());
        assert_eq!(restored.document.open_project_roots, vec![project]);
        assert!(restored.document.project_start_visible);
        assert_eq!(
            restored.document.prompt_text,
            "Continue the unfinished task"
        );
        assert_eq!(restored.document.prompt_agent.as_deref(), Some("Codex"));
    }

    #[test]
    fn project_prompt_remains_active_with_existing_tabs() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("prompt-with-tabs.sqlite");
        let project = directory.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let mut expected = session_with_layout();
        expected.document.open_project_roots = vec![project];
        expected.document.project_start_visible = true;

        save_session_snapshot(&mut connection, &expected).unwrap();
        let restored = load_session(&mut connection).unwrap().unwrap().session;
        assert_eq!(restored.tabs.len(), 1);
        assert!(restored.document.project_start_visible);
    }

    #[test]
    fn split_direction_size_identity_and_sidebar_visibility_round_trip() {
        fn pane(uuid: Uuid, active: bool) -> SerializedPaneLayout {
            SerializedPaneLayout::Single {
                uuid,
                cwd: std::path::PathBuf::from("/tmp/layout-project"),
                shell_title: None,
                is_active: active,
                clear_gap_height_px: 0.0,
                clear_gap_before_block_id: None,
                clear_gap_after_all: false,
                agent_id: None,
                browser_url: None,
            }
        }

        let directory = tempdir().unwrap();
        let path = directory.path().join("split-layout.sqlite");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let left = Uuid::new_v4();
        let right = Uuid::new_v4();
        let mut session = session_with_layout();
        session.tabs = vec![
            SerializedTab {
                id: 7,
                title: "Primary".into(),
                custom_label: Some("Primary".into()),
                color: None,
                root_layout: SerializedPaneLayout::Split {
                    direction: SerializedSplitDirection::Horizontal,
                    left_or_top: Box::new(pane(left, false)),
                    right_or_bottom: Box::new(pane(right, true)),
                    size: 420.0,
                },
                hidden_from_sidebar: false,
                owner_pane_uuid: Some(left),
            },
            SerializedTab {
                id: 8,
                title: "Split".into(),
                custom_label: None,
                color: None,
                root_layout: pane(right, true),
                hidden_from_sidebar: true,
                owner_pane_uuid: Some(right),
            },
        ];
        session.active_tab_id = 8;
        session.active_pane_uuid = Some(right);

        save_session_snapshot(&mut connection, &session).unwrap();
        let restored = load_session(&mut connection)
            .unwrap()
            .expect("saved split should restore")
            .session;

        assert_eq!(restored.tabs.len(), 2);
        assert_eq!(restored.tabs[0].owner_pane_uuid, Some(left));
        assert!(!restored.tabs[0].hidden_from_sidebar);
        assert_eq!(restored.tabs[1].owner_pane_uuid, Some(right));
        assert!(restored.tabs[1].hidden_from_sidebar);
        match &restored.tabs[0].root_layout {
            SerializedPaneLayout::Split {
                direction, size, ..
            } => {
                assert_eq!(*direction, SerializedSplitDirection::Horizontal);
                assert!((*size - 420.0).abs() < f32::EPSILON);
            }
            SerializedPaneLayout::Single { .. } => panic!("split collapsed during round trip"),
        }
    }

    #[test]
    fn empty_legacy_layout_state_remains_backward_compatible() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("legacy-layout.sqlite");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let mut session = session_with_layout();
        session.layouts = LayoutCollection::default();

        save_session_snapshot(&mut connection, &session).unwrap();
        let restored = load_session(&mut connection)
            .unwrap()
            .expect("legacy-shaped session should restore")
            .session;

        assert!(restored.layouts.instances.is_empty());
        assert_eq!(restored.tabs[0].id, session.tabs[0].id);
    }

    #[test]
    fn browser_panes_keep_their_address_beside_terminals() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("browser-layout.sqlite");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let terminal = Uuid::new_v4();
        let browser = Uuid::new_v4();
        let mut session = session_with_layout();
        let mut browser_leaf = leaf(browser, None);
        if let SerializedPaneLayout::Single { browser_url, .. } = &mut browser_leaf {
            *browser_url = Some("http://localhost:5173/dashboard".into());
        }
        session.tabs = vec![SerializedTab {
            id: 3,
            title: "Preview".into(),
            custom_label: None,
            color: None,
            root_layout: SerializedPaneLayout::Split {
                direction: SerializedSplitDirection::Horizontal,
                left_or_top: Box::new(leaf(terminal, Some("claude"))),
                right_or_bottom: Box::new(browser_leaf),
                size: 0.5,
            },
            hidden_from_sidebar: false,
            owner_pane_uuid: Some(terminal),
        }];
        session.active_tab_id = 3;

        save_session_snapshot(&mut connection, &session).unwrap();
        let restored = load_session(&mut connection).unwrap().unwrap().session;
        let SerializedPaneLayout::Split {
            left_or_top,
            right_or_bottom,
            ..
        } = &restored.tabs[0].root_layout
        else {
            panic!("split should restore");
        };
        assert!(matches!(
            left_or_top.as_ref(),
            SerializedPaneLayout::Single { uuid, browser_url: None, .. } if *uuid == terminal
        ));
        assert!(matches!(
            right_or_bottom.as_ref(),
            SerializedPaneLayout::Single { uuid, browser_url: Some(url), .. }
                if *uuid == browser && url == "http://localhost:5173/dashboard"
        ));
    }

    fn leaf(uuid: Uuid, agent_id: Option<&str>) -> SerializedPaneLayout {
        SerializedPaneLayout::Single {
            uuid,
            cwd: std::path::PathBuf::from("/tmp/worktree-b"),
            shell_title: None,
            is_active: false,
            clear_gap_height_px: 0.0,
            clear_gap_before_block_id: None,
            clear_gap_after_all: false,
            agent_id: agent_id.map(str::to_string),
            browser_url: None,
        }
    }

    #[test]
    fn parked_workspace_tabs_restore_their_panes_splits_and_sessions() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("parked.sqlite");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let (claude, shell) = (Uuid::new_v4(), Uuid::new_v4());
        let mut session = session_with_layout();
        session.document.parked_workspace_tabs = vec![crate::types::SerializedWorkspaceTab {
            id: 4,
            name: "worktree-b".into(),
            tabs: vec![SerializedTab {
                id: 11,
                title: "Claude".into(),
                custom_label: None,
                color: None,
                root_layout: SerializedPaneLayout::Split {
                    direction: SerializedSplitDirection::Horizontal,
                    left_or_top: Box::new(leaf(claude, Some("claude"))),
                    right_or_bottom: Box::new(leaf(shell, None)),
                    size: 612_500.0,
                },
                hidden_from_sidebar: false,
                owner_pane_uuid: Some(claude),
            }],
            active_tab_id: 11,
            active_pane_uuid: Some(shell),
        }];
        session.document.workspace_tab_order = vec![4, 1];
        session.document.active_workspace_tab_id = Some(1);
        session.document.host_terminal_bindings = [claude, shell]
            .into_iter()
            .map(|pane_uuid| crate::types::SerializedHostTerminalBinding {
                remote_host: None,
                pane_uuid,
                workspace_id: "ws".into(),
                home_id: "home".into(),
                session_id: format!("session-{pane_uuid}"),
                agent: pane_uuid == claude,
            })
            .collect();

        save_session_snapshot(&mut connection, &session).unwrap();
        let document = load_session(&mut connection)
            .unwrap()
            .unwrap()
            .session
            .document;

        assert_eq!(document.workspace_tab_order, vec![4, 1]);
        assert_eq!(document.active_workspace_tab_id, Some(1));
        let parked = &document.parked_workspace_tabs[0];
        assert_eq!((parked.id, parked.name.as_str()), (4, "worktree-b"));
        assert_eq!(parked.active_pane_uuid, Some(shell));
        let SerializedPaneLayout::Split {
            left_or_top, size, ..
        } = &parked.tabs[0].root_layout
        else {
            panic!("split must survive");
        };
        assert_eq!(*size, 612_500.0);
        assert!(matches!(
            left_or_top.as_ref(),
            SerializedPaneLayout::Single { uuid, agent_id: Some(agent), .. }
                if *uuid == claude && agent == "claude"
        ));
        let claude_binding = document
            .host_terminal_bindings
            .iter()
            .find(|binding| binding.pane_uuid == claude)
            .unwrap();
        assert_eq!(claude_binding.session_id, format!("session-{claude}"));
        assert!(claude_binding.agent);
    }

    #[test]
    fn window_frame_is_not_overwritten_by_later_snapshots() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("frame.sqlite");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        assert_eq!(load_window_frame(&mut connection).unwrap(), None);
        let frame = crate::types::SerializedWindowFrame {
            x: 120.0,
            y: 64.0,
            width: 1440.0,
            height: 900.0,
            fullscreen_state: 0,
        };

        save_window_frame(&mut connection, &frame).unwrap();
        save_session_snapshot(&mut connection, &session_with_layout()).unwrap();

        assert_eq!(load_window_frame(&mut connection).unwrap(), Some(frame));
    }

    #[test]
    fn sessions_saved_before_workspace_tabs_persisted_still_load() {
        let decoded: crate::types::SerializedDocumentSession =
            serde_json::from_str(r#"{"root":"/tmp","dock_terminals":[]}"#).unwrap();
        assert!(decoded.parked_workspace_tabs.is_empty());
        assert!(decoded.workspace_tab_order.is_empty());
        assert_eq!(decoded.active_workspace_tab_id, None);
    }
}

// ── Agent block persistence ───────────────────────────────────────────────────

pub fn save_agent_block(
    conn: &mut SqliteConnection,
    pane_uuid: Uuid,
    block: &PersistedAgentBlock,
    sort_order: i32,
) -> Result<()> {
    use diesel::prelude::*;
    let uuid_bytes = pane_uuid.as_bytes().to_vec();

    let existing_id: Option<i32> = agent_blocks::table
        .filter(agent_blocks::pane_uuid.eq(&uuid_bytes))
        .filter(agent_blocks::sort_order.eq(sort_order))
        .select(agent_blocks::id)
        .first::<i32>(conn)
        .optional()?;

    if let Some(id_val) = existing_id {
        diesel::update(agent_blocks::table.filter(agent_blocks::id.eq(id_val)))
            .set((
                agent_blocks::conversation_id.eq(block.conversation_id.as_bytes().to_vec()),
                agent_blocks::kind_json.eq(&block.kind_json),
            ))
            .execute(conn)?;
    } else {
        diesel::insert_into(agent_blocks::table)
            .values((
                agent_blocks::pane_uuid.eq(uuid_bytes),
                agent_blocks::conversation_id.eq(block.conversation_id.as_bytes().to_vec()),
                agent_blocks::kind_json.eq(&block.kind_json),
                agent_blocks::sort_order.eq(sort_order),
                agent_blocks::created_at_unix_ms.eq(block.created_at_unix_ms),
            ))
            .execute(conn)?;
    }
    Ok(())
}

pub fn delete_agent_blocks_for_pane(conn: &mut SqliteConnection, pane_uuid: Uuid) -> Result<()> {
    let uuid_bytes = pane_uuid.as_bytes().to_vec();
    diesel::delete(agent_blocks::table.filter(agent_blocks::pane_uuid.eq(uuid_bytes)))
        .execute(conn)?;
    Ok(())
}

/// Trim agent blocks to the most recent `max_blocks` for `pane_uuid`.
pub fn trim_agent_blocks_for_pane(
    conn: &mut SqliteConnection,
    pane_uuid: Uuid,
    max_blocks: i64,
) -> Result<()> {
    let uuid_bytes = pane_uuid.as_bytes().to_vec();
    let ids: Vec<i32> = agent_blocks::table
        .filter(agent_blocks::pane_uuid.eq(&uuid_bytes))
        .order(agent_blocks::sort_order.desc())
        .select(agent_blocks::id)
        .load(conn)?;

    if ids.len() as i64 <= max_blocks {
        return Ok(());
    }

    let to_delete: Vec<i32> = ids.into_iter().skip(max_blocks as usize).collect();
    diesel::delete(agent_blocks::table.filter(agent_blocks::id.eq_any(to_delete))).execute(conn)?;
    Ok(())
}

/// The highest `sort_order` used by any agent block for `pane_uuid`, across
/// every conversation (including ones not currently loaded into memory).
/// Callers that renumber a freshly-loaded block set must bump their next-id
/// counter above this, or a future save can silently collide with (and
/// overwrite) an unloaded older conversation's row — see the session-history
/// design notes for why this matters.
pub fn max_sort_order_for_pane(
    conn: &mut SqliteConnection,
    pane_uuid: Uuid,
) -> Result<Option<i32>> {
    let uuid_bytes = pane_uuid.as_bytes().to_vec();
    let max: Option<i32> = agent_blocks::table
        .filter(agent_blocks::pane_uuid.eq(&uuid_bytes))
        .select(diesel::dsl::max(agent_blocks::sort_order))
        .first(conn)
        .context("max sort_order for pane")?;
    Ok(max)
}

/// All agent blocks for one specific past conversation in a pane, ordered
/// oldest-first — the counterpart to `load_session`'s "latest conversation
/// only" restore, used to resume a conversation picked from the session
/// history browser.
pub fn load_conversation(
    conn: &mut SqliteConnection,
    pane_uuid: Uuid,
    conversation_id: Uuid,
) -> Result<Vec<PersistedAgentBlock>> {
    let pane_bytes = pane_uuid.as_bytes().to_vec();
    let conv_bytes = conversation_id.as_bytes().to_vec();
    let rows: Vec<(String, i64)> = agent_blocks::table
        .filter(agent_blocks::pane_uuid.eq(&pane_bytes))
        .filter(agent_blocks::conversation_id.eq(&conv_bytes))
        .order(agent_blocks::sort_order.asc())
        .select((agent_blocks::kind_json, agent_blocks::created_at_unix_ms))
        .load(conn)
        .context("load conversation")?;
    Ok(rows
        .into_iter()
        .map(|(kind_json, created_at_unix_ms)| PersistedAgentBlock {
            kind_json,
            conversation_id,
            created_at_unix_ms,
        })
        .collect())
}

/// Summaries of past conversations for a pane, most-recently-active first,
/// capped at `limit` — backs the session history browser's list.
/// Persist one command into the GLOBAL, never-cleared `command_history` table
/// (the ↑/↓ popover's source). Upserts: re-running a command bumps its timestamp
/// and use-count rather than adding a duplicate row.
pub fn save_command_history(
    conn: &mut SqliteConnection,
    command: &str,
    at_unix_ms: i64,
) -> Result<()> {
    use crate::schema::command_history::dsl;
    let command = command.trim();
    if command.is_empty() {
        return Ok(());
    }
    diesel::insert_into(dsl::command_history)
        .values((
            dsl::command.eq(command),
            dsl::last_used_at_unix_ms.eq(at_unix_ms),
            dsl::use_count.eq(1),
        ))
        .on_conflict(dsl::command)
        .do_update()
        .set((
            dsl::last_used_at_unix_ms.eq(at_unix_ms),
            dsl::use_count.eq(dsl::use_count + 1),
        ))
        .execute(conn)
        .context("save command history")?;
    Ok(())
}

/// Recent shell commands for the ↑/↓ history popover, most recent first. Reads
/// the dedicated `command_history` table (global, never wiped on close — unlike
/// `blocks`). Rows are already unique per command.
pub fn recent_commands(conn: &mut SqliteConnection, limit: usize) -> Result<Vec<(String, i64)>> {
    use crate::schema::command_history::dsl;
    dsl::command_history
        .select((dsl::command, dsl::last_used_at_unix_ms))
        .order(dsl::last_used_at_unix_ms.desc())
        .limit(limit as i64)
        .load(conn)
        .context("load recent commands")
}

pub fn conversation_summaries_for_pane(
    conn: &mut SqliteConnection,
    pane_uuid: Uuid,
    limit: i64,
) -> Result<Vec<crate::types::ConversationSummary>> {
    use crate::types::ConversationSummary;

    let uuid_bytes = pane_uuid.as_bytes().to_vec();
    let rows: Vec<(Vec<u8>, String, i64)> = agent_blocks::table
        .filter(agent_blocks::pane_uuid.eq(&uuid_bytes))
        .order(agent_blocks::sort_order.asc())
        .select((
            agent_blocks::conversation_id,
            agent_blocks::kind_json,
            agent_blocks::created_at_unix_ms,
        ))
        .load(conn)
        .context("load agent blocks for conversation summaries")?;

    // Group in memory — rows are already sort_order-ascending, so the first
    // occurrence of a conversation_id is its earliest block (title source)
    // and the last occurrence's timestamp is its most recent activity.
    let mut by_conv: HashMap<Uuid, ConversationSummary> = HashMap::new();
    let mut order: Vec<Uuid> = Vec::new();
    for (conv_bytes, kind_json, created_at) in rows {
        let conv_id = bytes_to_uuid(&conv_bytes)?;
        match by_conv.get_mut(&conv_id) {
            Some(summary) => {
                summary.last_activity_unix_ms = created_at;
                summary.block_count += 1;
            }
            None => {
                order.push(conv_id);
                by_conv.insert(
                    conv_id,
                    ConversationSummary {
                        conversation_id: conv_id,
                        started_at_unix_ms: created_at,
                        last_activity_unix_ms: created_at,
                        block_count: 1,
                        first_block_kind_json: Some(kind_json),
                    },
                );
            }
        }
    }

    let mut summaries: Vec<ConversationSummary> = order
        .into_iter()
        .filter_map(|id| by_conv.remove(&id))
        .collect();
    summaries.sort_by(|a, b| b.last_activity_unix_ms.cmp(&a.last_activity_unix_ms));
    summaries.truncate(limit.max(0) as usize);
    Ok(summaries)
}

/// Cap the number of DISTINCT conversations retained per pane at
/// `max_conversations`, deleting every row of the oldest excess conversations
/// (by last activity) in one shot. Never trims mid-conversation — a browsable
/// past conversation is never left truncated.
pub fn trim_agent_conversations_for_pane(
    conn: &mut SqliteConnection,
    pane_uuid: Uuid,
    max_conversations: i64,
) -> Result<()> {
    let uuid_bytes = pane_uuid.as_bytes().to_vec();
    let rows: Vec<Vec<u8>> = agent_blocks::table
        .filter(agent_blocks::pane_uuid.eq(&uuid_bytes))
        .order(agent_blocks::sort_order.asc())
        .select(agent_blocks::conversation_id)
        .load(conn)
        .context("load conversation ids for trim")?;

    // Last-occurrence index per conversation_id gives last-activity order
    // without needing a GROUP BY.
    let mut last_index: HashMap<Vec<u8>, usize> = HashMap::new();
    for (idx, conv_bytes) in rows.iter().enumerate() {
        last_index.insert(conv_bytes.clone(), idx);
    }
    if last_index.len() as i64 <= max_conversations {
        return Ok(());
    }
    let mut uniq: Vec<Vec<u8>> = last_index.keys().cloned().collect();
    uniq.sort_by_key(|k| last_index[k]);

    let excess = uniq.len() - max_conversations.max(0) as usize;
    let to_delete: Vec<Vec<u8>> = uniq.into_iter().take(excess).collect();
    if to_delete.is_empty() {
        return Ok(());
    }
    diesel::delete(
        agent_blocks::table
            .filter(agent_blocks::pane_uuid.eq(&uuid_bytes))
            .filter(agent_blocks::conversation_id.eq_any(to_delete)),
    )
    .execute(conn)?;
    Ok(())
}

/// Wipe all session state from SQLite (tabs, panes, blocks, agent blocks).
/// Called synchronously on app close so the next launch starts with a clean slate.
/// JSONL transcripts on disk are left untouched — they serve as context for
/// future sessions and are never deleted here.
pub fn clear_all_session_data(conn: &mut SqliteConnection) -> Result<()> {
    conn.transaction(|conn| {
        diesel::delete(agent_blocks::table).execute(conn)?;
        diesel::delete(blocks::table).execute(conn)?;
        diesel::delete(terminal_panes::table).execute(conn)?;
        diesel::delete(pane_nodes::table).execute(conn)?;
        diesel::delete(tabs::table).execute(conn)?;
        Ok(())
    })
}

pub fn update_clear_gap(
    conn: &mut SqliteConnection,
    pane_uuid: Uuid,
    height_px: f32,
    before_block_id: Option<i32>,
    after_all: bool,
) -> Result<()> {
    let uuid_bytes = pane_uuid.as_bytes().to_vec();
    diesel::update(terminal_panes::table.filter(terminal_panes::uuid.eq(uuid_bytes)))
        .set((
            terminal_panes::clear_gap_height_px.eq(height_px),
            terminal_panes::clear_gap_before_block_id.eq(before_block_id),
            terminal_panes::clear_gap_after_all.eq(after_all as i32),
        ))
        .execute(conn)?;
    Ok(())
}

/// Load one pane's terminal blocks in `sort_order.asc` order, using the same
/// row mapping as the bulk loader in `load_session`.
pub fn load_blocks_for_pane(
    conn: &mut SqliteConnection,
    pane_uuid: Uuid,
) -> Result<Vec<crate::types::PersistedBlock>> {
    let uuid_bytes = pane_uuid.as_bytes().to_vec();
    let rows: Vec<(
        String,
        String,
        String,
        String,
        Option<i32>,
        Option<i64>,
        Option<i64>,
        i32,
        i32,
        i32,
        i64,
    )> = blocks::table
        .filter(blocks::pane_uuid.eq(&uuid_bytes))
        .order(blocks::sort_order.asc())
        .select((
            blocks::command,
            blocks::cwd,
            blocks::output_json,
            blocks::status,
            blocks::exit_code,
            blocks::started_at_unix_ms,
            blocks::duration_ms,
            blocks::expanded,
            blocks::bookmarked,
            blocks::sort_order,
            blocks::created_at_unix_ms,
        ))
        .load(conn)
        .context("load blocks for pane")?;
    Ok(rows
        .into_iter()
        .map(
            |(
                command,
                cwd,
                output_json,
                status,
                exit_code,
                started_at_unix_ms,
                duration_ms,
                expanded,
                bookmarked,
                _sort,
                created_at_unix_ms,
            )| {
                let lines = serde_json::from_str(&output_json).unwrap_or_default();
                crate::types::PersistedBlock {
                    command,
                    cwd: cwd.into(),
                    lines,
                    status: match status.as_str() {
                        "running" => crate::types::PersistedRunStatus::Running,
                        "error" => crate::types::PersistedRunStatus::Error,
                        _ => crate::types::PersistedRunStatus::Success,
                    },
                    exit_code,
                    started_at_unix_ms,
                    duration_ms: duration_ms.map(|v| v as u64),
                    expanded: expanded != 0,
                    bookmarked: bookmarked != 0,
                    created_at_unix_ms,
                }
            },
        )
        .collect())
}

/// Load one pane's agent blocks in `sort_order.asc` order, using the same
/// mapping as the bulk loader (latest conversation only).
pub fn load_agent_blocks_for_pane(
    conn: &mut SqliteConnection,
    pane_uuid: Uuid,
) -> Result<Vec<crate::types::PersistedAgentBlock>> {
    let uuid_bytes = pane_uuid.as_bytes().to_vec();
    let rows: Vec<(Vec<u8>, String, i32, i64)> = agent_blocks::table
        .filter(agent_blocks::pane_uuid.eq(&uuid_bytes))
        .order(agent_blocks::sort_order.asc())
        .select((
            agent_blocks::conversation_id,
            agent_blocks::kind_json,
            agent_blocks::sort_order,
            agent_blocks::created_at_unix_ms,
        ))
        .load(conn)
        .context("load agent blocks for pane")?;
    let mut out: Vec<crate::types::PersistedAgentBlock> = rows
        .into_iter()
        .map(|(conv_bytes, kind_json, _sort, created_at_unix_ms)| {
            let conversation_id = bytes_to_uuid(&conv_bytes).unwrap_or_else(|_| Uuid::nil());
            crate::types::PersistedAgentBlock {
                kind_json,
                conversation_id,
                created_at_unix_ms,
            }
        })
        .collect();
    if let Some(latest) = out.last().map(|b| b.conversation_id) {
        out.retain(|b| b.conversation_id == latest);
    }
    Ok(out)
}

/// Whether `pane` has any block rows (terminal or agent) in `conn`.
pub fn pane_has_rows(conn: &mut SqliteConnection, pane_uuid: Uuid) -> Result<bool> {
    let uuid_bytes = pane_uuid.as_bytes().to_vec();
    let blocks_count: i64 = blocks::table
        .filter(blocks::pane_uuid.eq(&uuid_bytes))
        .count()
        .get_result(conn)
        .unwrap_or(0);
    if blocks_count > 0 {
        return Ok(true);
    }
    let agent_count: i64 = agent_blocks::table
        .filter(agent_blocks::pane_uuid.eq(&uuid_bytes))
        .count()
        .get_result(conn)
        .unwrap_or(0);
    Ok(agent_count > 0)
}

/// Transactionally copy one pane's terminal + agent blocks from `source` to
/// `target`, re-sequencing `sort_order` values sequentially from 0.
/// Returns the total number of rows copied.
pub fn copy_pane_blocks(
    source: &mut SqliteConnection,
    target: &mut SqliteConnection,
    pane_uuid: Uuid,
) -> Result<usize> {
    let pane_bytes = pane_uuid.as_bytes().to_vec();
    let block_rows: Vec<(
        String,
        String,
        String,
        String,
        Option<i32>,
        Option<i64>,
        Option<i64>,
        i32,
        i32,
        i64,
    )> = blocks::table
        .filter(blocks::pane_uuid.eq(&pane_bytes))
        .order(blocks::sort_order.asc())
        .select((
            blocks::command,
            blocks::cwd,
            blocks::output_json,
            blocks::status,
            blocks::exit_code,
            blocks::started_at_unix_ms,
            blocks::duration_ms,
            blocks::expanded,
            blocks::bookmarked,
            blocks::created_at_unix_ms,
        ))
        .load(source)
        .context("load source blocks for copy")?;
    let agent_rows: Vec<(Vec<u8>, String, i64)> = agent_blocks::table
        .filter(agent_blocks::pane_uuid.eq(&pane_bytes))
        .order(agent_blocks::sort_order.asc())
        .select((
            agent_blocks::conversation_id,
            agent_blocks::kind_json,
            agent_blocks::created_at_unix_ms,
        ))
        .load(source)
        .context("load source agent blocks for copy")?;

    target.transaction(|target| {
        for (idx, row) in block_rows.iter().enumerate() {
            diesel::insert_into(blocks::table)
                .values((
                    blocks::pane_uuid.eq(pane_bytes.clone()),
                    blocks::command.eq(&row.0),
                    blocks::cwd.eq(&row.1),
                    blocks::output_json.eq(&row.2),
                    blocks::status.eq(&row.3),
                    blocks::exit_code.eq(row.4),
                    blocks::started_at_unix_ms.eq(row.5),
                    blocks::duration_ms.eq(row.6),
                    blocks::expanded.eq(row.7),
                    blocks::bookmarked.eq(row.8),
                    blocks::sort_order.eq(idx as i32),
                    blocks::created_at_unix_ms.eq(row.9),
                ))
                .execute(target)?;
        }
        for (idx, (conv_id, kind_json, created_at)) in agent_rows.iter().enumerate() {
            diesel::insert_into(agent_blocks::table)
                .values((
                    agent_blocks::pane_uuid.eq(pane_bytes.clone()),
                    agent_blocks::conversation_id.eq(conv_id.clone()),
                    agent_blocks::kind_json.eq(kind_json),
                    agent_blocks::sort_order.eq(idx as i32),
                    agent_blocks::created_at_unix_ms.eq(*created_at),
                ))
                .execute(target)?;
        }
        Ok(block_rows.len() + agent_rows.len())
    })
}
