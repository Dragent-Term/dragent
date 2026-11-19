//! Cross-session persistent memory (memdir).
//!
//! Files live at:
//!   `~/.dragent/projects/{hex8(sha256(cwd))}/memory/`
//!
//! Each memory is an individual `.md` file with YAML-style frontmatter:
//! ```
//! ---
//! name: short-slug
//! description: one-line summary used for relevance matching
//! type: user | feedback | project | reference
//! ---
//! body content
//! ```
//!
//! `MEMORY.md` at the root of the directory is an index (max 200 lines).
//! The index and top-5 relevant memory files are injected into the system
//! prompt's dynamic block on every query.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct MemoryMetadata {
    pub source: String,
    pub source_evidence: String,
    pub confidence: f32,
    pub created_at: u64,
    pub updated_at: u64,
    pub expires_at: Option<u64>,
}

impl MemoryMetadata {
    pub fn extracted(source_evidence: impl Into<String>, confidence: f32, kind: &str) -> Self {
        let now = now_secs();
        Self {
            source: "conversation-extraction".to_string(),
            source_evidence: source_evidence.into(),
            confidence: confidence.clamp(0.0, 1.0),
            created_at: now,
            updated_at: now,
            // Project/reference facts decay because code and external resources
            // change. User preferences and explicit corrections remain until
            // contradicted or consolidated.
            expires_at: matches!(kind, "project" | "reference")
                .then_some(now.saturating_add(90 * 24 * 60 * 60)),
        }
    }
}

// ── Store ─────────────────────────────────────────────────────────────────────

pub struct MemoryStore {
    dir: PathBuf,
}

impl MemoryStore {
    pub fn for_cwd(cwd: &Path) -> Self {
        let dir = memory_dir(cwd);
        Self { dir }
    }

    /// Build the system-prompt block for this memory store.
    /// Returns an empty string if there are no memories.
    pub fn to_prompt_block(&self, query: &str) -> String {
        if !self.dir.exists() {
            return String::new();
        }

        let index = self.load_index();
        let relevant = self.load_relevant(query, 5);

        if index.is_empty() && relevant.is_empty() {
            return String::new();
        }

        let mut out = String::from(
            "\n## Persistent memory & standing rules\n\n\
             Learned about this user and project across past sessions. Treat \
             High-confidence `user` and `feedback` entries are standing preferences. \
             Apply them silently unless the current user message contradicts them. \
             Inferred `project` and `reference` entries are leads, not authority: \
             verify them against current code or tools before acting. Newer direct \
             user instructions override every memory.\n\n",
        );

        if !index.is_empty() {
            out.push_str("**Index (MEMORY.md):**\n");
            // Cap at 200 lines
            let capped: String = index.lines().take(200).collect::<Vec<_>>().join("\n");
            out.push_str(&capped);
            out.push_str("\n\n");
        }

        if !relevant.is_empty() {
            out.push_str("**Relevant memories:**\n\n");
            for m in &relevant {
                let standing =
                    matches!(m.kind.as_str(), "user" | "feedback") && m.metadata.confidence >= 0.8;
                out.push_str(&format!(
                    "### {} ({}, source={}, confidence={:.2}, standing={})\n{}\n\n",
                    m.name, m.kind, m.metadata.source, m.metadata.confidence, standing, m.body
                ));
            }
        }

        out
    }

    /// Save or overwrite a memory file. Creates `MEMORY.md` index entry.
    pub fn save(
        &self,
        name: &str,
        description: &str,
        kind: &str,
        body: &str,
    ) -> std::io::Result<()> {
        self.save_with_metadata(
            name,
            description,
            kind,
            body,
            MemoryMetadata {
                source: "legacy-caller".to_string(),
                source_evidence: "provenance unavailable".to_string(),
                confidence: 0.5,
                created_at: now_secs(),
                updated_at: now_secs(),
                expires_at: matches!(kind, "project" | "reference")
                    .then_some(now_secs().saturating_add(90 * 24 * 60 * 60)),
            },
        )
    }

