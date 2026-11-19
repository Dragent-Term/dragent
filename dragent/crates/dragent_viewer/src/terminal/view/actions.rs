use dragent_terminal::{MouseButton, MouseEventKind};
use uuid::Uuid;
use warpui::geometry::vector::Vector2F;
use warpui_core::event::ModifiersState;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryHint {
    pub block_id: usize,
    pub name: String,
    pub is_dir: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkHint {
    pub block_id: usize,
    pub url: String,
    /// Which occurrence this is, when known (terminal block output). Without
    /// it, every row showing the same path text would highlight together.
    pub occurrence: Option<LinkOccurrence>,
}

/// What the pointer is over in an `ls` grid/list card: an entry
/// `(block_id, entry_index)`, a sort toggle, or a view toggle. Lives on the
/// view for the same reason as `TerminalView::hovered_link`: the paint
/// element is rebuilt on every notify, wiping element-local hover state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DirHover {
    pub entry: Option<(usize, usize)>,
    pub sort: Option<crate::console::dir_listing::DirSort>,
    pub view: Option<crate::console::dir_listing::DirView>,
}

/// Where a link starts: `line` indexes the block's full line list (or the
/// bootstrap / live-prompt lines), `start_char` is the char index within it.
/// A link that soft-wraps keeps the start of its first row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkOccurrence {
    pub line: usize,
    pub start_char: usize,
}

/// The copy-confirmation key of a shell block. The block's card and pinned
/// headers share it, and its menu's copy options confirm on it too, because the
/// menu closes as soon as one of them is clicked.
pub fn block_copy_key(block_id: usize) -> String {
    format!("block:{block_id}")
}

