//! End-to-end encrypted push notifications, desktop side (docs/design/e2ee.md
//! §8, §6.6).
//!
//! `notifications::notify()` calls [`notify`] after its `settings.wants(kind)`
//! check, so Settings → Notifications decides for the desktop and the phone
//! alike. For every paired phone with a push key this:
//!
//! 1. builds the fixed 1,024-byte §8 plaintext (`dragent_e2ee` codec);
//! 2. seals it with Noise `X` from this desktop's identity key to the phone's
//!    push key under the push prologue (`account|phone`), so every blob is
//!    exactly 1,120 bytes / 1,494 base64url characters;
//! 3. hands it to `companion_host::send_host_notify`, which sends it as a
//!    `host-notify` text frame on the host socket.
//!
//! **No plaintext fallback.** If the keychain or pin store can't be opened,
//! nobody is signed in, or a phone has no push key, nothing is sent. The
//! server only ever receives the sealed blob.
//!
//! **Handled notifications (§8 step 6).** A notification counts as handled
//! once the person does something in a Dragent window (a click or key press,
//! `App::last_active_timestamp`) after it was sent. The ledger of sent ids is
//! kept **in memory only** (no notification ids on disk), at most 256 per
//! phone for 24 hours.
//!
//! - **iOS** gets no dismissal pushes (they would differ in push type and
//!   priority). On every app open the phone asks inside the KK channel with
//!   `notifications.done` (served by [`serve_done`]) and removes the
//!   delivered notifications by their collapse id.
//! - **Android** also reconciles on open, and in between gets sealed
//!   `dismiss = 1` blobs, the same size and the same high-priority data
//!   message, at most one per phone per 5 minutes, carrying up to 58 ids.
//! - The desktop learns a phone's platform from `notifications.done`'s
//!   `platform` parameter (the pin store doesn't keep it). Until then the
//!   phone is treated like iOS: no dismissal pushes.
//!
//! **Collapse id.** `base64url(SHA-256(blob)[0..16])` over the base64url blob
//! string, exactly as the push gateway computes `apns-collapse-id`; the iOS
//! extension shows the notification under that identifier, so the phone
//! removes it by the value [`serve_done`] returns.
//!
//! **Workspace and tab.** `notify()`'s callers don't pass the workspace or
//! tab a notification is about, so both ids are the nil id: tapping a push
//! opens the desktop on the phone, not the tab. Follow-up: thread the tab and
//! workspace through the callers that have them.
//!
//! **Push-key rotation (§6.6).** The phone announces a new push key in
//! `identity-rotate`; the channel endpoint re-pins it in the store
//! (`E2eStore::record_rotation`), and the next seal here reads the new key
//! from `paired_phones()`. The phone keeps its old push private key for 7
//! days to open blobs sealed before the switch.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use dragent_e2ee::codec::b64;
use dragent_e2ee::codec::push_plain::{PushPlain, MAX_BODY_LEN, MAX_DISMISS, MAX_TITLE_LEN};
use dragent_e2ee::noise::{PushPrologue, StaticKeypair};
use dragent_e2ee::seal::push as push_seal;
use dragent_e2ee::{fill_random, Id};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::e2e_store::PairedPhone;
use crate::notifications::Kind;

/// Sent ids are forgotten after this (the phone drops older pushes too).
pub const LEDGER_TTL_MS: u64 = 24 * 60 * 60 * 1000;
/// Most sent ids remembered per phone (oldest forgotten first).
pub const MAX_SENT_PER_PHONE: usize = 256;
/// Android dismissals: at most one per phone per this interval (§8 step 6).
pub const DISMISS_INTERVAL_MS: u64 = 5 * 60 * 1000;
/// How often the worker checks for handled notifications.
const TICK: Duration = Duration::from_secs(30);

/// The nil id: "not about a particular workspace or tab".
const NIL: Id = Id([0; 16]);

/// The `kind` byte in the plaintext.
pub fn kind_code(kind: Kind) -> u8 {
    match kind {
        Kind::AgentFinished => 1,
        Kind::AgentNeedsYou => 2,
        Kind::AgentError => 3,
        Kind::Automation => 4,
    }
}

