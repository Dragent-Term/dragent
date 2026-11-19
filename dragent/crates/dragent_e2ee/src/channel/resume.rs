//! §4.6 rekey, reconnect and resume: no input is lost silently across channels.
//!
//! Both rekey and reconnect use the same steps:
//!
//! 1. **The old channel goes quiet.** After its new HS1 the phone sends no more
//!    DATA on the old channel ([`super::Channel::quiesce`]); input waits in a
//!    queue for the new channel.
//! 2. **`hello` says where the old channel stopped:**
//!    `hello {resume: {chan, last_sent_ctr}}` ([`HelloResume`]).
//! 3. **The desktop processes the old channel through `last_sent_ctr` before
//!    anything from the new one** (at most [`super::DRAIN_TIMEOUT_MS`]);
//!    an old-channel frame above it is a protocol error
//!    ([`super::Channel::set_resume_ceiling`]). This is [`ResumeGate`].
//! 4. **If the old channel falls short**, the desktop discards the new
//!    channel's DATA since `hello`, sends `input_lost {handles}`, and writes
//!    nothing from the new channel until `input_lost_ack`.
//! 5. **Old-channel wind-down**: `bye{rekeyed}`, owed replies for 10 s, then
//!    zeroise. At most 2 channels per pair; a third evicts the oldest
//!    ([`PairChannels`]).
//!
//! Security review implementer notes, as implemented here:
//!
//! 1. The phone expects `bye{rekeyed}` on the old channel, or TERM
//!    `base == total` continuity on the new one, and re-seeds a stream if
//!    neither arrives ([`OutputContinuity`]).
//! 2. The desktop keeps each dead channel's last-processed counter until a
//!    resume consumes it or the lease expires, and the `input_lost`-pending
//!    state survives further reconnects: it is re-sent on the next channel if
//!    the ack never arrived ([`ResumeLedger`]).
//! 3. JSON requests sent on the new channel before `input_lost_ack` are
//!    discarded by the desktop, so the phone fails them at once
//!    ([`RequestsBeforeAck`]); flow-control and part acks are never discarded
//!    ([`RecordClass::FlowControl`]).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::{ProtocolViolation, DRAIN_TIMEOUT_MS, MAX_CHANNELS_PER_PAIR};
use crate::ids::ChannelId;

/// A terminal handle (minted by the desktop, bound to phone key and lease).
pub type Handle = u32;

/// `hello {resume: {chan, last_sent_ctr}}`: the old channel and the last DATA
/// counter the phone sent on it (`None` if it sent none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelloResume {
    /// The old channel's id.
    pub chan: ChannelId,
    /// The last DATA counter the phone sent on it.
    pub last_sent_ctr: Option<u64>,
}

// ---------------------------------------------------------------------------
// Desktop side
// ---------------------------------------------------------------------------

/// The most dead channels remembered per session; older ones are forgotten
/// (a resume naming a forgotten channel is treated as having fallen short).
const MAX_REMEMBERED_DEAD: usize = 16;

/// Per (phone key, lease) resume memory that outlives individual channels
/// (implementer note 2). Drop it, or call [`ResumeLedger::lease_expired`],
/// when the lease ends.
#[derive(Debug, Clone, Default)]
pub struct ResumeLedger {
    dead: VecDeque<(ChannelId, Option<u64>)>,
    input_lost_pending: Option<BTreeSet<Handle>>,
}

impl ResumeLedger {
    /// An empty ledger for a new lease.
    pub fn new() -> Self {
        Self::default()
    }

    /// A channel of this session closed (any reason) before a resume named
    /// it: remember how far it was processed.
    pub fn channel_died(&mut self, chan: ChannelId, processed_through: Option<u64>) {
        self.dead.retain(|(c, _)| *c != chan);
        if self.dead.len() == MAX_REMEMBERED_DEAD {
            self.dead.pop_front();
        }
        self.dead.push_back((chan, processed_through));
    }

    /// How far a dead channel was processed, if it is remembered.
    pub fn dead_progress(&self, chan: &ChannelId) -> Option<Option<u64>> {
        self.dead.iter().find(|(c, _)| c == chan).map(|(_, p)| *p)
    }

    /// The handles of an `input_lost` the phone has not acknowledged yet.
    pub fn input_lost_pending(&self) -> Option<&BTreeSet<Handle>> {
        self.input_lost_pending.as_ref()
    }

    /// The lease ended: forget everything (nothing can be resumed any more).
    pub fn lease_expired(&mut self) {
        *self = Self::default();
    }

    fn consume(&mut self, chan: &ChannelId) {
        self.dead.retain(|(c, _)| c != chan);
    }
}

/// What the desktop knows about the channel named in `hello {resume}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OldChannel {
    /// Still open, for this phone key and lease, processed this far.
    Live {
        /// [`super::Channel::processed_through`].
        processed_through: Option<u64>,
    },
    /// Closed; its progress is in the [`ResumeLedger`].
    Dead {
        /// From [`ResumeLedger::dead_progress`].
        processed_through: Option<u64>,
    },
    /// Not known (never seen, forgotten, or another session's).
    Unknown,
}

