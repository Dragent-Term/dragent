//! The built-in agent's conversation, for a phone.
//!
//! The built-in agent doesn't run in a terminal: its conversation lives in
//! the pane's agent controller, on the UI thread. A phone that opens such a
//! tab gets the conversation itself (messages, steps, approvals, questions)
//! and can send prompts and answer, instead of an empty terminal.
//!
//! This module is the meeting point between the two threads:
//!
//! - The pane (UI thread) **publishes** a bounded projection of its
//!   conversation here whenever it changes, but only while a phone watches.
//! - The companion host (its own thread) **reads** the latest projection,
//!   sends phones what changed, and **queues commands** (a prompt, Stop, an
//!   approval) that the pane carries out as if done on the desktop.
//!
//! Everything a phone sees or sends travels inside the encrypted channel; the
//! projection never leaves this process in the clear.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::agent_share::wire_block;
use crate::agent::controller::{AgentBlock, AgentBlockKind};

/// Newest conversation rows a phone is sent.
pub const MAX_BLOCKS: usize = 400;
/// Longest text of one field; longer text is cut and marked.
pub const MAX_TEXT_CHARS: usize = 16_000;
/// Diff lines sent per file.
pub const MAX_DIFF_LINES: usize = 200;
/// Files sent per diff.
pub const MAX_DIFF_FILES: usize = 20;
/// The whole projection stays under this many bytes of JSON, so one message
/// to the phone is always well below the channel's 1 MiB limit.
pub const MAX_TOTAL_BYTES: usize = 600 * 1024;
/// A pane stops projecting this long after the last phone looked.
const WATCH_MS: u64 = 4_000;
/// Longest prompt a phone may send.
pub const MAX_PROMPT_BYTES: usize = 32 * 1024;

/// Wall-clock millis of the last phone view per pane. Opening a tab on the
/// phone marks its agent completion seen, exactly like selecting it on the
/// desktop marks `agent_stop_state.seen_at_ms` — otherwise Review dots only
/// ever clear on the desktop. Read on the render thread, so `seen_at` uses
/// `try_lock` and degrades to "not seen" rather than stalling a frame.
fn seen() -> &'static Mutex<HashMap<Uuid, u64>> {
    static SEEN: OnceLock<Mutex<HashMap<Uuid, u64>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashMap::new()))
}

fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The phone opened this pane (terminal attach or conversation subscribe):
/// its finished agent counts as seen from now on. Returns the recorded
/// instant, so callers can reuse it for the hook activity file too.
pub fn mark_seen(pane: Uuid) -> u64 {
    let now = wall_ms();
    if let Ok(mut seen) = seen().try_lock() {
        seen.insert(pane, now);
        // Bounded without a pruner: entries only matter against stop times,
        // and a phone opens a handful of tabs per session.
        if seen.len() > 4096 {
            let cutoff = now.saturating_sub(24 * 60 * 60 * 1000);
            seen.retain(|_, at| *at >= cutoff);
        }
    }
    now
}

/// Wall millis the phone last opened this pane, if ever. `None` on a held
/// lock rather than blocking render.
pub fn seen_at(pane: Uuid) -> Option<u64> {
    seen().try_lock().ok()?.get(&pane).copied()
}

/// One conversation row as the phone gets it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: u64,
    /// Changes exactly when the row's content does.
    pub hash: u64,
    pub value: Value,
}

/// The conversation of one pane at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// Bumped on every publish.
    pub rev: u64,
    /// A turn is running (Stop applies; a new prompt queues).
    pub busy: bool,
    /// Older rows were left out to stay within the limits.
    pub clipped: bool,
    pub rows: Vec<Row>,
}

