diesel::table! {
    app (id) {
        id -> Integer,
        active_tab_id -> Integer,
        active_pane_uuid -> Nullable<Binary>,
        left_panel_open -> Integer,
        left_panel_width -> Nullable<Float>,
        right_panel_open -> Integer,
        right_panel_width -> Nullable<Float>,
        layout_json -> Nullable<Text>,
        pane_layout_version -> Integer,
        updated_at -> Text,
        window_frame_json -> Nullable<Text>,
    }
}

diesel::table! {
    tabs (id) {
        id -> Integer,
        logical_id -> Integer,
        sort_index -> Integer,
        title -> Text,
        custom_label -> Nullable<Text>,
        color -> Nullable<Text>,
        hidden_from_sidebar -> Integer,
        owner_pane_uuid -> Nullable<Binary>,
    }
}

diesel::table! {
    pane_nodes (id) {
        id -> Integer,
        tab_id -> Integer,
        parent_id -> Nullable<Integer>,
        is_leaf -> Integer,
        split_direction -> Nullable<Text>,
        flex_size -> Nullable<Float>,
    }
}

diesel::table! {
    terminal_panes (pane_node_id) {
        pane_node_id -> Integer,
        uuid -> Binary,
        cwd -> Text,
        shell_title -> Nullable<Text>,
        is_active -> Integer,
        clear_gap_height_px -> Float,
        clear_gap_before_block_id -> Nullable<Integer>,
        clear_gap_after_all -> Integer,
        agent_id -> Nullable<Text>,
        browser_url -> Nullable<Text>,
    }
}

diesel::table! {
    blocks (id) {
        id -> Integer,
        pane_uuid -> Binary,
        command -> Text,
        cwd -> Text,
        output_json -> Text,
        status -> Text,
        exit_code -> Nullable<Integer>,
        started_at_unix_ms -> Nullable<BigInt>,
        duration_ms -> Nullable<BigInt>,
        expanded -> Integer,
        bookmarked -> Integer,
        sort_order -> Integer,
        created_at_unix_ms -> BigInt,
    }
}

diesel::table! {
    agent_blocks (id) {
        id              -> Integer,
        pane_uuid       -> Binary,
        conversation_id -> Binary,
        kind_json       -> Text,
        sort_order      -> Integer,
        created_at_unix_ms -> BigInt,
    }
}

diesel::table! {
    conversation_events (id) {
        id -> Integer,
        conversation_key -> Text,
        seq -> BigInt,
        kind -> Text,
        payload_json -> Text,
        created_at_unix_ms -> BigInt,
    }
}

diesel::table! {
    command_history (id) {
        id                   -> Integer,
        command              -> Text,
        last_used_at_unix_ms -> BigInt,
        use_count            -> Integer,
    }
}

diesel::joinable!(pane_nodes -> tabs (tab_id));
diesel::joinable!(terminal_panes -> pane_nodes (pane_node_id));

diesel::allow_tables_to_appear_in_same_query!(
    app,
    tabs,
    pane_nodes,
    terminal_panes,
    blocks,
    agent_blocks,
    command_history,
    conversation_events
);