    pub fn save_with_metadata(
        &self,
        name: &str,
        description: &str,
        kind: &str,
        body: &str,
        mut metadata: MemoryMetadata,
    ) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;

        let safe_name = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>();

        let file = self.dir.join(format!("{safe_name}.md"));
        if let Some(previous) = parse_memory_file(&file) {
            metadata.created_at = previous.metadata.created_at;
        }
        metadata.updated_at = now_secs();
        let one_line = |value: &str| value.replace(['\r', '\n'], " ");
        let description = one_line(description);
        let source = one_line(&metadata.source);
        let evidence = one_line(&metadata.source_evidence);
        let kind = match kind {
            "user" | "feedback" | "project" | "reference" => kind,
            _ => "project",
        };
        let expires = metadata.expires_at.unwrap_or(0);
        let content = format!(
            "---\nname: {safe_name}\ndescription: {description}\ntype: {kind}\nsource: {}\nsource_evidence: {}\nconfidence: {:.3}\ncreated_at: {}\nupdated_at: {}\nexpires_at: {}\n---\n\n{body}\n",
            source,
            evidence,
            metadata.confidence,
            metadata.created_at,
            metadata.updated_at,
            expires,
        );
        std::fs::write(&file, &content)?;

        // Rebuild MEMORY.md index
        self.rebuild_index()
    }

    /// Return every memory file in the store (used by dream consolidation).
    pub fn load_all(&self) -> Vec<MemoryFile> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| {
                e.path().extension().map(|x| x == "md").unwrap_or(false)
                    && e.file_name() != "MEMORY.md"
            })
            .filter_map(|e| parse_memory_file(&e.path()))
            .collect()
    }

    /// Delete all memory files (including MEMORY.md). Called before writing a
    /// consolidated set so stale entries don't survive consolidation.
    pub fn clear_all(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.path().extension().map(|x| x == "md").unwrap_or(false) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    // ── Private ───────────────────────────────────────────────────────────────

    fn load_index(&self) -> String {
        let index_path = self.dir.join("MEMORY.md");
        std::fs::read_to_string(&index_path).unwrap_or_default()
    }

    fn load_relevant(&self, query: &str, max: usize) -> Vec<MemoryFile> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();

        let mut scored: Vec<(u32, MemoryFile)> = entries
            .flatten()
            .filter(|e| {
                e.path().extension().map(|x| x == "md").unwrap_or(false)
                    && e.file_name() != "MEMORY.md"
            })
            .filter_map(|e| parse_memory_file(&e.path()))
            .map(|m| {
                let score = relevance_score(&m, &query_words);
                (score, m)
            })
            .filter(|(score, _)| *score > 0)
            .collect();

        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.metadata.updated_at.cmp(&a.1.metadata.updated_at))
        });
        scored.into_iter().take(max).map(|(_, m)| m).collect()
    }

    fn rebuild_index(&self) -> std::io::Result<()> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Ok(());
        };

        let mut lines = vec!["# Memory index\n".to_string()];
        let mut files: Vec<_> = entries
            .flatten()
            .filter(|e| {
                e.path().extension().map(|x| x == "md").unwrap_or(false)
                    && e.file_name() != "MEMORY.md"
            })
            .collect();
        files.sort_by_key(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .map(std::cmp::Reverse)
        });

        for entry in files.iter().take(200) {
            if let Some(m) = parse_memory_file(&entry.path()) {
                let path_str = entry.file_name().to_string_lossy().to_string();
                lines.push(format!("- [{}]({}) — {}", m.name, path_str, m.description));
            }
        }

        let index_path = self.dir.join("MEMORY.md");
        if lines.len() == 1 {
            std::fs::write(index_path, "")
        } else {
            std::fs::write(index_path, lines.join("\n"))
        }
    }
}

// ── Memory file ───────────────────────────────────────────────────────────────

pub struct MemoryFile {
    pub name: String,
    pub description: String,
    pub kind: String,
    pub body: String,
    pub metadata: MemoryMetadata,
}

