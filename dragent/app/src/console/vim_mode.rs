//! Lightweight vim-style modal editing for the command input buffer.

use warpui::keymap::Keystroke;

use crate::console::input_editing::{line_end, line_start, word_end, word_start};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMode {
    Normal,
    Insert,
    VisualChar,
    VisualLine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VimEffect {
    SetMode(VimMode),
    MoveCursor(usize),
    DeleteRange(usize, usize),
    Yank(String),
    Paste(String),
    Undo,
    SelectRange(usize, usize),
    ClearSelection,
    HistoryUp,
    HistoryDown,
    MoveVisualLine(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VimDispatch {
    Consumed,
    Fallthrough,
    Effects(Vec<VimEffect>),
}

#[derive(Debug, Clone)]
pub struct VimState {
    pub mode: VimMode,
    pub register: String,
    /// Pending normal-mode prefix (`d`, `g`, …).
    pub pending: Option<char>,
    pub count: u32,
    pub show_cmd: String,
}

pub struct VimContext<'a> {
    pub buffer: &'a str,
    pub cursor: usize,
    pub cursor_visual_row: usize,
    pub visual_line_count: usize,
    pub selection: Option<(usize, usize)>,
}

impl Default for VimState {
    fn default() -> Self {
        Self::new_insert()
    }
}

impl VimState {
    pub fn new_insert() -> Self {
        Self {
            mode: VimMode::Insert,
            register: String::new(),
            pending: None,
            count: 0,
            show_cmd: String::new(),
        }
    }

    pub fn reset_to_insert(&mut self) {
        self.mode = VimMode::Insert;
        self.pending = None;
        self.count = 0;
        self.show_cmd.clear();
    }

    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            VimMode::Normal => "NORMAL",
            VimMode::Insert => "INSERT",
            VimMode::VisualChar => "VISUAL",
            VimMode::VisualLine => "VLINE",
        }
    }

    pub fn handle_key(&mut self, keystroke: &Keystroke, ctx: &VimContext<'_>) -> VimDispatch {
        if keystroke.cmd || keystroke.ctrl {
            return VimDispatch::Fallthrough;
        }

        match self.mode {
            VimMode::Insert => self.handle_insert(keystroke, ctx),
            VimMode::Normal => self.handle_normal(keystroke, ctx),
            VimMode::VisualChar | VimMode::VisualLine => self.handle_visual(keystroke, ctx),
        }
    }

    fn handle_insert(&mut self, keystroke: &Keystroke, ctx: &VimContext<'_>) -> VimDispatch {
        if keystroke.key == "escape" {
            self.mode = VimMode::Normal;
            let len = chars(ctx.buffer).len();
            // In NORMAL mode the cursor sits on a character, not past the end.
            let mut effects = vec![VimEffect::SetMode(VimMode::Normal)];
            if ctx.cursor > 0 && ctx.cursor == len {
                effects.insert(0, VimEffect::MoveCursor(ctx.cursor - 1));
            }
            return VimDispatch::Effects(effects);
        }
        if keystroke.key == "up" && ctx.cursor_visual_row == 0 {
            return VimDispatch::Effects(vec![VimEffect::HistoryUp]);
        }
        if keystroke.key == "down" && ctx.cursor_visual_row + 1 >= ctx.visual_line_count.max(1) {
            return VimDispatch::Effects(vec![VimEffect::HistoryDown]);
        }
        VimDispatch::Fallthrough
    }

    fn handle_normal(&mut self, keystroke: &Keystroke, ctx: &VimContext<'_>) -> VimDispatch {
        if let Some(digit) = digit_key(&keystroke.key) {
            self.count = self.count.saturating_mul(10).saturating_add(digit);
            self.show_cmd.push_str(&keystroke.key);
            return VimDispatch::Consumed;
        }

        let count = self.count.max(1);
        self.count = 0;
        self.show_cmd.clear();

        if let Some(pending) = self.pending.take() {
            return self.handle_pending(pending, &keystroke.key, count, ctx);
        }

        self.dispatch_normal_key(&keystroke.key, keystroke.shift, count, ctx)
    }

    fn dispatch_normal_key(
        &mut self,
        key: &str,
        shift: bool,
        count: u32,
        ctx: &VimContext<'_>,
    ) -> VimDispatch {
        match key {
            "escape" => VimDispatch::Consumed,
            "i" => self.enter_insert(
                ctx.cursor,
                VimDispatch::Effects(vec![VimEffect::SetMode(VimMode::Insert)]),
            ),
            "I" => {
                let start = line_start(&chars(ctx.buffer), ctx.cursor);
                self.enter_insert(
                    start,
                    VimDispatch::Effects(vec![
                        VimEffect::MoveCursor(start),
                        VimEffect::SetMode(VimMode::Insert),
                    ]),
                )
            }
            "a" => {
                let pos = (ctx.cursor + 1).min(chars(ctx.buffer).len());
                self.enter_insert(
                    pos,
                    VimDispatch::Effects(vec![
                        VimEffect::MoveCursor(pos),
                        VimEffect::SetMode(VimMode::Insert),
                    ]),
                )
            }
            "A" => {
                let end = line_end(&chars(ctx.buffer), ctx.cursor);
                self.enter_insert(
                    end,
                    VimDispatch::Effects(vec![
                        VimEffect::MoveCursor(end),
                        VimEffect::SetMode(VimMode::Insert),
                    ]),
                )
            }
            "o" => self.open_line_below(ctx),
            "O" => self.open_line_above(ctx),
            "h" | "left" => self.move_by(-(count as i32), ctx),
            "l" | "right" => self.move_by(count as i32, ctx),
            "k" | "up" => self.vertical_move(-(count as i32), ctx),
            "j" | "down" => self.vertical_move(count as i32, ctx),
            "0" => {
                let start = line_start(&chars(ctx.buffer), ctx.cursor);
                VimDispatch::Effects(vec![VimEffect::MoveCursor(start)])
            }
            "home" => {
                let start = line_start(&chars(ctx.buffer), ctx.cursor);
                VimDispatch::Effects(vec![VimEffect::MoveCursor(start)])
            }
            "end" | "$" => {
                let cs = chars(ctx.buffer);
                let end = line_end(&cs, ctx.cursor);
                let pos = if end == 0 { 0 } else { end.saturating_sub(1) };
                VimDispatch::Effects(vec![VimEffect::MoveCursor(pos)])
            }
            "^" => {
                let start = first_non_whitespace(&chars(ctx.buffer), ctx.cursor);
                VimDispatch::Effects(vec![VimEffect::MoveCursor(start)])
            }
            "w" => self.word_forward(count, ctx),
            "b" => self.word_backward(count, ctx),
            "e" => self.word_end_forward(count, ctx),
            "x" => self.delete_char(count, ctx),
            "X" => self.delete_char_before(count, ctx),
            "d" => {
                self.pending = Some('d');
                self.show_cmd.push('d');
                VimDispatch::Consumed
            }
            "c" => {
                self.pending = Some('c');
                self.show_cmd.push('c');
                VimDispatch::Consumed
            }
            "y" => {
                self.pending = Some('y');
                self.show_cmd.push('y');
                VimDispatch::Consumed
            }
            "p" => self.paste_after(ctx),
            "P" => self.paste_before(ctx),
            "u" => VimDispatch::Effects(vec![VimEffect::Undo]),
            "v" => {
                self.mode = VimMode::VisualChar;
                VimDispatch::Effects(vec![
                    VimEffect::SelectRange(ctx.cursor, ctx.cursor),
                    VimEffect::SetMode(VimMode::VisualChar),
                ])
            }
            "V" => self.visual_line(ctx),
            "g" => {
                self.pending = Some('g');
                VimDispatch::Consumed
            }
            "G" => self.goto_line(count, ctx, false),
            _ if key.len() == 1 && !shift => {
                self.mode = VimMode::Insert;
                VimDispatch::Fallthrough
            }
            _ => VimDispatch::Consumed,
        }
    }

    fn handle_visual(&mut self, keystroke: &Keystroke, ctx: &VimContext<'_>) -> VimDispatch {
        match keystroke.key.as_str() {
            "escape" => {
                self.mode = VimMode::Normal;
                VimDispatch::Effects(vec![
                    VimEffect::ClearSelection,
                    VimEffect::SetMode(VimMode::Normal),
                ])
            }
            "h" | "left" => self.visual_move(-1, ctx),
            "l" | "right" => self.visual_move(1, ctx),
            "j" | "down" => self.visual_line_move(1, ctx),
            "k" | "up" => self.visual_line_move(-1, ctx),
            "d" | "x" => self.delete_selection(ctx),
            "y" => self.yank_selection(ctx),
            _ => VimDispatch::Consumed,
        }
    }

    fn handle_pending(
        &mut self,
        pending: char,
        key: &str,
        count: u32,
        ctx: &VimContext<'_>,
    ) -> VimDispatch {
        match (pending, key) {
            ('d', "d") | ('c', "c") => self.delete_lines(count, ctx, pending == 'c'),
            ('d', "w") | ('c', "w") => self.delete_word(count, ctx, pending == 'c'),
            ('y', "y") => self.yank_lines(count, ctx),
            ('y', "w") => self.yank_word(count, ctx),
            ('g', "g") => self.goto_line(count.max(1), ctx, true),
            _ => {
                self.pending = None;
                self.show_cmd.clear();
                self.dispatch_normal_key(key, false, count, ctx)
            }
        }
    }

    fn enter_insert(&mut self, _pos: usize, dispatch: VimDispatch) -> VimDispatch {
        self.mode = VimMode::Insert;
        dispatch
    }

    fn move_by(&mut self, delta: i32, ctx: &VimContext<'_>) -> VimDispatch {
        let len = chars(ctx.buffer).len();
        if len == 0 {
            return VimDispatch::Effects(vec![VimEffect::MoveCursor(0)]);
        }
        let max = (len - 1) as i32;
        let next = (ctx.cursor as i32 + delta).clamp(0, max) as usize;
        VimDispatch::Effects(vec![VimEffect::MoveCursor(next)])
    }

    fn vertical_move(&mut self, delta: i32, ctx: &VimContext<'_>) -> VimDispatch {
        if delta < 0 && ctx.cursor_visual_row == 0 {
            return VimDispatch::Consumed;
        }
        if delta > 0 && ctx.cursor_visual_row + 1 >= ctx.visual_line_count.max(1) {
            return VimDispatch::Consumed;
        }
        VimDispatch::Effects(vec![VimEffect::MoveVisualLine(delta)])
    }

    fn word_forward(&mut self, count: u32, ctx: &VimContext<'_>) -> VimDispatch {
        let cs = chars(ctx.buffer);
        let mut pos = ctx.cursor;
        for _ in 0..count {
            pos = word_end(&cs, pos);
            if pos < cs.len() && cs[pos].is_whitespace() {
                pos = word_end(&cs, pos + 1);
            }
        }
        VimDispatch::Effects(vec![VimEffect::MoveCursor(pos.min(cs.len()))])
    }

    fn word_backward(&mut self, count: u32, ctx: &VimContext<'_>) -> VimDispatch {
        let cs = chars(ctx.buffer);
        let mut pos = ctx.cursor;
        for _ in 0..count {
            pos = word_start(&cs, pos.saturating_sub(1));
        }
        VimDispatch::Effects(vec![VimEffect::MoveCursor(pos)])
    }

    fn word_end_forward(&mut self, count: u32, ctx: &VimContext<'_>) -> VimDispatch {
        let cs = chars(ctx.buffer);
        let mut pos = ctx.cursor;
        for _ in 0..count {
            pos = word_end(&cs, if pos < cs.len() { pos + 1 } else { pos });
            if pos > 0 {
                pos = pos.saturating_sub(1);
            }
        }
        VimDispatch::Effects(vec![VimEffect::MoveCursor(
            pos.min(cs.len().saturating_sub(1)),
        )])
    }

    fn delete_char(&mut self, count: u32, ctx: &VimContext<'_>) -> VimDispatch {
        let cs = chars(ctx.buffer);
        let len = cs.len();
        if len == 0 {
            return VimDispatch::Consumed;
        }
        // At EOL the cursor may sit past the last char (insert-style); delete that char.
        let mut pos = ctx.cursor;
        if pos >= len {
            pos = len - 1;
        }
        let end = (pos + count as usize).min(len);
        if pos >= end {
            return VimDispatch::Consumed;
        }
        VimDispatch::Effects(vec![VimEffect::DeleteRange(pos, end)])
    }

    fn delete_char_before(&mut self, count: u32, ctx: &VimContext<'_>) -> VimDispatch {
        let start = ctx.cursor.saturating_sub(count as usize);
        if start >= ctx.cursor {
            return VimDispatch::Consumed;
        }
        VimDispatch::Effects(vec![VimEffect::DeleteRange(start, ctx.cursor)])
    }

    fn delete_lines(
        &mut self,
        count: u32,
        ctx: &VimContext<'_>,
        enter_insert: bool,
    ) -> VimDispatch {
        let cs = chars(ctx.buffer);
        let mut start = line_start(&cs, ctx.cursor);
        let mut end = ctx.cursor;
        for _ in 0..count {
            end = line_end(&cs, end);
            if end < cs.len() {
                end += 1;
            }
        }
        let mut effects = vec![VimEffect::DeleteRange(start, end.min(cs.len()))];
        if enter_insert {
            self.mode = VimMode::Insert;
            effects.push(VimEffect::SetMode(VimMode::Insert));
        } else {
            self.mode = VimMode::Normal;
        }
        VimDispatch::Effects(effects)
    }

    fn delete_word(&mut self, count: u32, ctx: &VimContext<'_>, enter_insert: bool) -> VimDispatch {
        let cs = chars(ctx.buffer);
        let mut end = ctx.cursor;
        for _ in 0..count {
            end = word_end(&cs, end);
        }
        let mut effects = vec![VimEffect::DeleteRange(ctx.cursor, end.min(cs.len()))];
        if enter_insert {
            self.mode = VimMode::Insert;
            effects.push(VimEffect::SetMode(VimMode::Insert));
        }
        VimDispatch::Effects(effects)
    }

    fn yank_lines(&mut self, count: u32, ctx: &VimContext<'_>) -> VimDispatch {
        let cs = chars(ctx.buffer);
        let start = line_start(&cs, ctx.cursor);
        let mut end = ctx.cursor;
        for _ in 0..count {
            end = line_end(&cs, end);
            if end < cs.len() {
                end += 1;
            }
        }
        let text: String = cs[start..end.min(cs.len())].iter().collect();
        self.register = text.clone();
        self.mode = VimMode::Normal;
        VimDispatch::Effects(vec![
            VimEffect::Yank(text),
            VimEffect::SetMode(VimMode::Normal),
        ])
    }

    fn yank_word(&mut self, count: u32, ctx: &VimContext<'_>) -> VimDispatch {
        let cs = chars(ctx.buffer);
        let mut end = ctx.cursor;
        for _ in 0..count {
            end = word_end(&cs, end);
        }
        let text: String = cs[ctx.cursor..end.min(cs.len())].iter().collect();
        self.register = text.clone();
        self.mode = VimMode::Normal;
        VimDispatch::Effects(vec![
            VimEffect::Yank(text),
            VimEffect::SetMode(VimMode::Normal),
        ])
    }

    fn paste_after(&mut self, ctx: &VimContext<'_>) -> VimDispatch {
        if self.register.is_empty() {
            return VimDispatch::Consumed;
        }
        VimDispatch::Effects(vec![VimEffect::Paste(self.register.clone())])
    }

    fn paste_before(&mut self, ctx: &VimContext<'_>) -> VimDispatch {
        if self.register.is_empty() {
            return VimDispatch::Consumed;
        }
        let pos = ctx.cursor.saturating_sub(1);
        VimDispatch::Effects(vec![
            VimEffect::MoveCursor(pos),
            VimEffect::Paste(self.register.clone()),
        ])
    }

    fn open_line_below(&mut self, ctx: &VimContext<'_>) -> VimDispatch {
        let cs = chars(ctx.buffer);
        let end = line_end(&chars(ctx.buffer), ctx.cursor);
        self.mode = VimMode::Insert;
        VimDispatch::Effects(vec![
            VimEffect::MoveCursor(end),
            VimEffect::Paste("\n".into()),
            VimEffect::SetMode(VimMode::Insert),
        ])
    }

    fn open_line_above(&mut self, ctx: &VimContext<'_>) -> VimDispatch {
        let start = line_start(&chars(ctx.buffer), ctx.cursor);
        self.mode = VimMode::Insert;
        VimDispatch::Effects(vec![
            VimEffect::MoveCursor(start),
            VimEffect::Paste("\n".into()),
            VimEffect::SetMode(VimMode::Insert),
        ])
    }

    fn goto_line(&mut self, line: u32, ctx: &VimContext<'_>, from_top: bool) -> VimDispatch {
        let cs = chars(ctx.buffer);
        let target = if from_top {
            line.saturating_sub(1) as usize
        } else {
            line.saturating_sub(1) as usize
        };
        let mut line_idx = 0usize;
        let mut pos = 0usize;
        while pos <= cs.len() {
            if line_idx == target {
                return VimDispatch::Effects(vec![VimEffect::MoveCursor(pos)]);
            }
            if pos >= cs.len() {
                break;
            }
            if cs[pos] == '\n' {
                line_idx += 1;
            }
            pos += 1;
        }
        VimDispatch::Effects(vec![VimEffect::MoveCursor(cs.len())])
    }

    fn visual_line(&mut self, ctx: &VimContext<'_>) -> VimDispatch {
        let cs = chars(ctx.buffer);
        let start = line_start(&cs, ctx.cursor);
        let end = line_end(&cs, ctx.cursor);
        self.mode = VimMode::VisualLine;
        VimDispatch::Effects(vec![
            VimEffect::SelectRange(start, end),
            VimEffect::SetMode(VimMode::VisualLine),
        ])
    }

    fn visual_move(&mut self, delta: i32, ctx: &VimContext<'_>) -> VimDispatch {
        let len = chars(ctx.buffer).len();
        let next = (ctx.cursor as i32 + delta).clamp(0, len as i32) as usize;
        let (start, end) = ctx.selection.unwrap_or((ctx.cursor, ctx.cursor));
        let anchor = if ctx.cursor == start { end } else { start };
        VimDispatch::Effects(vec![
            VimEffect::SelectRange(anchor.min(next), anchor.max(next)),
            VimEffect::MoveCursor(next),
        ])
    }

    fn visual_line_move(&mut self, delta: i32, _ctx: &VimContext<'_>) -> VimDispatch {
        VimDispatch::Effects(vec![VimEffect::MoveVisualLine(delta)])
    }

    fn delete_selection(&mut self, ctx: &VimContext<'_>) -> VimDispatch {
        let Some((start, end)) = ctx.selection else {
            return VimDispatch::Consumed;
        };
        self.mode = VimMode::Normal;
        VimDispatch::Effects(vec![
            VimEffect::DeleteRange(start, end),
            VimEffect::ClearSelection,
            VimEffect::SetMode(VimMode::Normal),
        ])
    }

    fn yank_selection(&mut self, ctx: &VimContext<'_>) -> VimDispatch {
        let Some((start, end)) = ctx.selection else {
            return VimDispatch::Consumed;
        };
        let cs = chars(ctx.buffer);
        let text: String = cs[start..end.min(cs.len())].iter().collect();
        self.register = text.clone();
        self.mode = VimMode::Normal;
        VimDispatch::Effects(vec![
            VimEffect::Yank(text),
            VimEffect::ClearSelection,
            VimEffect::SetMode(VimMode::Normal),
        ])
    }
}

