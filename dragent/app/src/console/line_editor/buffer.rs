use warpui::{Entity, ModelContext};

/// Editable command line at the bottom of the console pane.
pub struct LineEditorBuffer {
    pub buffer: String,
    pub cursor: usize,
    pub history: Vec<String>,
    pub history_idx: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum LineEditorEvent {
    CommandCaptured(String),
}

impl Entity for LineEditorBuffer {
    type Event = LineEditorEvent;
}

impl LineEditorBuffer {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_idx: None,
        }
    }

    pub fn push_char(&mut self, c: char) {
        let mut chars: Vec<char> = self.buffer.chars().collect();
        chars.insert(self.cursor, c);
        self.buffer = chars.into_iter().collect();
        self.cursor += 1;
        self.history_idx = None;
    }

    pub fn insert_newline(&mut self) {
        let mut chars: Vec<char> = self.buffer.chars().collect();
        chars.insert(self.cursor, '\n');
        self.buffer = chars.into_iter().collect();
        self.cursor += 1;
        self.history_idx = None;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let mut chars: Vec<char> = self.buffer.chars().collect();
            chars.remove(self.cursor - 1);
            self.buffer = chars.into_iter().collect();
            self.cursor -= 1;
            self.history_idx = None;
        }
    }

    pub fn set_buffer(&mut self, buffer: String) {
        self.buffer = buffer;
        self.cursor = self.buffer.chars().count();
        self.history_idx = None;
    }

    pub fn cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn cursor_right(&mut self, external_ghost: Option<String>) -> bool {
        let len = self.buffer.chars().count();
        if self.cursor < len {
            self.cursor += 1;
            false
        } else {
            self.accept_ghost(external_ghost)
        }
    }

    /// Inline suggestion from command history.
    ///
    /// Scores candidates by recency AND frequency rather than just longest match.
    /// The typed text must be a prefix of the candidate (prefix-only match).
    pub fn ghost_suffix(&self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }
        let needle = self.buffer.to_lowercase();
        let history_len = self.history.len();

        // Collect (frequency, recency_score, candidate) for all prefix matches.
        // recency_score: position from the end (0 = most recent) → lower is better.
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for entry in &self.history {
            *counts.entry(entry.as_str()).or_insert(0) += 1;
        }

        // Iterate from most recent to oldest, assigning recency index.
        let best = self
            .history
            .iter()
            .rev()
            .enumerate()
            .filter(|(_, entry)| {
                entry.len() > self.buffer.len() && entry.to_lowercase().starts_with(&needle)
            })
            .map(|(rev_idx, entry)| {
                let freq = *counts.get(entry.as_str()).unwrap_or(&1);
                // Higher score = better.
                // Recency bonus: history_len - rev_idx (most recent → largest value).
                // Frequency bonus: freq * 3.
                let score = (history_len - rev_idx) as f64 + (freq as f64) * 3.0;
                (score, entry)
            })
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        best.map(|(_, entry)| entry[self.buffer.len()..].to_string())
    }

    /// De-duplicated history with frequency counts, most recent first.
    pub fn history_with_counts(&self) -> Vec<(String, usize)> {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for entry in &self.history {
            *counts.entry(entry.as_str()).or_insert(0) += 1;
        }

        let mut result = Vec::new();
        for entry in self.history.iter().rev() {
            if seen.insert(entry.as_str()) {
                let count = *counts.get(entry.as_str()).unwrap_or(&1);
                result.push((entry.clone(), count));
            }
        }
        result
    }

    pub fn accept_ghost(&mut self, external_ghost: Option<String>) -> bool {
        let suffix = external_ghost.or_else(|| self.ghost_suffix());
        if let Some(s) = suffix {
            self.buffer.push_str(&s);
            self.cursor = self.buffer.chars().count();
            self.history_idx = None;
            true
        } else {
            false
        }
    }

    pub fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_idx {
            None => {
                let idx = self.history.len() - 1;
                self.history_idx = Some(idx);
                self.buffer = self.history[idx].clone();
            }
            Some(idx) if idx > 0 => {
                let new_idx = idx - 1;
                self.history_idx = Some(new_idx);
                self.buffer = self.history[new_idx].clone();
            }
            _ => {}
        }
        self.cursor = self.buffer.chars().count();
    }

    pub fn history_down(&mut self) {
        match self.history_idx {
            Some(idx) if idx < self.history.len() - 1 => {
                let new_idx = idx + 1;
                self.history_idx = Some(new_idx);
                self.buffer = self.history[new_idx].clone();
            }
            Some(_) => {
                self.history_idx = None;
                self.buffer.clear();
            }
            None => {}
        }
        self.cursor = self.buffer.chars().count();
    }

    pub fn submit(&mut self, ctx: &mut ModelContext<Self>) -> Option<String> {
        let command = self.buffer.trim().to_string();
        if command.is_empty() {
            return None;
        }
        if self.history.last().map(|h| h.as_str()) != Some(command.as_str()) {
            self.history.push(command.clone());
        }
        self.buffer.clear();
        self.cursor = 0;
        self.history_idx = None;
        ctx.emit(LineEditorEvent::CommandCaptured(command.clone()));
        Some(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghost_suffix_from_history() {
        let mut editor = LineEditorBuffer {
            buffer: "git sta".into(),
            cursor: 7,
            history: vec!["git status".into(), "ls".into()],
            history_idx: None,
        };
        assert_eq!(editor.ghost_suffix(), Some("tus".into()));
        editor.buffer = "ls".into();
        assert_eq!(editor.ghost_suffix(), None);
    }
}