fn parse_memory_file(path: &Path) -> Option<MemoryFile> {
    let text = std::fs::read_to_string(path).ok()?;
    // Expect frontmatter between first --- markers
    if !text.starts_with("---\n") {
        return None;
    }
    let rest = &text[4..];
    let end = rest.find("\n---\n")?;
    let frontmatter = &rest[..end];
    let body = rest[end + 5..].trim().to_string();

    let mut name = String::new();
    let mut description = String::new();
    let mut kind = "project".to_string();
    let mut source = "legacy".to_string();
    let mut source_evidence = String::new();
    let mut confidence = 0.5_f32;
    let mut created_at = 0_u64;
    let mut updated_at = 0_u64;
    let mut expires_at = None;

    for line in frontmatter.lines() {
        if let Some(v) = line.strip_prefix("name: ") {
            name = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("description: ") {
            description = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("type: ") {
            kind = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("source: ") {
            source = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("source_evidence: ") {
            source_evidence = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("confidence: ") {
            confidence = v.trim().parse().unwrap_or(0.5);
        } else if let Some(v) = line.strip_prefix("created_at: ") {
            created_at = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("updated_at: ") {
            updated_at = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("expires_at: ") {
            expires_at = v.trim().parse::<u64>().ok().filter(|value| *value > 0);
        }
    }

    if name.is_empty() {
        return None;
    }
    if expires_at.is_some_and(|expiry| expiry <= now_secs()) {
        return None;
    }
    Some(MemoryFile {
        name,
        description,
        kind,
        body,
        metadata: MemoryMetadata {
            source,
            source_evidence,
            confidence: confidence.clamp(0.0, 1.0),
            created_at,
            updated_at,
            expires_at,
        },
    })
}

fn now_secs() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn relevance_score(m: &MemoryFile, query_words: &[&str]) -> u32 {
    let haystack = format!(
        "{} {} {}",
        m.name.to_lowercase(),
        m.description.to_lowercase(),
        m.body.to_lowercase()
    );
    query_words
        .iter()
        .filter(|&&w| w.len() >= 3 && haystack.contains(w))
        .count() as u32
}

/// Returns the per-project root directory for `cwd` — parent of `memory/`.
pub fn project_root(cwd: &Path) -> PathBuf {
    dragent_paths::home().join("projects").join(cwd_hash(cwd))
}

fn memory_dir(cwd: &Path) -> PathBuf {
    project_root(cwd).join("memory")
}

fn cwd_hash(cwd: &Path) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(cwd.to_string_lossy().as_bytes());
    hex::encode(&h.finalize()[..8])
}

#[cfg(test)]
mod tests {
    use super::{MemoryMetadata, MemoryStore};

    #[test]
    fn prompt_exposes_provenance_and_standing_confidence() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore {
            dir: temp.path().join("memory"),
        };
        store
            .save_with_metadata(
                "verify-before-done",
                "Run verification before reporting completion",
                "feedback",
                "Run the relevant check before saying a fix works.",
                MemoryMetadata::extracted(
                    "always run the tests before saying it is fixed",
                    0.95,
                    "feedback",
                ),
            )
            .unwrap();

        let prompt = store.to_prompt_block("verification tests");
        assert!(prompt.contains("source=conversation-extraction"));
        assert!(prompt.contains("confidence=0.95"));
        assert!(prompt.contains("standing=true"));
    }

    #[test]
    fn expired_project_memory_is_not_loaded() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore {
            dir: temp.path().join("memory"),
        };
        let mut metadata = MemoryMetadata::extracted("old build output", 0.9, "project");
        metadata.expires_at = Some(1);
        store
            .save_with_metadata(
                "old-build-state",
                "Old build state",
                "project",
                "The build was failing.",
                metadata,
            )
            .unwrap();

        assert!(store.to_prompt_block("build failing").is_empty());
        assert!(store.load_all().is_empty());
    }
}