/// The phone's platform, as it reports it in `notifications.done`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhonePlatform {
    Ios,
    Android,
}

/// What a seal needs: this desktop's keys, the account and the phones.
pub struct Sealer<'a> {
    pub identity: &'a StaticKeypair,
    pub desktop: Id,
    pub account: Id,
    pub phones: Vec<PairedPhone>,
}

/// One sealed push, ready for `send_host_notify`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub phone: Id,
    pub blob: String,
}

/// `base64url(SHA-256(blob)[0..16])` over the base64url string (22 chars),
/// as the push gateway computes `apns-collapse-id`.
pub fn collapse_id(blob: &str) -> String {
    let digest = Sha256::digest(blob.as_bytes());
    b64::encode(&digest[..16])
}

/// Cut `text` to at most `max` bytes on a grapheme boundary, marking the cut
/// with `…`. No grapheme crate is available here, so this backs off over
/// combining marks, variation selectors, joiners, skin-tone modifiers, tag
/// characters and split regional-indicator pairs, which covers the
/// extended grapheme clusters titles and bodies contain in practice.
pub fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    const ELLIPSIS: &str = "…";
    let budget = max.saturating_sub(ELLIPSIS.len());
    let mut end = budget.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    loop {
        if end == 0 {
            break;
        }
        let next = text[end..].chars().next();
        let prev = text[..end].chars().next_back();
        let joins = next.is_some_and(extends) || prev == Some('\u{200D}');
        let splits_flag = next.is_some_and(regional) && {
            let run = text[..end].chars().rev().take_while(|c| regional(*c)).count();
            run % 2 == 1
        };
        if !(joins || splits_flag) {
            break;
        }
        end -= prev.map_or(1, char::len_utf8);
    }
    let mut out = text[..end].trim_end().to_string();
    out.push_str(ELLIPSIS);
    out
}

/// A character that continues the grapheme before it.
fn extends(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x0610..=0x061A
        | 0x064B..=0x065F | 0x0900..=0x0903 | 0x093A..=0x094F | 0x0E31 | 0x0E34..=0x0E3A
        | 0x0E47..=0x0E4E | 0x1160..=0x11FF | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF
        | 0x200C | 0x200D | 0x20D0..=0x20FF | 0x302A..=0x302F | 0x3099 | 0x309A
        | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F
        | 0xE0100..=0xE01EF)
}

fn regional(c: char) -> bool {
    matches!(c as u32, 0x1F1E6..=0x1F1FF)
}

fn random_id() -> Option<[u8; 16]> {
    let mut id = [0u8; 16];
    fill_random(&mut id).ok()?;
    Some(id)
}

/// Seal one plaintext for one phone. `None` when the phone has no push key
/// or sealing fails: nothing is sent (never plaintext).
fn seal_for(sealer: &Sealer<'_>, phone: &PairedPhone, plain: &PushPlain) -> Option<String> {
    let push_key = phone.push.as_ref()?;
    let bytes = plain.encode().ok()?;
    let prologue = PushPrologue {
        account: sealer.account,
        phone: phone.id,
    };
    let sealed = push_seal::seal(sealer.identity, push_key, &prologue, &bytes).ok()?;
    Some(b64::encode(&sealed))
}

#[derive(Debug, Clone)]
struct Sent {
    id: [u8; 16],
    collapse: String,
    sent_ms: u64,
}

#[derive(Debug, Default)]
struct PhoneLedger {
    /// Sent and not yet handled.
    open: VecDeque<Sent>,
    /// Handled (reported by `notifications.done` until they expire).
    done: VecDeque<Sent>,
    /// Handled ids an Android phone hasn't been sent a dismissal for.
    undismissed: VecDeque<[u8; 16]>,
    last_dismiss_ms: Option<u64>,
}

impl PhoneLedger {
    fn prune(&mut self, now_ms: u64) {
        let fresh = |s: &Sent| now_ms.saturating_sub(s.sent_ms) < LEDGER_TTL_MS;
        self.open.retain(fresh);
        self.done.retain(fresh);
        while self.open.len() > MAX_SENT_PER_PHONE {
            self.open.pop_front();
        }
        while self.done.len() > MAX_SENT_PER_PHONE {
            self.done.pop_front();
        }
        while self.undismissed.len() > MAX_SENT_PER_PHONE {
            self.undismissed.pop_front();
        }
    }

