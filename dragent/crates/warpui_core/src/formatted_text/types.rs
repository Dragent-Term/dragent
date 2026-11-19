use std::any::Any;
use std::collections::VecDeque;
use std::fmt;
use std::fmt::Debug;
use std::ops::Range;
use std::sync::Arc;

use super::weight::CustomWeight;

/// Trait for hyperlink click actions without depending on `warpui_core::Action`.
pub trait Action: Any + Debug + Send + Sync {
    fn as_any(&self) -> &dyn Any;
}

impl<T> Action for T
where
    T: Any + Debug + Send + Sync,
{
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub trait LineCount {
    fn num_lines(&self) -> usize;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormattedTextDelta {
    pub common_prefix_lines: usize,
    pub old_suffix_formatted_text_lines: usize,
    pub new_suffix: VecDeque<FormattedTextLine>,
}

impl FormattedTextDelta {
    pub fn is_noop(&self) -> bool {
        self.old_suffix_formatted_text_lines == 0 && self.new_suffix.is_empty()
    }
}

pub fn compute_formatted_text_delta(old: FormattedText, new: FormattedText) -> FormattedTextDelta {
    let mut common_prefix_formatted_text_lines = 0usize;
    let mut common_prefix_lines = 0usize;
    let old_len = old.lines.len();
    let new_len = new.lines.len();
    let shared_len = old_len.min(new_len);

    while common_prefix_formatted_text_lines < shared_len {
        let old_line = &old.lines[common_prefix_formatted_text_lines];
        let new_line = &new.lines[common_prefix_formatted_text_lines];
        let lines_equal = match (old_line, new_line) {
            (FormattedTextLine::CodeBlock(old_block), FormattedTextLine::CodeBlock(new_block)) => {
                old_block.code == new_block.code
            }
            _ => old_line == new_line,
        };
        if !lines_equal {
            break;
        }
        common_prefix_formatted_text_lines += 1;
        common_prefix_lines += old_line.num_lines();
    }

    FormattedTextDelta {
        common_prefix_lines,
        old_suffix_formatted_text_lines: old_len.saturating_sub(common_prefix_formatted_text_lines),
        new_suffix: new
            .lines
            .iter()
            .skip(common_prefix_formatted_text_lines)
            .cloned()
            .collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormattedText {
    pub lines: VecDeque<FormattedTextLine>,
}

impl FormattedText {
    pub fn new(lines: impl Into<VecDeque<FormattedTextLine>>) -> Self {
        Self {
            lines: lines.into(),
        }
    }

    pub fn new_trimmed(lines: impl Into<VecDeque<FormattedTextLine>>) -> Self {
        let mut new = Self::new(lines);
        new.trim();
        new
    }

    fn trim(&mut self) {
        if let Some(FormattedTextLine::LineBreak) = self.lines.front() {
            self.lines.pop_front();
        }
        if let Some(FormattedTextLine::LineBreak) = self.lines.back() {
            self.lines.pop_back();
        }
    }

    pub fn raw_text(&self) -> String {
        self.lines.iter().map(|line| line.raw_text()).collect()
    }

    pub fn append_line(mut self, line: FormattedTextLine) -> Self {
        self.lines.push_back(line);
        self
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum FormattedTextLine {
    Heading(FormattedTextHeader),
    Line(FormattedTextInline),
    OrderedList(OrderedFormattedIndentTextInline),
    UnorderedList(FormattedIndentTextInline),
    CodeBlock(CodeBlockText),
    TaskList(FormattedTaskList),
    LineBreak,
    HorizontalRule,
    Embedded(serde_json::Value),
    Image(FormattedImage),
    Table(FormattedTable),
}

impl FormattedTextLine {
    pub fn raw_text(&self) -> String {
        let mut text = match self {
            Self::CodeBlock(text) => text.code.clone(),
            Self::Heading(header) => header
                .text
                .iter()
                .map(|fragment| fragment.raw_text())
                .collect::<String>(),
            Self::Line(line) => line.iter().map(|f| f.raw_text()).collect(),
            Self::TaskList(line) => line.text.iter().map(|f| f.raw_text()).collect(),
            Self::OrderedList(list) => list
                .indented_text
                .text
                .iter()
                .map(|f| f.raw_text())
                .collect(),
            Self::UnorderedList(list) => list.text.iter().map(|f| f.raw_text()).collect(),
            Self::LineBreak | Self::HorizontalRule | Self::Embedded(_) => "\n".to_string(),
            Self::Image(image) => format!("{}\n", image.alt_text),
            Self::Table(table) => table.to_internal_format(),
        };
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text
    }

    pub fn set_weight(&mut self, weight: Option<CustomWeight>) -> &Self {
        if let Some(inline) = self.inline_fragments_mut() {
            for fragment in inline {
                fragment.styles.weight = weight;
            }
        }
        self
    }

    fn inline_fragments(&self) -> Option<&FormattedTextInline> {
        match self {
            FormattedTextLine::Heading(header) => Some(&header.text),
            FormattedTextLine::Line(texts) => Some(texts),
            FormattedTextLine::OrderedList(texts) => Some(&texts.indented_text.text),
            FormattedTextLine::UnorderedList(texts) => Some(&texts.text),
            FormattedTextLine::TaskList(list) => Some(&list.text),
            _ => None,
        }
    }

    fn inline_fragments_mut(&mut self) -> Option<&mut FormattedTextInline> {
        match self {
            FormattedTextLine::Heading(header) => Some(&mut header.text),
            FormattedTextLine::Line(texts) => Some(texts),
            FormattedTextLine::OrderedList(texts) => Some(&mut texts.indented_text.text),
            FormattedTextLine::UnorderedList(texts) => Some(&mut texts.text),
            FormattedTextLine::TaskList(list) => Some(&mut list.text),
            _ => None,
        }
    }

    pub fn hyperlinks(&self, skip_raw_links: bool) -> Vec<(Range<usize>, Hyperlink)> {
        let mut hyperlinks = Vec::new();
        if let Some(inline_fragments) = self.inline_fragments() {
            let mut char_count = 0;
            for fragment in inline_fragments {
                let range_start = char_count;
                char_count += fragment.text.chars().count();
                if let Some(link) = &fragment.styles.hyperlink {
                    let include = !skip_raw_links
                        || !matches!(&link, Hyperlink::Url(url) if url == &fragment.text);
                    if include {
                        hyperlinks.push((range_start..char_count, link.clone()));
                    }
                }
            }
        }
        hyperlinks
    }

    pub fn is_empty_line(&self) -> bool {
        matches!(self, Self::Line(line) if line.iter().all(|fragment| fragment.text.is_empty()))
    }
}

impl LineCount for FormattedTextLine {
    fn num_lines(&self) -> usize {
        match self {
            Self::CodeBlock(text) => text.code.matches('\n').count().max(1),
            Self::Heading(_)
            | Self::Line(_)
            | Self::OrderedList(_)
            | Self::UnorderedList(_)
            | Self::TaskList(_)
            | Self::Embedded(_)
            | Self::Image(_) => 1,
            Self::LineBreak | Self::HorizontalRule => 0,
            Self::Table(table) => 1 + table.rows.len(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FormattedTextHeader {
    pub heading_size: usize,
    pub text: FormattedTextInline,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FormattedTaskList {
    pub complete: bool,
    pub indent_level: usize,
    pub text: FormattedTextInline,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FormattedIndentTextInline {
    pub indent_level: usize,
    pub text: FormattedTextInline,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CodeBlockText {
    pub lang: String,
    pub code: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct OrderedFormattedIndentTextInline {
    pub number: Option<usize>,
    pub indented_text: FormattedIndentTextInline,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FormattedImage {
    pub alt_text: String,
    pub source: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default, Hash)]
pub enum TableAlignment {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FormattedTable {
    pub headers: Vec<FormattedTextInline>,
    pub alignments: Vec<TableAlignment>,
    pub rows: Vec<Vec<FormattedTextInline>>,
}

impl FormattedTable {
    pub fn from_internal_format(content: &str) -> Self {
        let parse_line = |line: &str| -> Vec<FormattedTextInline> {
            line.split('\t')
                .map(|cell| vec![FormattedTextFragment::plain_text(cell)])
                .collect()
        };
        let mut lines = content.lines().peekable();
        let headers = lines.next().map(parse_line).unwrap_or_default();
        let rows: Vec<Vec<FormattedTextInline>> = lines.map(parse_line).collect();
        let col_count = headers.len();
        Self {
            headers,
            alignments: vec![TableAlignment::default(); col_count],
            rows,
        }
    }

    pub fn normalize_shape(&mut self) {
        let mut column_count = self
            .headers
            .len()
            .max(self.rows.iter().map(Vec::len).max().unwrap_or(0));
        if column_count == 0 {
            column_count = 1;
        }
        self.headers.resize_with(column_count, Vec::new);
        self.alignments
            .resize(column_count, TableAlignment::default());
        for row in &mut self.rows {
            row.resize_with(column_count, Vec::new);
        }
    }

    pub fn to_internal_format(&self) -> String {
        if self.headers.is_empty() && self.rows.is_empty() {
            return String::new();
        }
        let mut result = String::new();
        let headers: Vec<String> = self.headers.iter().map(inline_to_markdown).collect();
        result.push_str(&headers.join("\t"));
        result.push('\n');
        for row in &self.rows {
            let cells: Vec<String> = row.iter().map(inline_to_markdown).collect();
            result.push_str(&cells.join("\t"));
            result.push('\n');
        }
        result
    }

    pub fn to_plain_text(&self) -> String {
        fn inline_to_text(inline: &FormattedTextInline) -> String {
            inline.iter().map(|f| f.text.as_str()).collect()
        }
        let mut lines = Vec::new();
        let headers: Vec<String> = self.headers.iter().map(inline_to_text).collect();
        lines.push(format!("| {} |", headers.join(" | ")));
        let separator: Vec<String> = self
            .alignments
            .iter()
            .map(|alignment| match alignment {
                TableAlignment::Left => "---".to_string(),
                TableAlignment::Center => ":---:".to_string(),
                TableAlignment::Right => "---:".to_string(),
            })
            .collect();
        lines.push(format!("| {} |", separator.join(" | ")));
        for row in &self.rows {
            let cells: Vec<String> = row.iter().map(inline_to_text).collect();
            lines.push(format!("| {} |", cells.join(" | ")));
        }
        lines.join("\n")
    }
}

pub type FormattedTableAlignment = TableAlignment;
pub type FormattedTextInline = Vec<FormattedTextFragment>;

#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct FormattedTextFragment {
    pub text: String,
    pub styles: FormattedTextStyles,
}

#[derive(Debug, Clone)]
pub enum Hyperlink {
    Url(String),
    Action(Arc<dyn Action>),
}

impl Hyperlink {
    pub fn url(self) -> Option<String> {
        match self {
            Hyperlink::Url(url) => Some(url),
            Hyperlink::Action(_) => None,
        }
    }
}

impl PartialEq for Hyperlink {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Url(left), Self::Url(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for Hyperlink {}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct FormattedTextStyles {
    pub weight: Option<CustomWeight>,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub inline_code: bool,
    pub hyperlink: Option<Hyperlink>,
}

impl FormattedTextFragment {
    pub fn plain_text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            styles: Default::default(),
        }
    }

    pub fn weighted(text: impl Into<String>, weight: Option<CustomWeight>) -> Self {
        Self {
            text: text.into(),
            styles: FormattedTextStyles {
                weight,
                ..Default::default()
            },
        }
    }

    pub fn with_weight(&mut self, weight: Option<CustomWeight>) -> &Self {
        self.styles.weight = weight;
        self
    }

    pub fn bold(text: impl Into<String>) -> Self {
        Self::weighted(text, Some(CustomWeight::Bold))
    }

    pub fn italic(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            styles: FormattedTextStyles {
                italic: true,
                ..Default::default()
            },
        }
    }

    pub fn bold_italic(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            styles: FormattedTextStyles {
                weight: Some(CustomWeight::Bold),
                italic: true,
                ..Default::default()
            },
        }
    }

    pub fn hyperlink(tag: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            text: tag.into(),
            styles: FormattedTextStyles {
                hyperlink: Some(Hyperlink::Url(url.into())),
                ..Default::default()
            },
        }
    }

    pub fn hyperlink_action<A: Action>(tag: impl Into<String>, action: A) -> Self {
        Self {
            text: tag.into(),
            styles: FormattedTextStyles {
                hyperlink: Some(Hyperlink::Action(Arc::new(action))),
                ..Default::default()
            },
        }
    }

    pub fn inline_code(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            styles: FormattedTextStyles {
                inline_code: true,
                ..Default::default()
            },
        }
    }

    pub fn strikethrough(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            styles: FormattedTextStyles {
                strikethrough: true,
                ..Default::default()
            },
        }
    }

    pub fn underline(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            styles: FormattedTextStyles {
                underline: true,
                ..Default::default()
            },
        }
    }

    pub fn raw_text(&self) -> &str {
        &self.text
    }
}

impl fmt::Debug for FormattedTextStyles {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut first = true;
        if let Some(weight) = self.weight {
            if !first {
                f.write_str(" | ")?;
            }
            write!(f, "{weight:?}")?;
            first = false;
        }
        if self.italic {
            if !first {
                f.write_str(" | ")?;
            }
            f.write_str("Italic")?;
            first = false;
        }
        if self.strikethrough {
            if !first {
                f.write_str(" | ")?;
            }
            f.write_str("Strikethrough")?;
            first = false;
        }
        if self.inline_code {
            if !first {
                f.write_str(" | ")?;
            }
            f.write_str("InlineCode")?;
            first = false;
        }
        if let Some(link) = &self.hyperlink {
            if !first {
                f.write_str(" | ")?;
            }
            write!(f, "Hyperlink({link:?})")?;
            first = false;
        }
        if first {
            f.write_str("PlainText")?;
        }
        Ok(())
    }
}

fn inline_to_markdown(inline: &FormattedTextInline) -> String {
    let mut result = String::new();
    for fragment in inline {
        let mut text = fragment.text.clone();
        if text.is_empty() {
            continue;
        }
        if fragment.styles.inline_code {
            result.push('`');
            result.push_str(&text);
            result.push('`');
            continue;
        }
        if let Some(Hyperlink::Url(url)) = &fragment.styles.hyperlink {
            text = format!("[{text}]({url})");
        }
        if fragment.styles.strikethrough {
            text = format!("~~{text}~~");
        }
        if fragment.styles.underline {
            text = format!("<u>{text}</u>");
        }
        let is_bold = fragment
            .styles
            .weight
            .is_some_and(|w| matches!(w, CustomWeight::Bold));
        if is_bold && fragment.styles.italic {
            text = format!("***{text}***");
        } else if is_bold {
            text = format!("**{text}**");
        } else if fragment.styles.italic {
            text = format!("*{text}*");
        }
        result.push_str(&text);
    }
    result
}
