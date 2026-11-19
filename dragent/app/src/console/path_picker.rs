use warpui::{Entity, ModelContext};

use crate::console::cmd_suggest::build_cmd_suggest;
use crate::console::fs_suggest::{apply_fs_insert, build_fs_suggest, FsEntry, FsSuggestRequest};

#[derive(Debug, Clone)]
pub enum PathPickerEvent {
    Opened,
    Closed,
    SelectionChanged,
}

#[derive(Clone)]
pub struct PathPicker {
    pub open: bool,
    pub selected: usize,
    request: Option<FsSuggestRequest>,
}

impl Entity for PathPicker {
    type Event = PathPickerEvent;
}

impl PathPicker {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            open: false,
            selected: 0,
            request: None,
        }
    }

    pub fn entries(&self) -> &[FsEntry] {
        self.request
            .as_ref()
            .map(|r| r.entries.as_slice())
            .unwrap_or(&[])
    }

    pub fn try_open_for_buffer(
        &mut self,
        buffer: &str,
        cwd: &std::path::Path,
        with_command_help: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        // Command/flag descriptions (the cmd_suggest dropdown) are suppressed in
        // shell mode — `with_command_help = false` leaves pure path completion.
        let suggestion = if with_command_help {
            build_cmd_suggest(buffer).or_else(|| build_fs_suggest(buffer, cwd))
        } else {
            build_fs_suggest(buffer, cwd)
        };
        match suggestion {
            Some(request) => {
                self.request = Some(request);
                self.selected = 0;
                self.open = true;
                ctx.emit(PathPickerEvent::Opened);
                ctx.notify();
            }
            None => self.close(ctx),
        }
    }

    pub fn close(&mut self, ctx: &mut ModelContext<Self>) {
        if self.open {
            self.open = false;
            self.request = None;
            self.selected = 0;
            ctx.emit(PathPickerEvent::Closed);
            ctx.notify();
        }
    }

    pub fn select_next(&mut self, ctx: &mut ModelContext<Self>) {
        if !self.open {
            return;
        }
        let len = self.entries().len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected + 1) % len;
        ctx.emit(PathPickerEvent::SelectionChanged);
        ctx.notify();
    }

    pub fn select_prev(&mut self, ctx: &mut ModelContext<Self>) {
        if !self.open {
            return;
        }
        let len = self.entries().len();
        if len == 0 {
            return;
        }
        self.selected = if self.selected == 0 {
            len - 1
        } else {
            self.selected - 1
        };
        ctx.emit(PathPickerEvent::SelectionChanged);
        ctx.notify();
    }

    pub fn accept_selection(&self, buffer: &str) -> Option<String> {
        let request = self.request.as_ref()?;
        let entry = request.entries.get(self.selected)?;
        Some(apply_fs_insert(buffer, request, entry))
    }

    pub fn accept_index(&self, buffer: &str, index: usize) -> Option<String> {
        let request = self.request.as_ref()?;
        let entry = request.entries.get(index)?;
        Some(apply_fs_insert(buffer, request, entry))
    }
}