fn chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

fn digit_key(key: &str) -> Option<u32> {
    if key.len() == 1 {
        key.chars().next().and_then(|c| c.to_digit(10))
    } else {
        None
    }
}

fn first_non_whitespace(cs: &[char], cursor: usize) -> usize {
    let line_start = line_start(cs, cursor);
    let line_end = line_end(cs, cursor);
    for i in line_start..line_end {
        if !cs[i].is_whitespace() {
            return i;
        }
    }
    line_start
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(buffer: &str, cursor: usize) -> VimContext<'_> {
        VimContext {
            buffer,
            cursor,
            cursor_visual_row: 0,
            visual_line_count: 1,
            selection: None,
        }
    }

    #[test]
    fn escape_exits_insert() {
        let mut state = VimState::new_insert();
        let keystroke = Keystroke {
            caps_lock: false,
            key: "escape".into(),
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        let dispatch = state.handle_key(&keystroke, &ctx("hello", 3));
        assert!(matches!(dispatch, VimDispatch::Effects(_)));
        assert_eq!(state.mode, VimMode::Normal);
    }

    #[test]
    fn x_deletes_at_end_of_insert() {
        let mut state = VimState {
            mode: VimMode::Normal,
            ..Default::default()
        };
        let ctx = VimContext {
            buffer: "hello",
            cursor: 5,
            cursor_visual_row: 0,
            visual_line_count: 1,
            selection: None,
        };
        let keystroke = Keystroke {
            caps_lock: false,
            key: "x".into(),
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        let dispatch = state.handle_key(&keystroke, &ctx);
        match dispatch {
            VimDispatch::Effects(effects) => {
                assert!(effects
                    .iter()
                    .any(|e| matches!(e, VimEffect::DeleteRange(4, 5))));
            }
            other => panic!("expected delete effect, got {other:?}"),
        }
    }

    #[test]
    fn escape_from_insert_moves_cursor_onto_last_char() {
        let mut state = VimState::new_insert();
        let ctx = VimContext {
            buffer: "hello",
            cursor: 5,
            cursor_visual_row: 0,
            visual_line_count: 1,
            selection: None,
        };
        let keystroke = Keystroke {
            caps_lock: false,
            key: "escape".into(),
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        let dispatch = state.handle_key(&keystroke, &ctx);
        match dispatch {
            VimDispatch::Effects(effects) => {
                assert!(effects
                    .iter()
                    .any(|e| matches!(e, VimEffect::MoveCursor(4))));
                assert!(effects
                    .iter()
                    .any(|e| matches!(e, VimEffect::SetMode(VimMode::Normal))));
            }
            other => panic!("expected effects, got {other:?}"),
        }
    }

    #[test]
    fn h_moves_cursor_left_in_normal() {
        let mut state = VimState {
            mode: VimMode::Normal,
            ..Default::default()
        };
        let ctx = VimContext {
            buffer: "hello",
            cursor: 4,
            cursor_visual_row: 0,
            visual_line_count: 1,
            selection: None,
        };
        let keystroke = Keystroke {
            caps_lock: false,
            key: "h".into(),
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        let dispatch = state.handle_key(&keystroke, &ctx);
        match dispatch {
            VimDispatch::Effects(effects) => {
                assert!(effects
                    .iter()
                    .any(|e| matches!(e, VimEffect::MoveCursor(3))));
            }
            other => panic!("expected move effect, got {other:?}"),
        }
    }

    #[test]
    fn j_on_single_line_is_noop_in_normal() {
        let mut state = VimState {
            mode: VimMode::Normal,
            ..Default::default()
        };
        let ctx = VimContext {
            buffer: "hello",
            cursor: 2,
            cursor_visual_row: 0,
            visual_line_count: 1,
            selection: None,
        };
        let keystroke = Keystroke {
            caps_lock: false,
            key: "j".into(),
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        let dispatch = state.handle_key(&keystroke, &ctx);
        assert!(matches!(dispatch, VimDispatch::Consumed));
    }

    #[test]
    fn x_after_cancelled_d_operator_deletes_char() {
        let mut state = VimState {
            mode: VimMode::Normal,
            pending: Some('d'),
            ..Default::default()
        };
        let ctx = VimContext {
            buffer: "hello",
            cursor: 4,
            cursor_visual_row: 0,
            visual_line_count: 1,
            selection: None,
        };
        let keystroke = Keystroke {
            caps_lock: false,
            key: "x".into(),
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        let dispatch = state.handle_key(&keystroke, &ctx);
        match dispatch {
            VimDispatch::Effects(effects) => {
                assert!(effects
                    .iter()
                    .any(|e| matches!(e, VimEffect::DeleteRange(4, 5))));
            }
            other => panic!("expected delete effect, got {other:?}"),
        }
        assert!(state.pending.is_none());
    }

    #[test]
    fn dd_deletes_line() {
        let mut state = VimState {
            mode: VimMode::Normal,
            ..Default::default()
        };
        state.pending = Some('d');
        let keystroke = Keystroke {
            caps_lock: false,
            key: "d".into(),
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        let dispatch = state.handle_key(&keystroke, &ctx("line one\nline two", 0));
        match dispatch {
            VimDispatch::Effects(effects) => {
                assert!(effects
                    .iter()
                    .any(|e| matches!(e, VimEffect::DeleteRange(0, 9))));
            }
            other => panic!("expected effects, got {other:?}"),
        }
    }
}