#[derive(Clone, Debug)]
pub enum TerminalAction {
    FocusOutput,
    FocusInput,
    /// Remove a pending attachment chip by index.
    RemovePendingAttachment {
        index: usize,
    },
    /// Dismiss the ↑/↓ command-history popover, restoring the typed buffer.
    CloseCmdHistory,
    /// Resize the history popover by a vertical delta (px) from DragResizeElement.
    /// Dragging up (negative dy) grows the popover.
    CmdHistResizeBy {
        dy: f32,
    },
    /// Pick a command-history row by index (click): preview and run it.
    CmdHistoryClick {
        index: usize,
    },
    /// Open the AI-tool `!` scratch-shell history popover (this session's `!`
    /// commands), triggered by ↑ when the rich input starts with `!`.
    OpenSideCmdHistory,
    /// Accept the `!`-command autocomplete ghost in the AI rich input (Tab).
    AcceptRichGhost,
    /// Hide the `!` scratch-shell card (the shell keeps running).
    CloseSideCommand,
    /// Kill and respawn the `!` scratch shell (clears its state and history).
    ResetSideShell,
    /// Send Ctrl+C to the `!` scratch shell's foreground process (Stop button).
    CancelSideCommand,
    /// Resize the scratch-shell card's output area by a vertical delta (px).
    /// Dragging up (negative dy) makes it taller.
    SideCardResizeBy {
        dy: f32,
    },
    /// Turn the scratch-shell output's follow-the-tail auto-scroll on/off.
    SideShellFollowTail {
        follow: bool,
    },
    /// Files dragged onto the pane: attach (terminal mode) or insert paths
    /// into the input (AI-tool mode).
    AttachDroppedFiles {
        paths: Vec<String>,
    },
    /// Open the full-size image viewer at the given image ordinal (among
    /// pending image attachments).
    OpenImageViewer {
        index: usize,
    },
    CloseImageViewer,
    /// The desktop takes a terminal back from the phone that sized it.
    TakeBackFromPhone,
    /// Step the image viewer carousel (+1 / -1, wrapping).
    ImageViewerNav {
        delta: i32,
    },
    /// Open the full-size Mermaid diagram viewer for the given rendered SVG.
    OpenDiagramViewer {
        path: String,
        width: f32,
        height: f32,
    },
    CloseDiagramViewer,
    DiagramViewerZoomIn,
    DiagramViewerZoomOut,
    DiagramViewerZoomReset,
    ClearBlockHints,
    HistoryUp,
    HistoryDown,
    /// Recall previous/next command into the AI rich input (Shift+Up/Down),
    /// bash-style, without opening the history popover.
    RichHistoryUp,
    RichHistoryDown,
    HistorySearchOpen,
    HistorySearchUp,
    HistorySearchDown,
    HistorySearchAccept,
    HistorySearchClose,
    HistorySearchClick {
        index: usize,
    },
    HistorySearchBackspace,
    HistorySearchQueryAppend(String),
    WorkflowSearchOpen,
    WorkflowSearchUp,
    WorkflowSearchDown,
    WorkflowSearchAccept,
    WorkflowSearchClose,
    WorkflowSearchClick {
        index: usize,
    },
    WorkflowSearchBackspace,
    WorkflowSearchQueryAppend(String),
    WorkflowArgPrev,
    WorkflowArgNext,
    ToggleVimMode,
    TabComplete,
    TabCompletePrev,
    AcceptCompletion,
    DismissCompletion,
    ClickPathCompletion {
        index: usize,
    },
    AcceptGhost,
    Submit,
    InsertNewline,
    Interrupt,
    ToggleRichInput,
    SubmitRichInput,
    ToggleVoiceInput,
    OpenFilePicker,
    SelectDirEntry {
        block_id: usize,
        name: String,
        is_dir: bool,
    },
    OpenDirEntry {
        block_id: usize,
        name: String,
        is_dir: bool,
    },
    /// A plain click (no modifier) on a detected link/path: highlights it and
    /// opens the small "Open in Editor / Reveal in Finder" (or "Open in
    /// Browser / Copy Link") action menu anchored near `absolute_position`
    /// (window/screen coordinates — the handler converts to viewport-local).
    SelectLink {
        block_id: usize,
        url: String,
        occurrence: Option<LinkOccurrence>,
        absolute_position: Vector2F,
    },
    OpenLink {
        block_id: usize,
        url: String,
    },
    /// Open an `http(s)://` URL in the system browser, bypassing the
    /// in-app-browser preference (`OpenLink` respects it).
    OpenLinkExternal {
        block_id: usize,
        url: String,
    },
    RevealLinkInFinder {
        block_id: usize,
        url: String,
    },
    /// Copy a link from its action menu and close the menu: a URL verbatim,
    /// a path as the absolute path it resolves to (against the block's cwd).
    CopyLinkTarget {
        block_id: usize,
        url: String,
    },
    CloseLinkMenu,
    /// The pointer is over a detected link/path in normal block output (not
    /// yet clicked). Persisted on the view (not the transient paint element)
    /// so the underline survives the re-render the notify triggers — see
    /// `TerminalView::hovered_link`.
    HoverLink {
        hint: LinkHint,
    },
    ClearHoverLink,
    /// Same idea as `HoverLink`, for the alt-screen grid (TUIs like vim,
    /// htop, Codex) — `(row, start_col, end_col)` per row the link covers,
    /// more than one when the app wrapped it onto following rows.
    HoverAltLink {
        segments: Vec<(usize, usize, usize)>,
    },
    ClearHoverAltLink,
    SetDirHover(DirHover),
    ToggleBlockExpansion {
        block_id: usize,
    },
    ToggleAgentThinking {
        block_id: usize,
    },
    ToggleToolStep {
        block_id: usize,
    },
    /// Expand/collapse a captured edit's diff card ("+N more lines"), keyed by
    /// the tool call id the diff was recorded under.
    ToggleDiffExpand {
        tool_id: String,
    },
    /// Open or fold a line of the chat view (a group of steps, a step's
    /// detail), named by its key.
    ToggleChatLine {
        key: String,
    },
    /// Show this terminal agent's tab as a chat (`true`) or as its terminal.
    SetChatView {
        chat: bool,
    },
    /// Stop the terminal agent shown as a chat, with the key it stops on.
    StopChatAgent,
    /// Draw more of a long conversation in the chat.
    ShowEarlierChat,
    /// Back to the newest line of the chat, and follow it again.
    JumpToLatestChat,
    /// Answer the chat agent's permission question (`None`: dismiss).
    AnswerAcpPermission {
        option: Option<String>,
    },
    CopyText {
        text: String,
    },
    /// A copy button or menu item: copy `text`, then show the confirmation on
    /// the button named `key` for a moment. Nothing is confirmed if `text` is
    /// empty, because nothing reached the clipboard.
    CopyWithConfirmation {
        key: String,
        text: String,
    },
    /// Clear command-input selection so block output copy is not blocked.
    ClearInputEditorSelection,
    SetDirSort(crate::console::dir_listing::DirSort),
    SetDirView(crate::console::dir_listing::DirView),
    SendPty(Vec<u8>),
    Copy,
    Paste,
    ShiftHeld(bool),
    CtrlHeld(bool),
    AltHeld(bool),
    CmdHeld(bool),
    TerminalMouse {
        position: Vector2F,
        button: MouseButton,
        kind: MouseEventKind,
        modifiers: ModifiersState,
    },
    TerminalWheel {
        position: Vector2F,
        delta_y: f32,
        /// True when `delta_y` is a pixel-precise distance (trackpad); false when
        /// it counts discrete wheel LINES. The two are different units, so a
        /// consumer that reads the magnitude must not treat them alike.
        precise: bool,
        modifiers: ModifiersState,
    },
    BlockScroll {
        delta_y: f32,
    },
    /// Fired on every drag-selection update when the cursor may be outside the viewport.
    /// The handler checks cursor_y against viewport bounds and scrolls if needed.
    DragSelectScrollCheck {
        cursor_y: f32,
    },
    /// Mouse released after a possible scrollbar drag — commit scroll position.
    UserScrollInteraction,
    /// Cycle input lock: unlocked → lock-Shell → lock-Agent → unlocked.
    ToggleInputTypeLock,
    /// Open/close the agent modes dropdown (Agent/Plan/Debug/Multitask/Ask).
    ToggleModeMenu,
    /// Pick an agent execution mode from the dropdown.
    SetChatMode {
        mode: crate::agent::chat_mode::ChatMode,
    },
    /// Open/close the model quick-switch dropdown in the input bar.
    ToggleModelMenu,
    /// Pick the agent model from the quick-switch (persists to settings).
    ///
    /// `effort` rides along because the picker lists each effort-capable model
    /// once per level ("Claude Opus 4.8", "… (low)", "… (medium)", "… (high)").
    /// Model and effort are chosen in one click, so they are set together —
    /// selecting a model that can't take an effort resets it to `Default`
    /// rather than leaving a stale level applied to it.
    SetAgentModel {
        model_id: String,
        effort: crate::agent::effort::AgentEffort,
    },
    /// Set a chat agent's own effort level (`low` … `max`), from the slider
    /// under the model list.
    SetChatEffort {
        level: String,
    },
    /// The effort slider under the pointer (window x): pressed, dragged, or
    /// released (which sets the level).
    EffortPointer {
        x: f32,
        phase: EffortPointerPhase,
    },
    /// Move the keyboard selection in the open slash-command palette.
    SlashPaletteMove {
        forward: bool,
    },
    /// Run a slash command by name (Enter or a palette-row click).
    RunSlashCommand {
        name: String,
    },
    /// Open/close the session history panel; fetches this pane's past
    /// conversations from local SQLite when opening.
    ToggleHistoryPanel,
    /// Resume a past conversation picked from the session history panel.
    ResumeConversation {
        conversation_id: Uuid,
    },
    /// Rich input's live-proxy mode: write `bytes` straight to the wrapped
    /// CLI's PTY and adjust the proxy-mode character counter by `delta`.
    ForwardRichInputProxyKey {
        bytes: Vec<u8>,
        delta: i32,
    },
    /// Approve a pending RunCommand agent action.
    ApproveAgentAction {
        action_id: Uuid,
    },
    /// Approve a pending tool action and mark it as always-allowed (persisted).
    AllowToolAlways {
        action_id: Uuid,
    },
    /// Reject a pending RunCommand agent action.
    RejectAgentAction {
        action_id: Uuid,
        feedback: Option<String>,
    },
    /// User clicked an option on an `ask_user` question card. Single-select
    /// records + advances; multi-select toggles the option.
    PickAgentQuestionOption {
        action_id: Uuid,
        option: String,
    },
    /// User clicked "Continue" on a multi-select question card.
    ConfirmAgentQuestion {
        action_id: Uuid,
    },
    /// User submitted a typed custom answer (the "Other" free-text field).
    SubmitAgentQuestionText {
        action_id: Uuid,
    },
    /// Keyboard: move the question-card cursor (Alt+Up/Down). Rendered as a
    /// highlight on the option row; Enter then picks it.
    MoveAgentQuestionCursor {
        action_id: Uuid,
        delta: i32,
    },
    /// Keyboard: pick the option under the question-card cursor (Alt+Enter).
    PickAgentQuestionCursor {
        action_id: Uuid,
    },
    /// Keyboard: pick the Nth option directly, 0-based (Alt+1..9).
    PickAgentQuestionDigit {
        action_id: Uuid,
        index: usize,
    },
    /// User removed a still-queued prompt (the × on its "Queued" pill).
    CancelQueuedMessage {
        queue_id: Uuid,
    },
    /// Accept every agent edit: clears the modified-files list and retires the
    /// Agent Changes card (the edits themselves stay on disk).
    AcceptAllAgentChanges,
    /// User clicked "Implement" on a plan review block — start implementation.
    ApprovePlan {
        plan_id: Uuid,
    },
    /// User clicked "Refine" on a plan review block — return focus to input bar.
    RefinePlan {
        plan_id: Uuid,
    },
    /// Clear all agent blocks and reset the conversation.
    ClearAgentConversation,
    /// Stop (cancel) the currently running agent stream.
    StopAgentStream,
    /// Ask the agent to review its currently tracked workspace changes.
    ReviewAgentChanges,
    /// Resume an interrupted agent turn from its durable execution checkpoint.
    /// The ID pins the request to the exact error card the user clicked.
    ContinueAgent {
        error_block_id: usize,
    },
    /// Request on-demand analysis of a failed shell block (Layer 5 self-heal).
    AnalyzeBlock {
        block_id: usize,
    },
    /// Run the suggested fix command from a completed analysis.
    RunBlockFix {
        block_id: usize,
    },
    /// Dismiss the fix card for a block (analysis result stays cached).
    DismissBlockFix {
        block_id: usize,
    },
    /// Expand a collapsed "Fix available" chip into the full fix card
    /// (proactive scan finished while "Auto-fix on failure" is off).
    RevealBlockFix {
        block_id: usize,
    },
    /// Kill a running agent shell command (live ToolStep row / activity strip).
    CancelAgentCommand {
        task_id: String,
    },
    /// Hand the completed Fix analysis to the normal agent workflow for
    /// diagnosis, edits, approvals, and verification.
    AutoFixBlock {
        block_id: usize,
    },
    /// Open the failed block in the agent.
    OpenInAgent {
        block_id: usize,
    },
    /// Detach a block from the active agent session.
    DetachBlock {
        block_id: usize,
    },
    /// Toggle the git diff bar above the input (collapsed ↔ expanded).
    ToggleGitDiff,
    /// Toggle the full branch diff panel opened from the +N/-N chip in the input bar.
    ToggleBranchDiff,
    /// Toggle staged state for a single file in the branch diff panel.
    StageBranchFile {
        rel_path: String,
    },
    /// Stage all files in the branch diff panel.
    StageAllBranchFiles,
    /// Unstage all files in the branch diff panel.
    UnstageAllBranchFiles,
    /// Discard a single file's changes (staged + unstaged) back to HEAD.
    /// Destructive — routes through the confirm modal first.
    DiscardBranchFile {
        rel_path: String,
    },
    /// Actual discard, after the confirmation modal is accepted.
    DiscardBranchFileConfirmed {
        rel_path: String,
    },
    /// Discard every tracked file's changes in the branch diff panel back to
    /// HEAD. Destructive — routes through the confirm modal first. Untracked
    /// (new) files are left alone, matching every other git GUI's "discard
    /// all" — that's a `git clean`, a different and further-reaching action.
    DiscardAllBranchChanges,
    /// Actual discard-all, after the confirmation modal is accepted.
    DiscardAllBranchChangesConfirmed,
    /// Discard all STAGED files' changes back to HEAD (both the staged and
    /// any unstaged parts of those files). Destructive — routes through the
    /// confirm modal first. Untracked files are left alone.
    DiscardAllStagedChanges,
    /// Actual staged discard, after the confirmation modal is accepted.
    DiscardAllStagedChangesConfirmed,
    /// Generate a commit message from the staged file list.
    GenerateBranchCommitMsg,
    /// Stage the selected files and run `git commit`.
    CommitBranchChanges,
    /// Dismiss the commit-error toast.
    DismissCommitError,
    /// Copy the commit error text and briefly show a "Copied!" indicator.
    CopyCommitError {
        text: String,
    },
    /// Run `git push` for the current branch.
    PushBranchChanges,
    /// Commit whatever is staged (with the panel's message), then push the
    /// current branch to origin — the panel's one-click ship button.
    CommitAndPushBranch,
    /// Pull the current branch's remote changes (the ↓N chip).
    PullBranchChanges,
    /// Open/close the branch popover on the branch chip. Opening kicks off a
    /// `git branch` listing.
    ToggleBranchMenu,
    /// Close the branch popover (outside click / Escape / after switching).
    CloseBranchMenu,
    /// Branch list finished loading.
    BranchListLoaded {
        branches: Vec<String>,
    },
    /// Check out an existing branch. Refuses with a prompt when the working
    /// tree is dirty, unless `force` is set by the user's confirmation.
    SwitchBranch {
        name: String,
        force: bool,
    },
    /// Create and check out a new branch from the current HEAD, using the
    /// popover's text field.
    CreateBranchFromInput,
    /// Create and check out a new branch from a name provided outside the
    /// terminal composer (source-control panel).
    CreateBranchFromName {
        name: String,
    },
    /// Open/close the folder popover on the cwd chip.
    ToggleFolderMenu,
    /// Close the folder popover.
    CloseFolderMenu,
    /// Subdirectory listing for the folder popover finished loading.
    FolderListLoaded {
        dirs: Vec<String>,
    },
    /// `cd` the shell into `path` (absolute), via the PTY so it lands in the
    /// user's own history and scrollback.
    ChangeDirectory {
        path: String,
    },
    /// Restore tracked files to the snapshot taken before the agent's last
    /// mutating turn.
    RevertLastAgentTurn,
    /// Open/reposition, or close if already open on this block, the checkpoint
    /// restore preview popover for a specific user message.
    ToggleCheckpointPreview {
        block_id: usize,
    },
    /// Close the checkpoint restore preview popover, if open.
    CloseCheckpointPreview,
    /// Dismiss any open confirmation modal without acting.
    DismissConfirm,
    /// User confirmed a guarded action; `action` is dispatched when confirm.
    ResolveConfirm {
        confirm: bool,
        action: Box<TerminalAction>,
    },
    /// Actual revert, after the confirmation modal is accepted.
    RevertLastAgentTurnConfirmed,
    /// Actual restore, after the confirmation modal is accepted.
    RestoreToMessageConfirmed {
        block_id: usize,
    },

