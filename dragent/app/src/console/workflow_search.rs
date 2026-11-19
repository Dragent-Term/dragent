//! Fuzzy workflow picker state (Ctrl/Cmd+Shift+P).

use crate::console::workflows::{WorkflowMatch, WorkflowStore};

#[derive(Debug, Default, Clone)]
pub struct WorkflowSearchState {
    pub open: bool,
    pub query: String,
    pub saved_buffer: String,
    pub results: Vec<WorkflowMatch>,
    pub selected: usize,
}

impl WorkflowSearchState {
    pub fn refresh(&mut self, store: &WorkflowStore) {
        self.results = store.search(&self.query);
        if self.selected >= self.results.len() {
            self.selected = 0;
        }
    }

    pub fn selected_workflow(&self) -> Option<&crate::console::workflows::Workflow> {
        self.results.get(self.selected).map(|m| &m.workflow)
    }

    pub fn move_up(&mut self) {
        if !self.results.is_empty() && self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if !self.results.is_empty() && self.selected + 1 < self.results.len() {
            self.selected += 1;
        }
    }
}
