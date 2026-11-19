//! VoiceFold — the append-only dictation contract, as pure testable logic.
//!
//! Rules (user-defined):
//!   1. Speech converts to text directly in the input field.
//!   2. Text only ever APPENDS while recording. A pause must never delete or
//!      restart what is already there — every spoken word is kept.
//!   3. Submit clears the input AND the transcription state; speech after a
//!      submit starts fresh (rules 1–2 apply again).
//!
//! The engine sends cumulative snapshots of everything heard this session.
//! Snapshots normally grow, but two things can invalidate naive rendering:
//!   • the editor changes underneath us (typing, inserted file path, submit
//!     clearing the input) — we must not clobber it;
//!   • the recognizer regresses after a pause (loses its transcript prefix
//!     and "starts over") — we must not let rendered words vanish.
//! VoiceFold handles both: external edits REBASE (adopt the editor content,
//! stop re-appending what was already rendered), and engine regressions
//! FREEZE (rendered text is absorbed into the base before accepting the new,
//! shorter snapshot).

/// Byte length of the longest common prefix, floored to a char boundary.
fn common_prefix(a: &str, b: &str) -> usize {
    let mut n = a
        .as_bytes()
        .iter()
        .zip(b.as_bytes())
        .take_while(|(x, y)| x == y)
        .count();
    while n > 0 && !a.is_char_boundary(n) {
        n -= 1;
    }
    n
}

/// Join two fragments with a single space (either side may be empty).
fn join(a: &str, b: &str) -> String {
    if b.is_empty() {
        a.to_string()
    } else if a.is_empty() {
        b.to_string()
    } else if a.ends_with(char::is_whitespace) {
        format!("{a}{b}")
    } else {
        format!("{a} {b}")
    }
}

/// Mid-utterance revisions legitimately shrink a snapshot by a few characters
/// ("I see them" → "I seem"); anything shrinking past this slack is treated as
/// a recognizer regression and triggers a freeze instead of a rewrite.
const REVISION_SLACK: usize = 8;

/// Case- and punctuation-free view of a transcript, used ONLY to decide whether
/// a snapshot revises what is rendered or replaces it.
///
/// Engines finalize an utterance by re-punctuating and capitalizing it: WinRT
/// streams the hypothesis "Hey how are you", then finalizes the very same words
/// as "Hey, how are you?". Compared raw, those diverge at the inserted comma, so
/// the shared prefix is just "Hey" — short enough to look like the recognizer
/// lost its transcript. The fold would then freeze the hypothesis into the base
/// and append the final after it, writing every utterance twice. Normalizing
/// first makes the two forms identical, so a finalization reads as the revision
/// it is while genuinely new speech still has no prefix in common.
fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

#[derive(Default)]
pub struct VoiceFold {
    /// Snapshot of the review preference when this recording started.
    confirm_before_insert: bool,
    /// Editor text dictation appends to (grows via rebases and freezes).
    base: String,
    /// Engine-snapshot prefix already represented in `base` — set on rebase
    /// so pre-rebase speech is never re-appended.
    consumed: String,
    /// The engine tail currently rendered after `base` (still revisable).
    active: String,
    /// The previous full snapshot — becomes `consumed` on a rebase, so words
    /// spoken between the last render and the rebase are NOT lost.
    last_snapshot: String,
    /// Exactly what we last wrote to the editor; a mismatch means someone
    /// else edited it.
    last_set: Option<String>,
}

impl VoiceFold {
    /// Begin a session over an editor currently containing `editor_text`
    /// (dictation continues after it).
    pub fn start(editor_text: String) -> Self {
        Self {
            base: editor_text,
            ..Default::default()
        }
    }

    pub fn start_with_confirmation(editor_text: String, confirm_before_insert: bool) -> Self {
        Self {
            confirm_before_insert,
            ..Self::start(editor_text)
        }
    }

    pub fn final_disposition(&self, text: String) -> super::FinalDisposition {
        super::final_disposition(self.confirm_before_insert, text)
    }

    /// A review session never writes partial transcripts to the editor.
    /// Leave the fold untouched so confirmation can merge subsequent typing.
    pub fn apply_partial(
        &mut self,
        current: &str,
        snapshot: &str,
        force_rebase: bool,
    ) -> Option<String> {
        if self.confirm_before_insert {
            None
        } else {
            Some(self.apply(current, snapshot, force_rebase))
        }
    }

