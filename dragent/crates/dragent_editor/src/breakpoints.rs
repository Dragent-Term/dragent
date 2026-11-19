use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BreakpointState {
    #[default]
    Enabled,
    Disabled,
    Unverified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Breakpoint {
    pub line: usize,
    pub state: BreakpointState,
}

#[derive(Clone, Debug, Default)]
pub struct BreakpointStore {
    entries: BTreeMap<usize, BreakpointState>,
}

#[derive(Clone, Debug, Default)]
pub struct BreakpointRegistry {
    documents: BTreeMap<PathBuf, BreakpointStore>,
}

impl BreakpointRegistry {
    pub fn toggle(&mut self, path: &Path, line: usize) -> bool {
        self.documents
            .entry(path.to_path_buf())
            .or_default()
            .toggle(line)
    }

    pub fn state(&self, path: &Path, line: usize) -> Option<BreakpointState> {
        self.documents.get(path).and_then(|store| store.state(line))
    }

    pub fn breakpoints(&self, path: &Path) -> impl Iterator<Item = Breakpoint> + '_ {
        self.documents
            .get(path)
            .into_iter()
            .flat_map(BreakpointStore::iter)
    }
}

impl BreakpointStore {
    pub fn toggle(&mut self, line: usize) -> bool {
        if line == 0 {
            return false;
        }
        if self.entries.remove(&line).is_none() {
            self.entries.insert(line, BreakpointState::Enabled);
            true
        } else {
            false
        }
    }

    pub fn state(&self, line: usize) -> Option<BreakpointState> {
        self.entries.get(&line).copied()
    }

    pub fn set_state(&mut self, line: usize, state: BreakpointState) {
        if line > 0 {
            self.entries.insert(line, state);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = Breakpoint> + '_ {
        self.entries
            .iter()
            .map(|(&line, &state)| Breakpoint { line, state })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggles_breakpoints_by_one_based_line() {
        let mut store = BreakpointStore::default();
        assert!(store.toggle(12));
        assert_eq!(store.state(12), Some(BreakpointState::Enabled));
        assert!(!store.toggle(12));
        assert_eq!(store.state(12), None);
    }

    #[test]
    fn registry_isolates_breakpoints_by_document() {
        let mut registry = BreakpointRegistry::default();
        registry.toggle(Path::new("src/a.rs"), 4);

        assert_eq!(
            registry.state(Path::new("src/a.rs"), 4),
            Some(BreakpointState::Enabled)
        );
        assert_eq!(registry.state(Path::new("src/b.rs"), 4), None);
    }
}