    fn is_idle(&self) -> bool {
        self.open.is_empty() && self.done.is_empty() && self.undismissed.is_empty()
    }
}

/// Why `notifications.done` was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoneError {
    /// Params other than `{}` or `{"platform": "ios"|"android"}`.
    Invalid,
}

/// The in-memory push state: sent ids per phone and phone platforms.
#[derive(Debug, Default)]
pub struct Pusher {
    phones: HashMap<Id, PhoneLedger>,
    platforms: HashMap<Id, PhonePlatform>,
}

impl Pusher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seal one notification for every paired phone with a push key.
    pub fn notify(
        &mut self,
        sealer: &Sealer<'_>,
        kind: u8,
        title: &str,
        body: &str,
        now_ms: u64,
    ) -> Vec<Outgoing> {
        self.notify_about(sealer, kind, title, body, NIL, now_ms)
    }

    /// …about one tab: `tab` is an agent's pane id, which the phone sends
    /// back to answer it from the notification (`agent.answer`).
    pub fn notify_about(
        &mut self,
        sealer: &Sealer<'_>,
        kind: u8,
        title: &str,
        body: &str,
        tab: Id,
        now_ms: u64,
    ) -> Vec<Outgoing> {
        let title = cut(title, MAX_TITLE_LEN);
        let body = cut(body, MAX_BODY_LEN);
        let mut out = Vec::new();
        for phone in &sealer.phones {
            // A fresh random id per phone: ids never link two phones' pushes.
            let Some(id) = random_id() else { continue };
            let Ok(plain) =
                PushPlain::notification(sealer.desktop, id, now_ms, kind, &title, &body, NIL, tab)
            else {
                continue;
            };
            let Some(blob) = seal_for(sealer, phone, &plain) else {
                continue;
            };
            let ledger = self.phones.entry(phone.id).or_default();
            ledger.open.push_back(Sent {
                id,
                collapse: collapse_id(&blob),
                sent_ms: now_ms,
            });
            ledger.prune(now_ms);
            out.push(Outgoing {
                phone: phone.id,
                blob,
            });
        }
        out
    }

    /// Everything sent before `last_active_ms` is handled.
    pub fn mark_handled(&mut self, last_active_ms: Option<u64>, now_ms: u64) {
        let Some(active) = last_active_ms else { return };
        for (phone, ledger) in self.phones.iter_mut() {
            let android = self.platforms.get(phone) == Some(&PhonePlatform::Android);
            while ledger.open.front().is_some_and(|s| s.sent_ms < active) {
                let sent = ledger.open.pop_front().expect("front checked");
                if android {
                    ledger.undismissed.push_back(sent.id);
                }
                ledger.done.push_back(sent);
            }
            ledger.prune(now_ms);
        }
    }

    /// Android dismissals that are due (§8 step 6). iOS phones and phones of
    /// unknown platform get none.
    pub fn due_dismissals(&mut self, sealer: &Sealer<'_>, now_ms: u64) -> Vec<Outgoing> {
        let mut out = Vec::new();
        for phone in &sealer.phones {
            if self.platforms.get(&phone.id) != Some(&PhonePlatform::Android) {
                continue;
            }
            let Some(ledger) = self.phones.get_mut(&phone.id) else { continue };
            if ledger.undismissed.is_empty() {
                continue;
            }
            if ledger
                .last_dismiss_ms
                .is_some_and(|last| now_ms.saturating_sub(last) < DISMISS_INTERVAL_MS)
            {
                continue;
            }
            let take = ledger.undismissed.len().min(MAX_DISMISS);
            let ids: Vec<[u8; 16]> = ledger.undismissed.iter().take(take).copied().collect();
            let Some(id) = random_id() else { continue };
            let Ok(plain) = PushPlain::dismissal(sealer.desktop, id, now_ms, 0, NIL, NIL, ids)
            else {
                continue;
            };
            let Some(blob) = seal_for(sealer, phone, &plain) else {
                continue;
            };
            ledger.undismissed.drain(..take);
            ledger.last_dismiss_ms = Some(now_ms);
            out.push(Outgoing {
                phone: phone.id,
                blob,
            });
        }
        out
    }

    /// Forget phones that are no longer paired (revoked or reset).
    pub fn retain_phones(&mut self, paired: &[Id]) {
        self.phones.retain(|id, _| paired.contains(id));
        self.platforms.retain(|id, _| paired.contains(id));
    }

    /// `notifications.done` inside the KK channel: params `{}` or
    /// `{"platform": "ios"|"android"}`. Returns the handled notifications
    /// for this phone: `{"ids": [collapse id…], "tags": [32-hex id…]}`
    /// (iOS removes by collapse id, Android cancels by tag). The phone has
    /// now reconciled, so pending Android dismissals are dropped.
    pub fn serve_done(
        &mut self,
        phone: Id,
        params: &Value,
        last_active_ms: Option<u64>,
        now_ms: u64,
    ) -> Result<Value, DoneError> {
        let obj = params.as_object().ok_or(DoneError::Invalid)?;
        if obj.keys().any(|k| k != "platform") {
            return Err(DoneError::Invalid);
        }
        match obj.get("platform") {
            None => {}
            Some(Value::String(p)) if p == "ios" => {
                self.platforms.insert(phone, PhonePlatform::Ios);
            }
            Some(Value::String(p)) if p == "android" => {
                self.platforms.insert(phone, PhonePlatform::Android);
            }
            Some(_) => return Err(DoneError::Invalid),
        }
        self.mark_handled(last_active_ms, now_ms);
        let Some(ledger) = self.phones.get_mut(&phone) else {
            return Ok(json!({"ids": [], "tags": []}));
        };
        ledger.prune(now_ms);
        ledger.undismissed.clear();
        let ids: Vec<&str> = ledger.done.iter().map(|s| s.collapse.as_str()).collect();
        let tags: Vec<String> = ledger.done.iter().map(|s| hex::encode(s.id)).collect();
        let reply = json!({"ids": ids, "tags": tags});
        if ledger.is_idle() {
            self.phones.remove(&phone);
        }
        Ok(reply)
    }

    fn has_work(&self) -> bool {
        self.phones.values().any(|l| !l.open.is_empty() || !l.undismissed.is_empty())
    }
}