/// What a phone asks the pane to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// A phone started watching: publish now.
    Refresh,
    Submit(String),
    Stop,
    Approve(Uuid),
    ApproveAlways(Uuid),
    Reject(Uuid),
    Answer { id: Uuid, option: String },
    ConfirmAnswer(Uuid),
    AnswerText { id: Uuid, text: String },
    ApprovePlan(Uuid),
    RefinePlan(Uuid),
    CancelQueued(Uuid),
    /// The tab's model menu, as the desktop shows it (any terminal tab).
    Models(Reply),
    /// Pick a model (and effort) from that menu, as a click would.
    SetModel { id: String, effort: Option<String>, reply: Reply },
}

/// Where a pane answers a phone's question, once.
#[derive(Clone, Debug)]
pub struct Reply(Arc<Mutex<Option<std::sync::mpsc::SyncSender<Value>>>>);

impl Reply {
    pub fn new() -> (Self, std::sync::mpsc::Receiver<Value>) {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        (Self(Arc::new(Mutex::new(Some(sender)))), receiver)
    }
    pub fn send(&self, value: Value) {
        if let Some(sender) = self.0.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = sender.try_send(value);
        }
    }
}

impl PartialEq for Reply {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for Reply {}

struct Pane {
    snapshot: Option<Arc<Snapshot>>,
    commands: async_channel::Sender<Command>,
    watched_at: Option<web_time::Instant>,
    rev: u64,
}

fn panes() -> &'static Mutex<HashMap<Uuid, Pane>> {
    static PANES: OnceLock<Mutex<HashMap<Uuid, Pane>>> = OnceLock::new();
    PANES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<Uuid, Pane>> {
    panes().lock().unwrap_or_else(|e| e.into_inner())
}

/// A pane starts taking phone commands. One stream per pane; registering
/// again replaces the earlier stream.
pub fn register(pane: Uuid) -> async_channel::Receiver<Command> {
    let (commands, receiver) = async_channel::bounded(32);
    lock().insert(
        pane,
        Pane {
            snapshot: None,
            commands,
            watched_at: None,
            rev: 0,
        },
    );
    receiver
}

/// The pane got its lasting id (restore): keep its stream under the new id.
pub fn rebind(old: Uuid, new: Uuid) {
    if old == new {
        return;
    }
    let mut panes = lock();
    if let Some(mut pane) = panes.remove(&old) {
        pane.snapshot = None;
        panes.insert(new, pane);
    }
}

pub fn unregister(pane: Uuid) {
    lock().remove(&pane);
}

/// Whether a phone looked at this pane recently. A pane only projects its
/// conversation while this is true.
pub fn watched(pane: Uuid) -> bool {
    lock()
        .get(&pane)
        .and_then(|p| p.watched_at)
        .is_some_and(|at| at.elapsed().as_millis() as u64 <= WATCH_MS)
}

/// The companion host is serving this pane to a phone. Asks the pane for a
/// fresh projection when it wasn't being watched.
pub fn watch(pane: Uuid) {
    let mut panes = lock();
    let Some(entry) = panes.get_mut(&pane) else { return };
    let fresh = entry
        .watched_at
        .is_some_and(|at| at.elapsed().as_millis() as u64 <= WATCH_MS);
    entry.watched_at = Some(web_time::Instant::now());
    if !fresh || entry.snapshot.is_none() {
        let _ = entry.commands.try_send(Command::Refresh);
    }
}

/// The pane's conversation changed.
pub fn publish(pane: Uuid, mut snapshot: Snapshot) {
    let mut panes = lock();
    let Some(entry) = panes.get_mut(&pane) else { return };
    entry.rev += 1;
    snapshot.rev = entry.rev;
    entry.snapshot = Some(Arc::new(snapshot));
}

pub fn snapshot(pane: Uuid) -> Option<Arc<Snapshot>> {
    lock().get(&pane).and_then(|p| p.snapshot.clone())
}

/// Queue a phone's command for the pane. False when the pane is gone or
/// isn't taking any more right now.
pub fn send(pane: Uuid, command: Command) -> bool {
    lock()
        .get(&pane)
        .is_some_and(|p| p.commands.try_send(command).is_ok())
}

fn clip(text: &str) -> (String, bool) {
    match text.char_indices().nth(MAX_TEXT_CHARS) {
        None => (text.to_string(), false),
        Some((cut, _)) => (format!("{}…", &text[..cut]), true),
    }
}

/// Cut every long string and long diff in place. True when anything was cut.
fn clip_value(value: &mut Value) -> bool {
    match value {
        Value::String(text) => {
            let (short, cut) = clip(text);
            if cut {
                *text = short;
            }
            cut
        }
        Value::Array(items) => items.iter_mut().fold(false, |cut, item| clip_value(item) | cut),
        Value::Object(map) => {
            let mut cut = false;
            for (key, item) in map.iter_mut() {
                if let Value::Array(items) = item {
                    let limit = match key.as_str() {
                        "lines" => MAX_DIFF_LINES,
                        "files" => MAX_DIFF_FILES,
                        _ => usize::MAX,
                    };
                    if items.len() > limit {
                        items.truncate(limit);
                        cut = true;
                    }
                }
                cut |= clip_value(item);
            }
            cut
        }
        _ => false,
    }
}

fn hash_of(value: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.to_string().hash(&mut hasher);
    hasher.finish()
}

fn row(id: u64, mut value: Value) -> Row {
    if clip_value(&mut value) {
        value["clipped"] = json!(true);
    }
    Row {
        id,
        hash: hash_of(&value),
        value,
    }
}

/// The phone's rows for one block. Groups are flattened: a phone shows one
/// flat conversation, with a closing line for each finished turn.
fn rows_of(block: &AgentBlock, out: &mut Vec<Row>) {
    let id = block.id as u64;
    match &block.kind {
        AgentBlockKind::Empty => {}
        AgentBlockKind::TurnGroup {
            children,
            duration_ms,
            status_note,
            ..
        } => {
            for child in children {
                rows_of(child, out);
            }
            if duration_ms.is_some() || status_note.is_some() {
                out.push(row(
                    id,
                    json!({"id": id, "ts_ms": block.created_at_unix_ms, "kind": "turn_end",
                        "duration_ms": duration_ms, "status_note": status_note}),
                ));
            }
        }
        AgentBlockKind::ActivityGroup { blocks, .. } => {
            for child in blocks {
                rows_of(child, out);
            }
        }
        AgentBlockKind::TodoList { todos } => {
            let items: Vec<Value> = todos
                .iter()
                .map(|t| json!({"content": t.content, "status": t.status}))
                .collect();
            out.push(row(
                id,
                json!({"id": id, "ts_ms": block.created_at_unix_ms, "kind": "todos", "items": items}),
            ));
        }
        kind => {
            let Ok(mut value) = serde_json::to_value(wire_block(block)) else {
                return;
            };
            // What the phone names when it answers this row.
            match kind {
                AgentBlockKind::ApprovalRequest { action_id, .. } => {
                    value["ref"] = json!(action_id.to_string());
                }
                AgentBlockKind::UserQuestion {
                    action_id,
                    questions,
                    current,
                    answers,
                    ..
                } => {
                    value["ref"] = json!(action_id.to_string());
                    value["index"] = json!(current);
                    value["count"] = json!(questions.len());
                    if let Some(q) = questions.get(*current) {
                        value["multi"] = json!(q.multi_select);
                        value["custom"] = json!(q.custom);
                        value["descriptions"] =
                            json!(q.options.iter().map(|o| o.description.clone()).collect::<Vec<_>>());
                    }
                    value["picked"] = json!(answers.get(*current).cloned().unwrap_or_default());
                }
                AgentBlockKind::PlanReview { plan_id, .. } => {
                    value["ref"] = json!(plan_id.to_string());
                }
                AgentBlockKind::QueuedMessage { queue_id, .. } => {
                    value["ref"] = json!(queue_id.to_string());
                }
                _ => {}
            }
            out.push(row(id, value));
        }
    }
}

/// The bounded projection of a conversation: the newest rows that fit.
pub fn project(blocks: &[AgentBlock], busy: bool) -> Snapshot {
    let mut rows = Vec::new();
    for block in blocks {
        rows_of(block, &mut rows);
    }
    let mut clipped = false;
    if rows.len() > MAX_BLOCKS {
        rows.drain(..rows.len() - MAX_BLOCKS);
        clipped = true;
    }
    // Keep the newest rows that fit the byte budget.
    let mut total = 0usize;
    let mut keep_from = 0usize;
    for (index, row) in rows.iter().enumerate().rev() {
        total += row.value.to_string().len() + 16;
        if total > MAX_TOTAL_BYTES {
            keep_from = index + 1;
            break;
        }
    }
    if keep_from > 0 {
        rows.drain(..keep_from);
        clipped = true;
    }
    Snapshot {
        rev: 0,
        busy,
        clipped,
        rows,
    }
}

/// What one phone has been sent of a pane, so the next message carries only
/// the rows that changed.
#[derive(Default)]
pub struct Sent {
    rev: Option<u64>,
    rows: HashMap<u64, u64>,
}

impl Sent {
    /// The message that brings this phone up to `snapshot`, or `None` when it
    /// already has it. Always carries the full row order.
    pub fn update(&mut self, snapshot: &Snapshot) -> Option<Value> {
        if self.rev == Some(snapshot.rev) {
            return None;
        }
        self.rev = Some(snapshot.rev);
        let order: Vec<u64> = snapshot.rows.iter().map(|r| r.id).collect();
        let changed: Vec<&Value> = snapshot
            .rows
            .iter()
            .filter(|r| self.rows.get(&r.id) != Some(&r.hash))
            .map(|r| &r.value)
            .collect();
        let message = json!({"rev": snapshot.rev, "busy": snapshot.busy, "clipped": snapshot.clipped,
            "ready": true, "order": order, "blocks": changed});
        self.rows = snapshot.rows.iter().map(|r| (r.id, r.hash)).collect();
        Some(message)
    }
}

/// The first message of a stream whose pane hasn't projected yet.
pub fn waiting_message() -> Value {
    json!({"rev": 0, "busy": false, "clipped": false, "ready": false, "order": [], "blocks": []})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::controller::block::ToolStepStatus;
    use crate::agent::controller::ApprovalStatus;

    fn block(id: usize, kind: AgentBlockKind) -> AgentBlock {
        AgentBlock::new(id, Uuid::nil(), kind)
    }

    fn message(id: usize, text: &str) -> AgentBlock {
        block(id, AgentBlockKind::AgentMessage { text: text.into(), is_streaming: false })
    }

    #[test]
    fn groups_are_flattened_and_a_finished_turn_gets_a_closing_row() {
        let turn = block(
            9,
            AgentBlockKind::TurnGroup {
                children: vec![
                    block(
                        2,
                        AgentBlockKind::ToolStep {
                            tool_id: "t".into(),
                            name: "Read".into(),
                            icon: String::new(),
                            args_summary: "a.rs".into(),
                            status: ToolStepStatus::Done,
                            result_summary: None,
                            duration_ms: Some(4),
                            expanded: false,
                            diff: None,
                        },
                    ),
                    message(3, "done"),
                ],
                expanded: false,
                started_at_ms: 0,
                duration_ms: Some(1500),
                status_note: None,
            },
        );
        let snap = project(&[message(1, "hi"), turn], true);
        let kinds: Vec<&str> = snap.rows.iter().map(|r| r.value["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["agent_message", "tool_step", "agent_message", "turn_end"]);
        assert_eq!(snap.rows[3].value["duration_ms"], 1500);
        assert!(snap.busy);
        assert!(!snap.clipped);
    }

    #[test]
    fn phone_views_are_recorded_per_pane() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        assert_eq!(seen_at(first), None);
        let at = mark_seen(first);
        assert!(at > 0);
        assert_eq!(seen_at(first), Some(at));
        // Other panes are unaffected.
        assert_eq!(seen_at(second), None);
        // Re-marking moves the instant forward (or ties on coarse clocks).
        let again = mark_seen(first);
        assert!(again >= at);
        assert_eq!(seen_at(first), Some(again));
    }

    #[test]
    fn an_approval_carries_the_reference_the_phone_answers_with() {        let id = Uuid::new_v4();
        let snap = project(
            &[block(
                1,
                AgentBlockKind::ApprovalRequest {
                    action_id: id,
                    command: "rm -rf build".into(),
                    reason: "clean".into(),
                    status: ApprovalStatus::Pending,
                },
            )],
            true,
        );
        assert_eq!(snap.rows[0].value["kind"], "approval");
        assert_eq!(snap.rows[0].value["status"], "pending");
        assert_eq!(snap.rows[0].value["ref"], id.to_string());
    }

    #[test]
    fn long_text_is_cut_and_marked_and_old_rows_are_dropped_first() {
        let long = "x".repeat(MAX_TEXT_CHARS + 50);
        let snap = project(&[message(1, &long)], false);
        assert_eq!(snap.rows[0].value["clipped"], true);
        assert_eq!(
            snap.rows[0].value["text"].as_str().unwrap().chars().count(),
            MAX_TEXT_CHARS + 1
        );
        let many: Vec<AgentBlock> = (0..MAX_BLOCKS + 5).map(|i| message(i, "m")).collect();
        let snap = project(&many, false);
        assert_eq!(snap.rows.len(), MAX_BLOCKS);
        assert!(snap.clipped);
        assert_eq!(snap.rows[0].id, 5, "the oldest rows go first");
        // The byte budget also keeps the newest rows.
        let heavy: Vec<AgentBlock> = (0..100).map(|i| message(i, &"y".repeat(MAX_TEXT_CHARS))).collect();
        let snap = project(&heavy, false);
        assert!(snap.clipped);
        assert_eq!(snap.rows.last().unwrap().id, 99);
        let bytes: usize = snap.rows.iter().map(|r| r.value.to_string().len()).sum();
        assert!(bytes <= MAX_TOTAL_BYTES);
    }

    #[test]
    fn a_phone_is_sent_only_the_rows_that_changed() {
        let mut sent = Sent::default();
        let mut blocks = vec![message(1, "a"), message(2, "b")];
        let mut first = project(&blocks, true);
        first.rev = 1;
        let m = sent.update(&first).unwrap();
        assert_eq!(m["order"], json!([1, 2]));
        assert_eq!(m["blocks"].as_array().unwrap().len(), 2);
        assert!(sent.update(&first).is_none(), "nothing new");
        blocks[1].kind = AgentBlockKind::AgentMessage { text: "b more".into(), is_streaming: false };
        blocks.push(message(3, "c"));
        let mut second = project(&blocks, false);
        second.rev = 2;
        let m = sent.update(&second).unwrap();
        assert_eq!(m["order"], json!([1, 2, 3]));
        let ids: Vec<u64> = m["blocks"].as_array().unwrap().iter().map(|b| b["id"].as_u64().unwrap()).collect();
        assert_eq!(ids, [2, 3]);
        assert_eq!(m["busy"], false);
    }

    #[test]
    fn a_pane_projects_only_while_a_phone_watches_and_takes_its_commands() {
        let pane = Uuid::new_v4();
        let commands = register(pane);
        assert!(!watched(pane));
        watch(pane);
        assert!(watched(pane));
        assert_eq!(commands.try_recv(), Ok(Command::Refresh), "asked to publish now");
        publish(pane, project(&[message(1, "a")], false));
        assert_eq!(snapshot(pane).unwrap().rev, 1);
        watch(pane);
        assert!(commands.try_recv().is_err(), "already fresh: no second refresh");
        assert!(send(pane, Command::Submit("hello".into())));
        assert_eq!(commands.try_recv(), Ok(Command::Submit("hello".into())));
        let lasting = Uuid::new_v4();
        rebind(pane, lasting);
        assert!(send(lasting, Command::Stop));
        assert!(!send(pane, Command::Stop));
        unregister(lasting);
        assert!(!send(lasting, Command::Stop));
    }
}