    /// Fold the next engine snapshot. `current` is the editor content right
    /// now; `force_rebase` marks a submit (rule 3). Returns the full text to
    /// place in the editor.
    pub fn apply(&mut self, current: &str, snapshot: &str, force_rebase: bool) -> String {
        let externally_edited = force_rebase
            || match &self.last_set {
                Some(last) => last != current,
                None => current != self.base,
            };
        if externally_edited {
            // Rule 3 / external edit: keep what's in the editor (possibly
            // nothing, after a submit), and stop re-appending everything
            // rendered up to the LAST snapshot. Words newer than that (spoken
            // in the gap) survive into the fresh text.
            self.base = current.to_string();
            self.consumed = std::mem::take(&mut self.last_snapshot);
            self.active.clear();
        }

        let tail = snapshot[common_prefix(snapshot, &self.consumed)..].trim_start();

        // Rule 2: the recognizer regressed (lost transcript after a pause) —
        // rendered words must not vanish. Freeze them into the base, then
        // accept the new snapshot as fresh speech. Compared on the normalized
        // form so re-punctuating an utterance counts as revising it, not
        // replacing it.
        //
        // What separates a regression from a revision is HOW MUCH of the
        // rendered text the new snapshot still agrees with — not merely that
        // it diverges somewhere.
        //
        // Cloud engines rewrite words they have already streamed as later
        // audio settles the endpointing: Deepgram turns "Did it in the debate"
        // into "You did it in the debate" while the snapshot keeps growing.
        // That diverges mid-way but still shares the whole opening. Treating
        // any divergence as a lost transcript froze the rendered text into the
        // base and appended the full cumulative snapshot after it, so the
        // transcript restarted from the top on every revision and compounded.
        //
        // A true reset shares essentially nothing with what is rendered. So
        // freeze only when the agreement covers less than half of it; past
        // that the snapshot is a revision and must replace, not duplicate.
        let (tail_norm, active_norm) = (normalized(tail), normalized(&self.active));
        let agreed = common_prefix(&tail_norm, &active_norm);
        let beyond_slack = agreed + REVISION_SLACK < active_norm.len();
        let mostly_disagrees = agreed * 2 < active_norm.len();
        if beyond_slack && mostly_disagrees {
            let active = std::mem::take(&mut self.active);
            self.base = join(&self.base, &active);
        }

        self.active = tail.to_string();
        let full = join(&self.base, &self.active);
        self.last_set = Some(full.clone());
        self.last_snapshot = snapshot.to_string();
        full
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_holds_every_partial_and_merges_edits_on_confirmation() {
        let mut fold = VoiceFold::start_with_confirmation("Draft".into(), true);
        assert_eq!(fold.apply_partial("Draft", "hello", false), None);
        assert_eq!(
            fold.apply_partial("Draft typed", "hello world", false),
            None
        );
        let super::super::FinalDisposition::AwaitingConfirm(text) =
            fold.final_disposition("Hello world.".into())
        else {
            panic!("must await review")
        };
        // Confirmation preserves typing that happened during recording or review.
        assert_eq!(
            fold.apply("Draft typed more", &text, false),
            "Draft typed more Hello world."
        );
    }

    #[test]
    fn immediate_dictation_keeps_live_feedback() {
        let mut fold = VoiceFold::start_with_confirmation("Draft".into(), false);
        assert_eq!(
            fold.apply_partial("Draft", "hello", false),
            Some("Draft hello".into())
        );
        assert_eq!(
            fold.final_disposition("Hello.".into()),
            super::super::FinalDisposition::Insert("Hello.".into())
        );
    }

    #[test]
    fn speech_appends_and_grows() {
        let mut f = VoiceFold::start(String::new());
        assert_eq!(f.apply("", "hello", false), "hello");
        assert_eq!(f.apply("hello", "hello world", false), "hello world");
    }

    #[test]
    fn continues_after_preexisting_text() {
        let mut f = VoiceFold::start("fix the bug".into());
        assert_eq!(
            f.apply("fix the bug", "in auth", false),
            "fix the bug in auth"
        );
    }

    #[test]
    fn midutterance_revisions_replace_not_duplicate() {
        let mut f = VoiceFold::start(String::new());
        f.apply("", "I sea", false);
        assert_eq!(
            f.apply("I sea", "I see the point", false),
            "I see the point"
        );
    }

    #[test]
    fn pause_regression_never_deletes() {
        // THE bug: after a pause the engine loses its transcript and sends
        // only the new sentence. Rendered text must survive.
        let mut f = VoiceFold::start(String::new());
        f.apply("", "first sentence here", false);
        let out = f.apply("first sentence here", "second", false);
        assert_eq!(out, "first sentence here second");
        let out = f.apply(&out, "second thing", false);
        assert_eq!(out, "first sentence here second thing");
    }

    #[test]
    fn repeated_regressions_accumulate() {
        let mut f = VoiceFold::start(String::new());
        let mut text = f.apply("", "alpha beta gamma", false);
        text = f.apply(&text, "delta epsilon", false);
        text = f.apply(&text, "zeta eta theta", false);
        assert_eq!(text, "alpha beta gamma delta epsilon zeta eta theta");
    }

    #[test]
    fn external_path_insert_survives() {
        let mut f = VoiceFold::start(String::new());
        f.apply("", "add this image", false);
        // App inserts a file path; next snapshot keeps growing.
        let out = f.apply(
            "add this image '/tmp/shot.png'",
            "add this image please",
            false,
        );
        assert!(out.starts_with("add this image '/tmp/shot.png'"), "{out}");
        assert!(out.ends_with("please"), "{out}");
        assert_eq!(
            out.matches("add this image").count(),
            1,
            "no duplication: {out}"
        );
    }

    #[test]
    fn finalized_punctuation_is_a_revision_not_new_speech() {
        // THE bug: WinRT streams a bare hypothesis ("Hey how are you") and then
        // finalizes the SAME utterance with punctuation and capitalization
        // ("Hey, how are you?"). That is a revision, but a raw-prefix comparison
        // sees the transcripts diverge at the inserted comma, calls it a
        // regression, freezes the hypothesis into the base and appends the final
        // on top — rendering "Hey how are you Hey, how are you?".
        let mut f = VoiceFold::start(String::new());
        let out = f.apply("", "Hey how are you", false);
        assert_eq!(out, "Hey how are you");
        let out = f.apply(&out, "Hey, how are you?", false);
        assert_eq!(out, "Hey, how are you?");
    }

    #[test]
    fn finalized_revision_then_more_speech_still_appends() {
        let mut f = VoiceFold::start(String::new());
        let out = f.apply("", "hey how are you", false);
        let out = f.apply(&out, "Hey, how are you?", false);
        // A genuine pause-regression after the finalized utterance must still
        // append rather than replace.
        let out = f.apply(&out, "what time is it", false);
        assert_eq!(out, "Hey, how are you? what time is it");
    }

    #[test]
    fn submit_clears_and_restarts_fresh() {
        let mut f = VoiceFold::start(String::new());
        f.apply("", "send this message", false);
        // Enter: input cleared, force_rebase marks the submit. Post-send
        // speech starts fresh; the old transcript never reappears.
        let out = f.apply("", "send this message and more", true);
        assert_eq!(out, "and more");
        let out = f.apply("and more", "send this message and more words", false);
        assert_eq!(out, "and more words");
    }

    #[test]
    fn submit_then_engine_reset_stays_fresh() {
        let mut f = VoiceFold::start(String::new());
        f.apply("", "first query", false);
        let out = f.apply("", "first query", true);
        assert_eq!(out, "");
        // Engine also reset after the pause: brand-new snapshot.
        let out = f.apply("", "new question", false);
        assert_eq!(out, "new question");
    }

    /// Regression test for the real Deepgram transcript that repeated itself.
    ///
    /// The cloud engine revises words it has already streamed ("Did it in the
    /// debate" → "You did it in the debate") while the snapshot keeps growing.
    /// Each revision used to look like a lost transcript, so the rendered text
    /// was frozen and the whole cumulative snapshot appended after it — the
    /// transcript restarted from the top every time, compounding.
    #[test]
    fn cloud_revisions_of_already_streamed_words_do_not_duplicate() {
        let mut f = VoiceFold::start(String::new());
        let snapshots = [
            "Never forget. That's what this is. Don't freeze up. What? Did it in the debate. You did it",
            "Never forget. That's what this is. Don't freeze up. What? You did it in the debate. You did one out in labor. Take my",
            "Never forget. That's what this is. Don't freeze up. What? You did it in the debate. You did it when I was in labor. You did when you No. I I did not",
            "Never forget. That's what this is. Don't freeze up. What? You did it in the debate. You did it when I was in labor. You did when you No. I I did not freeze up. You did.",
        ];
        let mut rendered = String::new();
        for snap in snapshots {
            rendered = f.apply(&rendered, snap, false);
        }
        // The rendered text is the LAST snapshot, not an accumulation of all.
        assert_eq!(rendered, snapshots[snapshots.len() - 1]);
        // The tell-tale symptom: the opening phrase must appear exactly once.
        assert_eq!(rendered.matches("Never forget").count(), 1);
    }

    /// The revision guard must not weaken the pause-regression protection:
    /// a genuinely SHORTER snapshot still freezes what was rendered.
    #[test]
    fn shorter_snapshot_after_pause_still_freezes() {
        let mut f = VoiceFold::start(String::new());
        f.apply("", "the quick brown fox jumps over", false);
        let out = f.apply("the quick brown fox jumps over", "a new sentence", false);
        assert_eq!(out, "the quick brown fox jumps over a new sentence");
    }

    /// A growing snapshot that revises its own middle replaces cleanly, even
    /// when the divergence point is early.
    #[test]
    fn growing_snapshot_with_early_divergence_replaces() {
        let mut f = VoiceFold::start(String::new());
        f.apply("", "call the bank tomorrow", false);
        let out = f.apply(
            "call the bank tomorrow",
            "call the bakery tomorrow morning please",
            false,
        );
        assert_eq!(out, "call the bakery tomorrow morning please");
    }
}