impl OldChannel {
    /// Look `chan` up: `live` is the progress of a live channel with that id
    /// for the same phone key and lease, if there is one.
    pub fn find(chan: &ChannelId, live: Option<Option<u64>>, ledger: &ResumeLedger) -> Self {
        match (live, ledger.dead_progress(chan)) {
            (Some(p), _) => OldChannel::Live {
                processed_through: p,
            },
            (None, Some(p)) => OldChannel::Dead {
                processed_through: p,
            },
            (None, None) => OldChannel::Unknown,
        }
    }
}

/// How the caller classified a decoded new-channel record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordClass {
    /// CTRL `input_lost_ack`.
    InputLostAck,
    /// Flow-control or part acknowledgements: never discarded (note 3), or
    /// streams may stall.
    FlowControl,
    /// Everything else: input, JSON requests, other CTRL.
    Other,
}

/// What the desktop must do, in order.
#[derive(Debug, PartialEq, Eq)]
pub enum GateAction<T> {
    /// Call [`super::Channel::set_resume_ceiling`] on the live old channel.
    LimitOld {
        /// The old channel.
        chan: ChannelId,
        /// The ceiling.
        last_sent_ctr: Option<u64>,
    },
    /// Process these new-channel records now, in this order.
    Process(Vec<T>),
    /// Drop these records: never written to a PTY, never answered.
    Discard(Vec<T>),
    /// Send CTRL `input_lost {handles}` on the new channel.
    SendInputLost(BTreeSet<Handle>),
    /// The phone acknowledged `input_lost`; new-channel input is written again.
    InputLostCleared,
    /// Send `bye{rekeyed}` on the live old channel and begin its wind-down
    /// ([`super::Channel::begin_wind_down`]).
    WindDownOld(ChannelId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GateState {
    /// Waiting for the old channel to be processed through `through`.
    Draining {
        old: ChannelId,
        through: Option<u64>,
        deadline_ms: u64,
    },
    /// `input_lost` sent; waiting for `input_lost_ack`.
    InputLostPending,
    /// Records flow normally.
    Normal,
}

/// The desktop's §4.6 gate for one new channel, created when its `hello`
/// arrives. Every record the new channel delivers after `hello` goes through
/// [`ResumeGate::on_new_record`].
#[derive(Debug)]
pub struct ResumeGate<T> {
    state: GateState,
    old_live: Option<ChannelId>,
    handles: BTreeSet<Handle>,
    held: Vec<(T, RecordClass)>,
}

/// Is `processed` short of, equal to, or beyond `promised`? (`None` < `Some`.)
fn compare(processed: Option<u64>, promised: Option<u64>) -> std::cmp::Ordering {
    processed.cmp(&promised)
}

impl<T> ResumeGate<T> {
    /// `hello` arrived on a new, confirmed channel.
    ///
    /// - `resume`: the `hello`'s resume field. `None` means a fresh session:
    ///   **nothing is rebound** (the caller mints fresh handles) and the
    ///   ledger is reset.
    /// - `old`: what the desktop knows about `resume.chan` ([`OldChannel::find`]).
    /// - `handles`: the terminal handles being rebound to the new channel; an
    ///   `input_lost` names these.
    ///
    /// An error is a protocol error on the new channel.
    pub fn on_hello(
        resume: Option<HelloResume>,
        old: OldChannel,
        ledger: &mut ResumeLedger,
        handles: BTreeSet<Handle>,
        now_ms: u64,
    ) -> Result<(Self, Vec<GateAction<T>>), ProtocolViolation> {
        let Some(resume) = resume else {
            ledger.lease_expired();
            let gate = ResumeGate {
                state: GateState::Normal,
                old_live: None,
                handles,
                held: Vec::new(),
            };
            return Ok((gate, Vec::new()));
        };
        ledger.consume(&resume.chan);
        let mut gate = ResumeGate {
            state: GateState::Normal,
            old_live: None,
            handles,
            held: Vec::new(),
        };
        let mut actions = Vec::new();
        use std::cmp::Ordering::*;
        match old {
            OldChannel::Live { processed_through } => {
                if compare(processed_through, resume.last_sent_ctr) == Greater {
                    return Err(ProtocolViolation::InconsistentResume);
                }
                gate.old_live = Some(resume.chan);
                actions.push(GateAction::LimitOld {
                    chan: resume.chan,
                    last_sent_ctr: resume.last_sent_ctr,
                });
                if processed_through == resume.last_sent_ctr {
                    gate.finish(true, ledger, &mut actions);
                } else {
                    gate.state = GateState::Draining {
                        old: resume.chan,
                        through: resume.last_sent_ctr,
                        deadline_ms: now_ms.saturating_add(DRAIN_TIMEOUT_MS),
                    };
                }
            }
            OldChannel::Dead { processed_through } => {
                match compare(processed_through, resume.last_sent_ctr) {
                    Greater => return Err(ProtocolViolation::InconsistentResume),
                    Equal => gate.finish(true, ledger, &mut actions),
                    Less => gate.finish(false, ledger, &mut actions),
                }
            }
            // Progress can't be shown, so anything the phone sent is lost.
            OldChannel::Unknown => {
                gate.finish(resume.last_sent_ctr.is_none(), ledger, &mut actions)
            }
        }
        Ok((gate, actions))
    }

    /// Whether the gate is still waiting for the old channel.
    pub fn is_draining(&self) -> bool {
        matches!(self.state, GateState::Draining { .. })
    }

    /// Whether new-channel input is blocked pending `input_lost_ack`.
    pub fn is_input_lost_pending(&self) -> bool {
        self.state == GateState::InputLostPending
    }

    /// The old channel processed a frame with counter `ctr` (call after each
    /// record from it is processed).
    pub fn on_old_processed(&mut self, ctr: u64, ledger: &mut ResumeLedger) -> Vec<GateAction<T>> {
        let mut actions = Vec::new();
        if let GateState::Draining { through, .. } = self.state {
            if through == Some(ctr) {
                self.finish(true, ledger, &mut actions);
            }
        }
        actions
    }

    /// The old channel closed (any reason, including `protocol_error`).
    pub fn on_old_closed(&mut self, ledger: &mut ResumeLedger) -> Vec<GateAction<T>> {
        let mut actions = Vec::new();
        if self.is_draining() {
            self.old_live = None;
            self.finish(false, ledger, &mut actions);
        }
        actions
    }

    /// Advance the drain timer.
    pub fn poll(&mut self, now_ms: u64, ledger: &mut ResumeLedger) -> Vec<GateAction<T>> {
        let mut actions = Vec::new();
        if let GateState::Draining { deadline_ms, .. } = self.state {
            if now_ms >= deadline_ms {
                self.finish(false, ledger, &mut actions);
            }
        }
        actions
    }

    /// A record from the new channel (after `hello`), decoded and classified.
    pub fn on_new_record(
        &mut self,
        record: T,
        class: RecordClass,
        ledger: &mut ResumeLedger,
    ) -> Vec<GateAction<T>> {
        match self.state {
            GateState::Draining { .. } => {
                self.held.push((record, class));
                Vec::new()
            }
            GateState::InputLostPending => match class {
                RecordClass::InputLostAck => {
                    ledger.input_lost_pending = None;
                    self.state = GateState::Normal;
                    vec![GateAction::InputLostCleared]
                }
                RecordClass::FlowControl => vec![GateAction::Process(vec![record])],
                RecordClass::Other => vec![GateAction::Discard(vec![record])],
            },
            GateState::Normal => match class {
                // Nothing is pending on this channel: a stray ack is ignored.
                RecordClass::InputLostAck => vec![GateAction::Discard(vec![record])],
                _ => vec![GateAction::Process(vec![record])],
            },
        }
    }

    /// The old channel is done (`complete`: processed through
    /// `last_sent_ctr`).
    fn finish(
        &mut self,
        complete: bool,
        ledger: &mut ResumeLedger,
        actions: &mut Vec<GateAction<T>>,
    ) {
        let held = std::mem::take(&mut self.held);
        // A pending loss from an earlier channel that was never acknowledged
        // is re-sent here (note 2), even if this drain itself was complete.
        let lost = !complete || ledger.input_lost_pending.is_some();
        if lost {
            let mut handles = ledger.input_lost_pending.take().unwrap_or_default();
            handles.extend(self.handles.iter().copied());
            let (mut keep, mut drop) = (Vec::new(), Vec::new());
            for (r, class) in held {
                match class {
                    RecordClass::FlowControl => keep.push(r),
                    // An ack sent before this channel's input_lost can't
                    // acknowledge it.
                    RecordClass::InputLostAck | RecordClass::Other => drop.push(r),
                }
            }
            if !keep.is_empty() {
                actions.push(GateAction::Process(keep));
            }
            if !drop.is_empty() {
                actions.push(GateAction::Discard(drop));
            }
            ledger.input_lost_pending = Some(handles.clone());
            actions.push(GateAction::SendInputLost(handles));
            self.state = GateState::InputLostPending;
        } else {
            let (mut keep, mut drop) = (Vec::new(), Vec::new());
            for (r, class) in held {
                match class {
                    RecordClass::InputLostAck => drop.push(r),
                    _ => keep.push(r),
                }
            }
            if !keep.is_empty() {
                actions.push(GateAction::Process(keep));
            }
            if !drop.is_empty() {
                actions.push(GateAction::Discard(drop));
            }
            self.state = GateState::Normal;
        }
        if let Some(old) = self.old_live.take() {
            actions.push(GateAction::WindDownOld(old));
        }
    }
}

/// The channels of one (phone, desktop) pair, oldest first (§4.6 step 5).
#[derive(Debug, Clone, Default)]
pub struct PairChannels {
    chans: VecDeque<ChannelId>,
}

impl PairChannels {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit a new channel. Returns the channel evicted to make room (close it
    /// with [`super::CloseReason::Evicted`]), if any.
    pub fn admit(&mut self, chan: ChannelId) -> Option<ChannelId> {
        self.chans.retain(|c| *c != chan);
        let evicted = if self.chans.len() >= MAX_CHANNELS_PER_PAIR {
            self.chans.pop_front()
        } else {
            None
        };
        self.chans.push_back(chan);
        evicted
    }

    /// A channel closed.
    pub fn remove(&mut self, chan: &ChannelId) {
        self.chans.retain(|c| c != chan);
    }

    /// The channels, oldest first.
    pub fn channels(&self) -> impl Iterator<Item = &ChannelId> {
        self.chans.iter()
    }
}

// ---------------------------------------------------------------------------
// Phone side
// ---------------------------------------------------------------------------

/// Whether a terminal stream is known to continue gaplessly from the old
/// channel onto the new one (implementer note 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Continuity {
    /// Nothing was lost: render normally.
    Continuous,
    /// Not decided yet (the old channel's tail may still arrive): hold the
    /// new channel's output for this handle.
    Pending,
    /// Output may have been lost: re-seed this stream.
    Reseed,
}

/// Phone-side output continuity across a rekey or reconnect (note 1).
///
/// TERM `base` is the stream offset of a chunk's first byte and `total` the
/// offset after its last byte. A stream is continuous when `bye{rekeyed}`
/// arrived on the old channel (the desktop sends it after its last
/// old-channel output, and §4.3 guarantees nothing before it is missing) and
/// the new channel's first TERM starts where the old one ended, or, without
/// `bye`, when that `base == total` check holds. Anything else re-seeds.
#[derive(Debug, Clone, Default)]
pub struct OutputContinuity {
    /// Per handle: the stream offset the phone has rendered up to.
    totals: BTreeMap<Handle, u64>,
    /// Handles whose first new-channel TERM is waiting, with its `base`.
    pending: BTreeMap<Handle, u64>,
    resolved: BTreeMap<Handle, Continuity>,
    bye_rekeyed: bool,
    old_closed: bool,
}

impl OutputContinuity {
    /// Start tracking at the quiesce, with each rebound handle's current
    /// `total`.
    pub fn new(totals: BTreeMap<Handle, u64>) -> Self {
        OutputContinuity {
            totals,
            ..Default::default()
        }
    }

    fn old_done(&self) -> bool {
        self.bye_rekeyed || self.old_closed
    }

    fn decide(&self, handle: Handle, base: u64) -> Continuity {
        match self.totals.get(&handle) {
            Some(&t) if base == t => Continuity::Continuous,
            Some(&t) if base < t => Continuity::Reseed,
            _ if self.old_done() => Continuity::Reseed,
            _ => Continuity::Pending,
        }
    }

    /// A TERM for `handle` arrived on the **old** channel (after the
    /// quiesce). Returns a pending handle this resolved, if any.
    pub fn on_old_term(&mut self, handle: Handle, total: u64) -> Option<(Handle, Continuity)> {
        if self.resolved.contains_key(&handle) {
            return None;
        }
        self.totals.insert(handle, total);
        let base = *self.pending.get(&handle)?;
        let c = self.decide(handle, base);
        if c != Continuity::Pending {
            self.pending.remove(&handle);
            self.resolved.insert(handle, c);
            return Some((handle, c));
        }
        None
    }

    /// A TERM for `handle` arrived on the **new** channel with this `base`.
    pub fn on_new_term(&mut self, handle: Handle, base: u64) -> Continuity {
        if let Some(c) = self.resolved.get(&handle) {
            return *c;
        }
        if self.pending.contains_key(&handle) {
            return Continuity::Pending;
        }
        let c = self.decide(handle, base);
        match c {
            Continuity::Pending => {
                self.pending.insert(handle, base);
            }
            _ => {
                self.resolved.insert(handle, c);
            }
        }
        c
    }

    /// `bye{rekeyed}` arrived on the old channel: its output is complete.
    /// Returns the pending handles this resolved.
    pub fn on_old_bye_rekeyed(&mut self) -> Vec<(Handle, Continuity)> {
        self.bye_rekeyed = true;
        self.resolve_pending()
    }

    /// The old channel closed. Without a prior `bye{rekeyed}`, every handle
    /// not yet proven continuous must be re-seeded; returns those handles.
    pub fn on_old_closed(&mut self) -> Vec<Handle> {
        self.old_closed = true;
        let mut reseed: Vec<Handle> = self
            .resolve_pending()
            .into_iter()
            .filter(|(_, c)| *c == Continuity::Reseed)
            .map(|(h, _)| h)
            .collect();
        if !self.bye_rekeyed {
            for h in self.totals.keys() {
                if !self.resolved.contains_key(h) {
                    reseed.push(*h);
                }
            }
            for h in &reseed {
                self.resolved.insert(*h, Continuity::Reseed);
            }
        }
        reseed.sort_unstable();
        reseed.dedup();
        reseed
    }

    fn resolve_pending(&mut self) -> Vec<(Handle, Continuity)> {
        let pending = std::mem::take(&mut self.pending);
        let mut out = Vec::new();
        for (h, base) in pending {
            let c = self.decide(h, base);
            debug_assert_ne!(c, Continuity::Pending);
            self.resolved.insert(h, c);
            out.push((h, c));
        }
        out
    }
}

/// Phone side of note 3: JSON requests sent on a resumed channel before the
/// desktop could have accepted its input are discarded by the desktop if it
/// sends `input_lost`, so the phone fails them at once instead of waiting
/// for a timeout.
#[derive(Debug, Clone)]
pub struct RequestsBeforeAck<Id> {
    tracking: bool,
    ids: Vec<Id>,
}

impl<Id> Default for RequestsBeforeAck<Id> {
    fn default() -> Self {
        RequestsBeforeAck {
            tracking: true,
            ids: Vec::new(),
        }
    }
}

impl<Id> RequestsBeforeAck<Id> {
    /// Start tracking when `hello {resume}` is sent.
    pub fn new() -> Self {
        Self::default()
    }

    /// A JSON request was sent on the new channel.
    pub fn on_request_sent(&mut self, id: Id) {
        if self.tracking {
            self.ids.push(id);
        }
    }

    /// A reply arrived on the new channel: the desktop is serving it, so no
    /// `input_lost` can follow on this channel. Stop tracking.
    pub fn on_reply(&mut self) {
        self.tracking = false;
        self.ids.clear();
    }

    /// CTRL `input_lost` arrived: fail these requests now. The caller drops
    /// its queued input, shows "Some of what you typed didn't reach
    /// `<desktop>`", and sends `input_lost_ack` before any further request.
    pub fn on_input_lost(&mut self) -> Vec<Id> {
        self.tracking = false;
        std::mem::take(&mut self.ids)
    }
}

#[cfg(test)]
mod tests {
    use super::super::state::tests::open_pair;
    use super::super::{ChannelError, Inbound};
    use super::*;

    fn handles(h: &[Handle]) -> BTreeSet<Handle> {
        h.iter().copied().collect()
    }

    // -- desktop gate ------------------------------------------------------

    #[test]
    fn fresh_session_rebinds_nothing_and_resets_ledger() {
        let mut ledger = ResumeLedger::new();
        ledger.channel_died(ChannelId([1; 16]), Some(3));
        ledger.input_lost_pending = Some(handles(&[7]));
        let (mut g, actions) =
            ResumeGate::<&str>::on_hello(None, OldChannel::Unknown, &mut ledger, handles(&[]), 0)
                .unwrap();
        assert!(actions.is_empty());
        assert!(ledger.dead_progress(&ChannelId([1; 16])).is_none());
        assert!(ledger.input_lost_pending().is_none());
        assert_eq!(
            g.on_new_record("input", RecordClass::Other, &mut ledger),
            vec![GateAction::Process(vec!["input"])]
        );
    }

    #[test]
    fn live_old_already_complete() {
        let mut ledger = ResumeLedger::new();
        let old = ChannelId([1; 16]);
        let r = HelloResume {
            chan: old,
            last_sent_ctr: Some(4),
        };
        let (g, actions) = ResumeGate::<&str>::on_hello(
            Some(r),
            OldChannel::Live {
                processed_through: Some(4),
            },
            &mut ledger,
            handles(&[1]),
            0,
        )
        .unwrap();
        assert_eq!(
            actions,
            vec![
                GateAction::LimitOld {
                    chan: old,
                    last_sent_ctr: Some(4)
                },
                GateAction::WindDownOld(old)
            ]
        );
        assert!(!g.is_draining() && !g.is_input_lost_pending());
    }

    #[test]
    fn drain_completes_then_held_records_are_processed_in_order() {
        let mut ledger = ResumeLedger::new();
        let old = ChannelId([1; 16]);
        let r = HelloResume {
            chan: old,
            last_sent_ctr: Some(5),
        };
        let (mut g, _) = ResumeGate::on_hello(
            Some(r),
            OldChannel::Live {
                processed_through: Some(3),
            },
            &mut ledger,
            handles(&[1]),
            0,
        )
        .unwrap();
        assert!(g.is_draining());
        // New-channel DATA is held, not processed.
        assert!(g
            .on_new_record("n1", RecordClass::Other, &mut ledger)
            .is_empty());
        assert!(g
            .on_new_record("n2", RecordClass::FlowControl, &mut ledger)
            .is_empty());
        assert!(g.on_old_processed(4, &mut ledger).is_empty());
        assert_eq!(
            g.on_old_processed(5, &mut ledger),
            vec![
                GateAction::Process(vec!["n1", "n2"]),
                GateAction::WindDownOld(old)
            ]
        );
        assert_eq!(
            g.on_new_record("n3", RecordClass::Other, &mut ledger),
            vec![GateAction::Process(vec!["n3"])]
        );
    }

    /// §12.3 H13R: the relay drops the old channel's last frames, then the
    /// drain times out: no new-channel input before the loss is shown, and
    /// nothing written until the ack.
    #[test]
    fn tail_dropped_at_rekey_times_out_into_input_lost() {
        let mut ledger = ResumeLedger::new();
        let old = ChannelId([1; 16]);
        let r = HelloResume {
            chan: old,
            last_sent_ctr: Some(9),
        };
        let (mut g, _) = ResumeGate::on_hello(
            Some(r),
            OldChannel::Live {
                processed_through: Some(6),
            },
            &mut ledger,
            handles(&[1, 2]),
            1_000,
        )
        .unwrap();
        g.on_new_record("typed-after", RecordClass::Other, &mut ledger);
        g.on_new_record("flow-ack", RecordClass::FlowControl, &mut ledger);
        g.on_new_record("rpc-request", RecordClass::Other, &mut ledger);
        assert!(g.poll(1_000 + DRAIN_TIMEOUT_MS - 1, &mut ledger).is_empty());
        assert_eq!(
            g.poll(1_000 + DRAIN_TIMEOUT_MS, &mut ledger),
            vec![
                GateAction::Process(vec!["flow-ack"]),
                GateAction::Discard(vec!["typed-after", "rpc-request"]),
                GateAction::SendInputLost(handles(&[1, 2])),
                GateAction::WindDownOld(old),
            ]
        );
        assert!(g.is_input_lost_pending());
        // Before the ack: input and requests discarded, flow control passes.
        assert_eq!(
            g.on_new_record("more input", RecordClass::Other, &mut ledger),
            vec![GateAction::Discard(vec!["more input"])]
        );
        assert_eq!(
            g.on_new_record("part-ack", RecordClass::FlowControl, &mut ledger),
            vec![GateAction::Process(vec!["part-ack"])]
        );
        assert_eq!(
            g.on_new_record("ack", RecordClass::InputLostAck, &mut ledger),
            vec![GateAction::InputLostCleared]
        );
        assert!(ledger.input_lost_pending().is_none());
        assert_eq!(
            g.on_new_record("after ack", RecordClass::Other, &mut ledger),
            vec![GateAction::Process(vec!["after ack"])]
        );
    }

    /// §12.3: tail dropped at a forced reconnect (old channel closed short).
    #[test]
    fn old_channel_closed_short_is_input_lost() {
        let mut ledger = ResumeLedger::new();
        let old = ChannelId([1; 16]);
        let r = HelloResume {
            chan: old,
            last_sent_ctr: Some(9),
        };
        // Live but then closes.
        let (mut g, _) = ResumeGate::on_hello(
            Some(r),
            OldChannel::Live {
                processed_through: Some(6),
            },
            &mut ledger,
            handles(&[1]),
            0,
        )
        .unwrap();
        g.on_new_record("x", RecordClass::Other, &mut ledger);
        assert_eq!(
            g.on_old_closed(&mut ledger),
            vec![
                GateAction::Discard(vec!["x"]),
                GateAction::SendInputLost(handles(&[1]))
            ]
        );
        // Already dead when the new hello arrives.
        let mut ledger = ResumeLedger::new();
        ledger.channel_died(old, Some(6));
        let (_, actions) = ResumeGate::<&str>::on_hello(
            Some(r),
            OldChannel::find(&old, None, &ledger),
            &mut ledger,
            handles(&[1]),
            0,
        )
        .unwrap();
        assert_eq!(actions, vec![GateAction::SendInputLost(handles(&[1]))]);
        assert!(
            ledger.dead_progress(&old).is_none(),
            "consumed by the resume"
        );
    }

    #[test]
    fn dead_old_fully_processed_is_complete() {
        let mut ledger = ResumeLedger::new();
        let old = ChannelId([1; 16]);
        ledger.channel_died(old, Some(9));
        let r = HelloResume {
            chan: old,
            last_sent_ctr: Some(9),
        };
        let (g, actions) = ResumeGate::<&str>::on_hello(
            Some(r),
            OldChannel::find(&old, None, &ledger),
            &mut ledger,
            handles(&[1]),
            0,
        )
        .unwrap();
        assert!(actions.is_empty());
        assert!(!g.is_input_lost_pending());
    }

    #[test]
    fn unknown_old_channel() {
        let mut ledger = ResumeLedger::new();
        let r = HelloResume {
            chan: ChannelId([9; 16]),
            last_sent_ctr: None,
        };
        let (g, _) = ResumeGate::<&str>::on_hello(
            Some(r),
            OldChannel::Unknown,
            &mut ledger,
            handles(&[1]),
            0,
        )
        .unwrap();
        assert!(!g.is_input_lost_pending(), "nothing sent, nothing lost");
        let r = HelloResume {
            chan: ChannelId([9; 16]),
            last_sent_ctr: Some(0),
        };
        let (g, _) = ResumeGate::<&str>::on_hello(
            Some(r),
            OldChannel::Unknown,
            &mut ledger,
            handles(&[1]),
            0,
        )
        .unwrap();
        assert!(
            g.is_input_lost_pending(),
            "progress unprovable: treat as lost"
        );
    }

    #[test]
    fn processed_beyond_promise_is_protocol_error() {
        let mut ledger = ResumeLedger::new();
        let old = ChannelId([1; 16]);
        for old_state in [
            OldChannel::Live {
                processed_through: Some(5),
            },
            OldChannel::Dead {
                processed_through: Some(5),
            },
            OldChannel::Live {
                processed_through: Some(0),
            },
        ] {
            let r = HelloResume {
                chan: old,
                last_sent_ctr: if old_state
                    == (OldChannel::Live {
                        processed_through: Some(0),
                    }) {
                    None
                } else {
                    Some(4)
                },
            };
            assert_eq!(
                ResumeGate::<&str>::on_hello(Some(r), old_state, &mut ledger, handles(&[]), 0)
                    .unwrap_err(),
                ProtocolViolation::InconsistentResume
            );
        }
    }

    /// Note 2: the pending input_lost survives a further reconnect and is
    /// re-sent on the next channel, even when that channel's own drain is
    /// complete.
    #[test]
    fn input_lost_pending_survives_reconnect() {
        let mut ledger = ResumeLedger::new();
        let b = ChannelId([2; 16]);
        let (mut gb, _) = ResumeGate::<&str>::on_hello(
            Some(HelloResume {
                chan: ChannelId([1; 16]),
                last_sent_ctr: Some(3),
            }),
            OldChannel::Dead {
                processed_through: Some(1),
            },
            &mut ledger,
            handles(&[1]),
            0,
        )
        .unwrap();
        assert!(gb.is_input_lost_pending());
        // B dies before the ack arrives; it processed nothing after hello.
        let _ = gb.on_new_record("discarded", RecordClass::Other, &mut ledger);
        ledger.channel_died(b, Some(0));
        // C resumes B cleanly, but the loss is still unacknowledged.
        let (mut gc, actions) = ResumeGate::<&str>::on_hello(
            Some(HelloResume {
                chan: b,
                last_sent_ctr: Some(0),
            }),
            OldChannel::find(&b, None, &ledger),
            &mut ledger,
            handles(&[1, 3]),
            0,
        )
        .unwrap();
        assert_eq!(actions, vec![GateAction::SendInputLost(handles(&[1, 3]))]);
        assert_eq!(
            gc.on_new_record("input", RecordClass::Other, &mut ledger),
            vec![GateAction::Discard(vec!["input"])]
        );
        gc.on_new_record("ack", RecordClass::InputLostAck, &mut ledger);
        assert!(ledger.input_lost_pending().is_none());
    }

    #[test]
    fn lease_expiry_forgets_dead_channels() {
        let mut ledger = ResumeLedger::new();
        ledger.channel_died(ChannelId([1; 16]), Some(1));
        ledger.lease_expired();
        assert!(ledger.dead_progress(&ChannelId([1; 16])).is_none());
    }

    #[test]
    fn ledger_remembers_a_bounded_number_of_dead_channels() {
        let mut ledger = ResumeLedger::new();
        for i in 0..(MAX_REMEMBERED_DEAD as u8 + 2) {
            ledger.channel_died(ChannelId([i; 16]), Some(i as u64));
        }
        assert!(ledger.dead_progress(&ChannelId([0; 16])).is_none());
        assert_eq!(
            ledger.dead_progress(&ChannelId([MAX_REMEMBERED_DEAD as u8 + 1; 16])),
            Some(Some(MAX_REMEMBERED_DEAD as u64 + 1))
        );
    }

    #[test]
    fn third_channel_evicts_the_oldest() {
        let mut p = PairChannels::new();
        assert_eq!(p.admit(ChannelId([1; 16])), None);
        assert_eq!(p.admit(ChannelId([2; 16])), None);
        assert_eq!(p.admit(ChannelId([3; 16])), Some(ChannelId([1; 16])));
        assert_eq!(
            p.channels().copied().collect::<Vec<_>>(),
            vec![ChannelId([2; 16]), ChannelId([3; 16])]
        );
        p.remove(&ChannelId([2; 16]));
        assert_eq!(p.admit(ChannelId([4; 16])), None);
    }

    /// End to end over real channels: the phone quiesces A after 3 keystrokes,
    /// the relay drops A's last frame, the desktop gates B until it gives up.
    #[test]
    fn rekey_over_real_channels_with_dropped_tail() {
        let mut a = open_pair(1, 0);
        let k1 = a.phone.seal(b"k1").unwrap();
        let k2 = a.phone.seal(b"k2").unwrap();
        let k3 = a.phone.seal(b"k3").unwrap(); // dropped by the relay
        let resume = a.phone.quiesce();
        assert_eq!(resume.last_sent_ctr, Some(3));
        assert_eq!(a.phone.seal(b"k4").unwrap_err(), ChannelError::NotReady);
        let mut written = vec![];
        for f in [&k1, &k2] {
            if let Inbound::Record(r) = a.desktop.receive(f.ctr, &f.ciphertext).unwrap() {
                written.push(r.to_vec());
            }
        }
        let _ = k3;

        let mut b = open_pair(2, 0);
        let mut ledger = ResumeLedger::new();
        let old = OldChannel::find(&resume.chan, Some(a.desktop.processed_through()), &ledger);
        let (mut gate, actions) =
            ResumeGate::on_hello(Some(resume), old, &mut ledger, handles(&[1]), 0).unwrap();
        for act in &actions {
            if let GateAction::LimitOld { last_sent_ctr, .. } = act {
                a.desktop.set_resume_ceiling(*last_sent_ctr);
            }
        }
        // New-channel keystroke is held while the old channel drains.
        let n1 = b.phone.seal(b"n1").unwrap();
        let Inbound::Record(rec) = b.desktop.receive(n1.ctr, &n1.ciphertext).unwrap() else {
            panic!()
        };
        assert!(gate
            .on_new_record(rec.to_vec(), RecordClass::Other, &mut ledger)
            .is_empty());
        let acts = gate.poll(DRAIN_TIMEOUT_MS, &mut ledger);
        assert!(acts.contains(&GateAction::SendInputLost(handles(&[1]))));
        assert!(acts.contains(&GateAction::Discard(vec![b"n1".to_vec()])));
        assert_eq!(
            written,
            vec![b"k1".to_vec(), b"k2".to_vec()],
            "a prefix of what was typed"
        );

        // An old-channel frame above the promised counter is a protocol error.
        let mut c = open_pair(3, 0);
        let f1 = c.phone.seal(b"x").unwrap();
        c.desktop.set_resume_ceiling(Some(0));
        assert!(matches!(
            c.desktop.receive(f1.ctr, &f1.ciphertext),
            Err(ChannelError::Protocol(
                ProtocolViolation::BeyondResumePoint { .. }
            ))
        ));
    }

    // -- phone side --------------------------------------------------------

    #[test]
    fn continuity_proven_by_base_equals_total() {
        let mut c = OutputContinuity::new([(1, 100), (2, 50)].into_iter().collect());
        assert_eq!(c.on_new_term(1, 100), Continuity::Continuous);
        // Handle 2's old tail is still in flight.
        assert_eq!(c.on_new_term(2, 80), Continuity::Pending);
        assert_eq!(c.on_old_term(2, 80), Some((2, Continuity::Continuous)));
        assert_eq!(c.on_new_term(2, 90), Continuity::Continuous);
        assert!(c.on_old_closed().is_empty());
    }

    #[test]
    fn missing_bye_and_unproven_handles_reseed() {
        let mut c = OutputContinuity::new([(1, 100), (2, 50), (3, 7)].into_iter().collect());
        assert_eq!(c.on_new_term(1, 100), Continuity::Continuous);
        assert_eq!(c.on_new_term(2, 80), Continuity::Pending); // tail 50..80 dropped
                                                               // Old channel closes without bye{rekeyed}: 2 (gap) and 3 (idle,
                                                               // unproven) re-seed.
        assert_eq!(c.on_old_closed(), vec![2, 3]);
        assert_eq!(c.on_new_term(2, 90), Continuity::Reseed);
    }

    #[test]
    fn bye_rekeyed_proves_idle_streams_and_settles_pending() {
        let mut c = OutputContinuity::new([(1, 100), (2, 50), (3, 7)].into_iter().collect());
        assert_eq!(c.on_new_term(2, 80), Continuity::Pending);
        assert_eq!(c.on_old_bye_rekeyed(), vec![(2, Continuity::Reseed)]);
        assert!(
            c.on_old_closed().is_empty(),
            "idle handles 1 and 3 are fine after bye"
        );
        assert_eq!(c.on_new_term(1, 100), Continuity::Continuous);
        assert_eq!(c.on_new_term(3, 9), Continuity::Reseed);
    }

    #[test]
    fn overlap_reseeds() {
        let mut c = OutputContinuity::new([(1, 100)].into_iter().collect());
        assert_eq!(c.on_new_term(1, 90), Continuity::Reseed);
    }

    #[test]
    fn requests_before_ack_fail_at_once() {
        let mut r = RequestsBeforeAck::new();
        r.on_request_sent(1u32);
        r.on_request_sent(2);
        assert_eq!(r.on_input_lost(), vec![1, 2]);
        r.on_request_sent(3);
        assert!(r.on_input_lost().is_empty());

        let mut r = RequestsBeforeAck::new();
        r.on_request_sent(1u32);
        r.on_reply();
        r.on_request_sent(2);
        assert!(r.on_input_lost().is_empty());
    }
}
