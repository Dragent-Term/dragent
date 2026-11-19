//! Device-local review drafts. The organisation and exact head remain part of every saved state.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Draft {
    pub body: String,
    pub pending: Vec<LineComment>,
    pub head: String,
    pub line: Option<LineComment>,
    pub reply: Option<u64>,
    pub kind: ReviewKind,
}
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    updated_at: u64,
    #[serde(default)]
    organisation: String,
    #[serde(default)]
    viewed: HashSet<String>,
    #[serde(default)]
    drafts: HashMap<String, Draft>,
}
impl PullRequestsView {
    pub(super) fn load_local(&mut self) {
        if self.preview {
            return;
        }
        let path = dragent_paths::organisation_dir().join("pull-request-reviews.json");
        let data = std::fs::metadata(&path)
            .ok()
            .filter(|m| m.len() <= 8 * 1024 * 1024)
            .and_then(|_| std::fs::read(path).ok())
            .and_then(|d| serde_json::from_slice::<Saved>(&d).ok());
        if let Some(saved) = data.filter(|s| s.organisation == self.scope) {
            self.viewed = saved.viewed;
            self.review_drafts = saved.drafts;
        }
    }
    pub(super) fn remember_draft(&mut self) {
        if let Some(key) = self.selected.clone() {
            self.review_drafts.insert(
                key,
                Draft {
                    body: self.body.clone(),
                    pending: self.pending.clone(),
                    head: self.pending_head.clone(),
                    line: self.line.clone(),
                    reply: self.reply,
                    kind: self.review,
                },
            );
        }
    }
    pub(super) fn restore_draft(&mut self, key: &str) {
        if let Some(d) = self.review_drafts.get(key) {
            self.body = d.body.clone();
            self.pending = d.pending.clone();
            self.pending_head = d.head.clone();
            self.line = d.line.clone();
            self.reply = d.reply;
            self.review = d.kind;
        }
    }
    pub(super) fn save_local(&mut self, ctx: &mut ViewContext<Self>) {
        if self.preview || !self.valid() || self.form.is_some() {
            return;
        }
        self.remember_draft();
        self.draft_generation += 1;
        let generation = self.draft_generation;
        ctx.spawn(
            async { warpui::r#async::Timer::after(Duration::from_millis(900)).await },
            move |v, _, ctx| {
                if v.valid() && generation == v.draft_generation && v.form.is_none() {
                    v.flush_local(ctx);
                }
            },
        );
    }
    pub(super) fn flush_local(&mut self, ctx: &mut ViewContext<Self>) {
        if self.preview || !self.valid() {
            return;
        }
        self.remember_draft();
        let state = Saved {
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            organisation: self.scope.clone(),
            viewed: self.viewed.clone(),
            drafts: self.review_drafts.clone(),
        };
        let path = dragent_paths::organisation_dir().join("pull-request-reviews.json");
        let scope = self.scope.clone();
        ctx.spawn(
            async move { write_saved(path, state) },
            move |v, result, ctx| {
                if v.scope == scope && v.valid() {
                    if let Err(e) = result {
                        ctx.emit(Event::Error(format!(
                            "Couldn't save the local review draft: {e}"
                        )))
                    }
                }
            },
        );
    }
}
fn write_saved(path: PathBuf, state: Saved) -> Result<(), String> {
    static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = WRITER.lock().map_err(|e| e.to_string())?;
    if std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Saved>(&b).ok())
        .is_some_and(|old| {
            old.organisation == state.organisation && old.updated_at > state.updated_at
        })
    {
        return Ok(());
    }
    let bytes = serde_json::to_vec(&state).map_err(|e| e.to_string())?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(
            "Local review drafts exceed 8 MB. Submit or remove older drafts before saving more."
                .into(),
        );
    }
    let parent = path.parent().ok_or("Missing review state directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = parent.join(format!(".pr-reviews-{}.tmp", uuid::Uuid::new_v4()));
    let result = std::fs::write(&temp, bytes).and_then(|_| std::fs::rename(&temp, &path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn older_background_writes_do_not_replace_newer_drafts() {
        let root = std::env::temp_dir().join(format!("dragent-pr-state-{}", uuid::Uuid::new_v4()));
        let path = root.join("reviews.json");
        let current = Saved {
            updated_at: 20,
            organisation: "org-a".into(),
            ..Saved::default()
        };
        write_saved(path.clone(), current).unwrap();
        let older = Saved {
            updated_at: 10,
            organisation: "org-a".into(),
            ..Saved::default()
        };
        write_saved(path.clone(), older).unwrap();
        let loaded: Saved = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(loaded.updated_at, 20);
        assert_eq!(loaded.organisation, "org-a");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn saved_reviews_keep_identity_head_and_line_coordinates() {
        let draft = Draft {
            body: "Review summary".into(),
            pending: vec![LineComment {
                path: "src/a.rs".into(),
                line: 9,
                side: "RIGHT".into(),
                body: "Please check the expiry".into(),
                start_line: Some(7),
                start_side: Some("RIGHT".into()),
            }],
            head: "a".repeat(40),
            line: None,
            reply: Some(42),
            kind: ReviewKind::RequestChanges,
        };
        let saved = Saved {
            updated_at: 1,
            organisation: "org-a".into(),
            viewed: HashSet::from(["repo#1:head:path".into()]),
            drafts: HashMap::from([("repo#1".into(), draft)]),
        };
        let decoded: Saved = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(decoded.organisation, "org-a");
        let draft = &decoded.drafts["repo#1"];
        assert_eq!(draft.head, "a".repeat(40));
        assert_eq!(draft.pending[0].start_line, Some(7));
        assert_eq!(draft.kind, ReviewKind::RequestChanges);
    }
}