// ── The live wiring ──────────────────────────────────────────────────────────

fn pusher() -> &'static Mutex<Pusher> {
    static PUSHER: OnceLock<Mutex<Pusher>> = OnceLock::new();
    PUSHER.get_or_init(|| Mutex::new(Pusher::new()))
}

/// Last click or key press in a Dragent window, in ms (`None`: never).
fn last_active_ms() -> Option<u64> {
    let secs = warpui::App::last_active_timestamp();
    // Second resolution: count a press in the same second as not after.
    (secs > 0).then(|| (secs as u64 + 1) * 1000)
}

struct Job {
    user_id: String,
    kind: u8,
    title: String,
    body: String,
    /// The agent pane it's about, when it can be answered from the phone.
    tab: Id,
}

/// Seal and queue pushes with this process's keys. Nothing happens if phone
/// access is off or the account id isn't canonical: never plaintext.
fn with_sealer(user_id: &str, f: impl FnOnce(&Sealer<'_>)) {
    let Ok(desktop) = super::e2e_store::desktop() else { return };
    let Ok(account) = Id::parse(user_id) else { return };
    let phones = desktop.store().paired_phones();
    let identity = desktop.identity();
    f(&Sealer {
        identity: &identity,
        desktop: desktop.device(),
        account,
        phones,
    });
}

fn send_all(out: Vec<Outgoing>) {
    for o in out {
        if super::companion_host::send_host_notify(&o.phone.to_string(), &o.blob).is_err() {
            log::warn!("[e2e-notify] a sealed push was refused by the host queue");
        }
    }
}

/// The account the last notification was sealed for (for the tick).
static LAST_USER: Mutex<Option<String>> = Mutex::new(None);

fn tick() {
    let now = super::e2e_store::wall_ms();
    let user = LAST_USER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(user) = user else { return };
    with_sealer(&user, |sealer| {
        let mut p = pusher().lock().unwrap_or_else(|e| e.into_inner());
        let paired: Vec<Id> = sealer.phones.iter().map(|p| p.id).collect();
        p.retain_phones(&paired);
        p.mark_handled(last_active_ms(), now);
        let out = p.due_dismissals(sealer, now);
        drop(p);
        send_all(out);
    });
}

fn worker() -> &'static Mutex<mpsc::Sender<Job>> {
    static WORKER: OnceLock<Mutex<mpsc::Sender<Job>>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name("dragent-e2e-notify".into())
            .spawn(move || loop {
                match rx.recv_timeout(TICK) {
                    Ok(job) => {
                        let now = super::e2e_store::wall_ms();
                        *LAST_USER.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(job.user_id.clone());
                        with_sealer(&job.user_id, |sealer| {
                            let mut p = pusher().lock().unwrap_or_else(|e| e.into_inner());
                            p.mark_handled(last_active_ms(), now);
                            let out = p.notify_about(sealer, job.kind, &job.title, &job.body, job.tab, now);
                            drop(p);
                            send_all(out);
                        });
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let busy = pusher()
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .has_work();
                        if busy {
                            tick();
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            });
        if spawned.is_err() {
            log::warn!("[e2e-notify] couldn't start the push worker");
        }
        Mutex::new(tx)
    })
}

/// Called by `notifications::notify()` after Settings allowed `kind`.
/// Never blocks the UI thread: the keychain read and the seals run on the
/// push worker.
pub fn notify(user_id: &str, kind: Kind, title: &str, body: &str) {
    notify_for_pane(user_id, kind, title, body, None);
}

/// [`notify`] about an agent's pane, so the phone can answer it from the
/// notification (only an agent waiting on a permission prompt is answered).
pub fn notify_for_pane(user_id: &str, kind: Kind, title: &str, body: &str, pane: Option<uuid::Uuid>) {
    if user_id.is_empty() {
        return;
    }
    let job = Job {
        user_id: user_id.to_string(),
        kind: kind_code(kind),
        title: title.to_string(),
        body: body.to_string(),
        tab: pane.map_or(NIL, |pane| Id(*pane.as_bytes())),
    };
    let _ = worker()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .send(job);
}

/// `notifications.done` for the host's method table (§8 step 6).
pub fn serve_done(phone: Id, params: &Value) -> Result<Value, DoneError> {
    pusher()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .serve_done(phone, params, last_active_ms(), super::e2e_store::wall_ms())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_e2ee::codec::push_plain;
    use dragent_e2ee::noise::PublicKey;

    const ACCOUNT: Id = Id([0x11; 16]);
    const DESKTOP: Id = Id([0x22; 16]);
    const T0: u64 = 1_800_000_000_000;

    struct Phone {
        id: Id,
        push: StaticKeypair,
    }

    fn phone(n: u8) -> Phone {
        Phone {
            id: Id([n; 16]),
            push: StaticKeypair::generate().unwrap(),
        }
    }

    fn paired(p: &Phone) -> PairedPhone {
        PairedPhone {
            id: p.id,
            identity: StaticKeypair::generate().unwrap().public(),
            push: Some(p.push.public()),
            name: "Phone".into(),
            pinned_at_ms: T0,
        }
    }

    fn open(p: &Phone, blob: &str) -> (PublicKey, push_plain::PushPlain) {
        let bytes = b64::decode(blob).unwrap();
        let pro = PushPrologue {
            account: ACCOUNT,
            phone: p.id,
        };
        let (sender, plain) = push_seal::open(&p.push, &pro, &bytes).unwrap();
        (sender, push_plain::decode(&plain[..]).unwrap())
    }

    #[test]
    fn every_paired_phone_gets_a_fixed_size_seal_it_alone_can_open() {
        let identity = StaticKeypair::generate().unwrap();
        let (a, b) = (phone(1), phone(2));
        let mut no_push = paired(&phone(3));
        no_push.push = None;
        let sealer = Sealer {
            identity: &identity,
            desktop: DESKTOP,
            account: ACCOUNT,
            phones: vec![paired(&a), paired(&b), no_push],
        };
        let mut p = Pusher::new();
        let out = p.notify(&sealer, 2, "Your agents need you", "The lead asks you", T0);
        // The phone without a push key gets nothing (never plaintext).
        assert_eq!(out.len(), 2);
        for o in &out {
            assert_eq!(o.blob.len(), 1494);
            assert!(!o.blob.contains("agents"));
        }
        let (sender, plain) = open(&a, &out[0].blob);
        assert_eq!(sender, identity.public());
        assert_eq!(plain.desktop_id, DESKTOP);
        assert_eq!((plain.kind, plain.dismiss), (2, false));
        assert_eq!(plain.title, "Your agents need you");
        assert_eq!(plain.body, "The lead asks you");
        assert_eq!(plain.issued_at_ms, T0);
        assert_eq!((plain.workspace_id, plain.tab_id), (NIL, NIL));
        // Phone b's blob doesn't open with a's key or a's prologue.
        let bytes = b64::decode(&out[1].blob).unwrap();
        let pro_a = PushPrologue { account: ACCOUNT, phone: a.id };
        assert!(push_seal::open(&a.push, &pro_a, &bytes).is_err());
        let (_, plain_b) = open(&b, &out[1].blob);
        assert_ne!(plain_b.id, plain.id, "ids never link two phones' pushes");
    }

    #[test]
    fn over_long_text_is_cut_to_fit_and_still_seals() {
        let identity = StaticKeypair::generate().unwrap();
        let a = phone(1);
        let sealer = Sealer {
            identity: &identity,
            desktop: DESKTOP,
            account: ACCOUNT,
            phones: vec![paired(&a)],
        };
        let mut p = Pusher::new();
        let title = "é".repeat(400);
        let body = "👍🏽".repeat(300);
        let out = p.notify(&sealer, 1, &title, &body, T0);
        let (_, plain) = open(&a, &out[0].blob);
        assert!(plain.title.len() <= MAX_TITLE_LEN && plain.title.ends_with('…'));
        assert!(plain.body.len() <= MAX_BODY_LEN && plain.body.ends_with('…'));
        // No skin tone split from its thumb.
        assert!(plain.body.trim_end_matches('…').ends_with("👍🏽"));
    }

    #[test]
    fn cut_keeps_graphemes_whole() {
        assert_eq!(cut("short", 10), "short");
        // e + combining acute is one grapheme.
        let s = "ab\u{0065}\u{0301}cdef";
        let c = cut(s, 6); // budget 3 bytes: "ab" + "e" would split é
        assert_eq!(c, "ab…");
        // A flag is two regional indicators.
        let flags = "🇬🇧🇫🇷🇩🇪";
        let c = cut(flags, 15); // budget 12: 1.5 flags → 1 flag
        assert_eq!(c, "🇬🇧…");
        // ZWJ family stays whole or goes.
        let fam = "x👨\u{200D}👩\u{200D}👧";
        let c = cut(fam, 12);
        assert_eq!(c, "x…");
        assert!(cut(&"a".repeat(300), 240).len() <= 240);
    }

    #[test]
    fn collapse_id_matches_the_gateway() {
        // Go: base64.RawURLEncoding(sha256([]byte(blob))[:16]).
        let blob = "A".repeat(1494);
        let id = collapse_id(&blob);
        assert_eq!(id.len(), 22);
        let digest = Sha256::digest(blob.as_bytes());
        assert_eq!(b64::decode(&id).unwrap(), digest[..16].to_vec());
    }

    fn setup() -> (StaticKeypair, Phone, Phone) {
        (StaticKeypair::generate().unwrap(), phone(1), phone(2))
    }

    #[test]
    fn ios_reconciles_on_open_and_gets_no_dismissal_pushes() {
        let (identity, ios, _) = setup();
        let sealer = Sealer {
            identity: &identity,
            desktop: DESKTOP,
            account: ACCOUNT,
            phones: vec![paired(&ios)],
        };
        let mut p = Pusher::new();
        let out = p.notify(&sealer, 2, "t", "b", T0);
        let collapse = collapse_id(&out[0].blob);
        // Nothing handled yet.
        let r = p.serve_done(ios.id, &json!({"platform": "ios"}), Some(T0 - 1), T0 + 10).unwrap();
        assert_eq!(r["ids"], json!([]));
        // The person clicks in Dragent afterwards.
        p.mark_handled(Some(T0 + 5_000), T0 + 6_000);
        assert!(p.due_dismissals(&sealer, T0 + 10 * 60_000).is_empty());
        let r = p.serve_done(ios.id, &json!({}), None, T0 + 7_000).unwrap();
        assert_eq!(r["ids"], json!([collapse]));
        assert_eq!(r["tags"].as_array().unwrap().len(), 1);
        // Expired after 24 h.
        let r = p.serve_done(ios.id, &json!({}), None, T0 + LEDGER_TTL_MS).unwrap();
        assert_eq!(r["ids"], json!([]));
    }

    #[test]
    fn unknown_platform_gets_no_dismissal_pushes() {
        let (identity, a, _) = setup();
        let sealer = Sealer {
            identity: &identity,
            desktop: DESKTOP,
            account: ACCOUNT,
            phones: vec![paired(&a)],
        };
        let mut p = Pusher::new();
        p.notify(&sealer, 2, "t", "b", T0);
        p.mark_handled(Some(T0 + 1), T0 + 2);
        assert!(p.due_dismissals(&sealer, T0 + 3).is_empty());
    }

    #[test]
    fn android_dismissals_are_sealed_same_size_and_rate_limited() {
        let (identity, droid, _) = setup();
        let sealer = Sealer {
            identity: &identity,
            desktop: DESKTOP,
            account: ACCOUNT,
            phones: vec![paired(&droid)],
        };
        let mut p = Pusher::new();
        p.serve_done(droid.id, &json!({"platform": "android"}), None, T0).unwrap();
        let first = p.notify(&sealer, 2, "t", "b", T0);
        let (_, n1) = open(&droid, &first[0].blob);
        p.mark_handled(Some(T0 + 1_000), T0 + 1_000);
        let d = p.due_dismissals(&sealer, T0 + 2_000);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].blob.len(), first[0].blob.len());
        let (sender, dismissal) = open(&droid, &d[0].blob);
        assert_eq!(sender, identity.public());
        assert!(dismissal.dismiss && dismissal.title.is_empty() && dismissal.body.is_empty());
        assert_eq!(dismissal.dismiss_ids, vec![n1.id]);
        // A second handled notification within 5 minutes waits.
        p.notify(&sealer, 2, "t", "b", T0 + 3_000);
        p.mark_handled(Some(T0 + 4_000), T0 + 4_000);
        assert!(p.due_dismissals(&sealer, T0 + 2_000 + DISMISS_INTERVAL_MS - 1).is_empty());
        assert_eq!(p.due_dismissals(&sealer, T0 + 2_000 + DISMISS_INTERVAL_MS).len(), 1);
    }

    #[test]
    fn android_dismissals_carry_at_most_58_ids_and_open_reconciles_the_rest() {
        let (identity, droid, _) = setup();
        let sealer = Sealer {
            identity: &identity,
            desktop: DESKTOP,
            account: ACCOUNT,
            phones: vec![paired(&droid)],
        };
        let mut p = Pusher::new();
        p.serve_done(droid.id, &json!({"platform": "android"}), None, T0).unwrap();
        for i in 0..70 {
            p.notify(&sealer, 1, "t", "b", T0 + i);
        }
        p.mark_handled(Some(T0 + 100), T0 + 100);
        let d = p.due_dismissals(&sealer, T0 + 200);
        let (_, dismissal) = open(&droid, &d[0].blob);
        assert_eq!(dismissal.dismiss_ids.len(), MAX_DISMISS);
        // The app opens: everything is reconciled, nothing left to dismiss.
        let r = p.serve_done(droid.id, &json!({"platform": "android"}), None, T0 + 300).unwrap();
        assert_eq!(r["tags"].as_array().unwrap().len(), 70);
        assert!(p
            .due_dismissals(&sealer, T0 + 300 + DISMISS_INTERVAL_MS)
            .is_empty());
    }

    #[test]
    fn done_params_are_strict_and_unpaired_phones_are_forgotten() {
        let (identity, a, b) = setup();
        let mut p = Pusher::new();
        for bad in [json!(null), json!([]), json!({"x": 1}), json!({"platform": "web"}), json!({"platform": 1})] {
            assert_eq!(p.serve_done(a.id, &bad, None, T0), Err(DoneError::Invalid), "{bad}");
        }
        let sealer = Sealer {
            identity: &identity,
            desktop: DESKTOP,
            account: ACCOUNT,
            phones: vec![paired(&a), paired(&b)],
        };
        p.notify(&sealer, 1, "t", "b", T0);
        p.retain_phones(&[b.id]);
        p.mark_handled(Some(T0 + 1), T0 + 1);
        assert_eq!(p.serve_done(a.id, &json!({}), None, T0 + 2).unwrap()["ids"], json!([]));
        assert_eq!(p.serve_done(b.id, &json!({}), None, T0 + 2).unwrap()["ids"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn the_ledger_is_bounded() {
        let (identity, a, _) = setup();
        let sealer = Sealer {
            identity: &identity,
            desktop: DESKTOP,
            account: ACCOUNT,
            phones: vec![paired(&a)],
        };
        let mut p = Pusher::new();
        for i in 0..(MAX_SENT_PER_PHONE as u64 + 20) {
            p.notify(&sealer, 1, "t", "b", T0 + i);
        }
        p.mark_handled(Some(T0 + 10_000), T0 + 10_000);
        let r = p.serve_done(a.id, &json!({}), None, T0 + 10_000).unwrap();
        assert_eq!(r["ids"].as_array().unwrap().len(), MAX_SENT_PER_PHONE);
    }

    /// §6.6 with the real store: a push-only `identity-rotate` (new_pub =
    /// the current identity) re-pins the push key, keeps the identity pin,
    /// and the next seal opens only with the new push key.
    #[test]
    fn a_push_key_rotation_repins_push_and_keeps_the_identity_pin() {
        use crate::realtime::e2e_keys::MemorySecretStore;
        use crate::realtime::e2e_store::tests::TempHome;
        use crate::realtime::e2e_store::E2eDesktop;
        use std::sync::Arc;
        let home = TempHome::new("e9a_rotate");
        let d = E2eDesktop::open(&home.0, DESKTOP, Arc::new(MemorySecretStore::new()), T0).unwrap();
        let identity_key = StaticKeypair::generate().unwrap().public();
        let (old, new) = (phone(5), StaticKeypair::generate().unwrap());
        d.store()
            .pin_phone(old.id, identity_key, old.push.public(), "Pixel".into(), T0)
            .unwrap();
        d.store()
            .record_rotation(&old.id, identity_key, Some(new.public()), T0 + 1)
            .unwrap();
        let pinned = d.store().phone(&old.id).unwrap();
        assert_eq!(pinned.identity, identity_key, "identity pin kept");
        assert_eq!(pinned.push, Some(new.public()), "push key re-pinned");
        assert_eq!(d.store().candidate_keys(&old.id, T0 + 2), vec![identity_key], "no identity rotation pending");
        let identity = d.identity();
        let sealer = Sealer {
            identity: &identity,
            desktop: d.device(),
            account: ACCOUNT,
            phones: d.store().paired_phones(),
        };
        let out = Pusher::new().notify(&sealer, 2, "t", "b", T0 + 3);
        let bytes = b64::decode(&out[0].blob).unwrap();
        let pro = PushPrologue { account: ACCOUNT, phone: old.id };
        assert!(push_seal::open(&old.push, &pro, &bytes).is_err(), "old push key no longer used");
        let (sender, _) = push_seal::open(&new, &pro, &bytes).unwrap();
        assert_eq!(sender, d.public());
    }

    #[test]
    fn kinds_are_stable() {
        assert_eq!(
            [Kind::AgentFinished, Kind::AgentNeedsYou, Kind::AgentError, Kind::Automation].map(kind_code),
            [1, 2, 3, 4]
        );
    }
}
