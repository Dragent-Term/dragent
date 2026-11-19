use std::collections::VecDeque;
use std::path::PathBuf;

use uuid::Uuid;

const MAX_ENTRIES: usize = 50;

#[derive(Clone, Debug)]
pub struct AgentQueryEntry {
    pub query: String,
    pub cwd: PathBuf,
    pub is_autodetected: bool,
    /// Assigned by the API on a successful response.
    pub conversation_id: Option<Uuid>,
    /// Summary of returned action kinds, e.g. "Message, RunCommand, Done".
    pub action_summary: Option<String>,
}

pub struct AgentQueryLog {
    entries: VecDeque<AgentQueryEntry>,
}

impl AgentQueryLog {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    pub fn push(&mut self, entry: AgentQueryEntry) {
        if self.entries.len() >= MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn entries(&self) -> impl Iterator<Item = &AgentQueryEntry> {
        self.entries.iter()
    }
}