    // ── Per-block options menu (⋯) ────────────────────────────────────────────
    /// Toggle the block options dropdown anchored to a block header's ⋯ button.
    ToggleBlockMenu {
        block_id: usize,
    },
    /// Close any open block options dropdown.
    CloseBlockMenu,
    /// Copy the full plain-text output of a block to the clipboard.
    CopyBlockOutput {
        block_id: usize,
    },
    /// Copy the command plus its full output to the clipboard.
    CopyBlockCommandAndOutput {
        block_id: usize,
    },
    /// Scroll the viewport so the block's header aligns to the top.
    ScrollBlockToTop {
        block_id: usize,
    },
    /// Scroll the viewport so the block's bottom aligns to the viewport bottom.
    ScrollBlockToBottom {
        block_id: usize,
    },
    /// Right-click anywhere on the block output area — opens/repositions that
    /// block's options menu exactly at the click point. `local_x`/`local_y` are
    /// relative to the block-output viewport's top-left (same frame
    /// `wrap_block_menu_overlay` uses).
    OpenBlockMenuAtClick {
        local_x: f32,
        local_y: f32,
    },
    /// Interrupt the foreground process belonging to a running terminal block.
    TerminateBlock {
        block_id: usize,
    },

    // ── Per-block output filter ───────────────────────────────────────────────
    /// Toggle the inline output filter bar for a block.
    ToggleBlockFilter {
        block_id: usize,
    },
    /// Close the output filter bar for a block (clears its query).
    CloseBlockFilter {
        block_id: usize,
    },
    /// Toggle regex mode for a block's filter query.
    ToggleBlockFilterRegex {
        block_id: usize,
    },
    /// Move the active block-filter match forward or backward.
    BlockFilterMatch {
        block_id: usize,
        delta: i32,
    },
}

/// Where an effort-slider gesture is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffortPointerPhase {
    Press,
    Drag,
    Release,
}
