use crate::Language;
use arborium::tree_sitter::{InputEdit, Node, Parser, Point, Tree};
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FoldRegionKind {
    Brace,
    Parenthesis,
    Bracket,
    Tag,
    Comment,
    Indentation,
    Heading,
}

/// Exact syntax range replaced by a presentation-only placeholder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldRegion {
    pub id: u64,
    pub start_line: usize,
    pub end_line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub indent_column: usize,
    pub kind: FoldRegionKind,
    pub placeholder: String,
}

impl FoldRegion {
    pub fn guide_line_range(&self) -> Option<std::ops::RangeInclusive<usize>> {
        (self.end_line > self.start_line)
            .then(|| (self.start_line + 1)..=self.end_line.saturating_sub(1))
    }

    pub fn folded_line(&self, source: &str) -> Option<String> {
        let line_start = line_start_byte(source, self.start_line)?;
        let line_end = line_end_byte(source, self.end_line)?;
        if self.start_byte < line_start
            || self.end_byte < self.start_byte
            || self.end_byte > line_end
        {
            return None;
        }
        Some(format!(
            "{}{}{}",
            &source[line_start..self.start_byte],
            self.placeholder,
            &source[self.end_byte..line_end]
        ))
    }
}

#[derive(Clone, Debug, Default)]
pub struct FoldIndex {
    regions: Vec<FoldRegion>,
    by_start: BTreeMap<usize, Vec<usize>>,
    by_id: BTreeMap<u64, usize>,
}

impl FoldIndex {
    pub fn from_text(language: Language, text: &str) -> Self {
        let regions = match language {
            Language::Python | Language::Yaml => indentation_regions(language, text),
            Language::Markdown => markdown_regions(text),
            Language::PlainText => Vec::new(),
            _ => syntax_regions(language, text).unwrap_or_default(),
        };
        Self::from_regions(regions)
    }

    fn from_regions(mut regions: Vec<FoldRegion>) -> Self {
        regions.retain(|region| region.end_line > region.start_line);
        regions.sort_by_key(|region| {
            (
                region.start_line,
                region.end_line,
                region.start_byte,
                region.end_byte,
            )
        });
        regions.dedup_by(|left, right| {
            left.start_byte == right.start_byte && left.end_byte == right.end_byte
        });
        let mut identity_occurrences = BTreeMap::<u64, u32>::new();
        for region in &mut regions {
            let occurrence = identity_occurrences.entry(region.id).or_default();
            if *occurrence > 0 {
                let mut hasher = DefaultHasher::new();
                region.id.hash(&mut hasher);
                occurrence.hash(&mut hasher);
                region.id = hasher.finish();
            }
            *occurrence += 1;
        }
        let mut by_start = BTreeMap::<usize, Vec<usize>>::new();
        let mut by_id = BTreeMap::new();
        for (index, region) in regions.iter().enumerate() {
            by_start.entry(region.start_line).or_default().push(index);
            by_id.insert(region.id, index);
        }
        // One gutter cell exposes one range. Prefer the smallest independent
        // syntax node, such as an object pattern inside formal parameters.
        for candidates in by_start.values_mut() {
            candidates.sort_by_key(|index| {
                let region = &regions[*index];
                (region.end_byte - region.start_byte, region.start_byte)
            });
        }
        Self {
            regions,
            by_start,
            by_id,
        }
    }

    pub fn region_starting_at(&self, line: usize) -> Option<&FoldRegion> {
        self.by_start
            .get(&line)
            .and_then(|regions| regions.first())
            .map(|index| &self.regions[*index])
    }

    pub fn regions(&self) -> impl Iterator<Item = &FoldRegion> {
        self.regions.iter()
    }

    pub fn innermost_region_at(&self, line: usize) -> Option<&FoldRegion> {
        self.regions
            .iter()
            .filter(|region| region.start_line < line && line <= region.end_line)
            .min_by_key(|region| region.end_byte - region.start_byte)
    }

    fn region_by_id(&self, id: u64) -> Option<&FoldRegion> {
        self.by_id.get(&id).map(|index| &self.regions[*index])
    }
}

/// Reusable Tree-sitter state for fold discovery. The editor moves this value
/// to its background worker, applies the latest text edit to the existing
/// tree, and receives it back with the resulting fold snapshot.
pub struct FoldParser {
    language: Language,
    parser: Option<Parser>,
    tree: Option<Tree>,
    text: String,
}

impl FoldParser {
    pub fn parse(self, language: Language, text: String) -> (Self, FoldIndex) {
        if self.language != language || self.parser.is_none() {
            return Self::initial(language, text);
        }
        let mut parser = self.parser.expect("parser checked above");
        let mut tree = self.tree;
        if let Some(existing) = tree.as_mut() {
            existing.edit(&input_edit(&self.text, &text));
        }
        tree = parser.parse(text.as_str(), tree.as_ref());
        let index = tree
            .as_ref()
            .map(|tree| {
                let mut regions = Vec::new();
                collect_syntax_regions(tree.root_node(), &text, &mut regions);
                FoldIndex::from_regions(regions)
            })
            .unwrap_or_default();
        (
            Self {
                language,
                parser: Some(parser),
                tree,
                text,
            },
            index,
        )
    }

    pub fn initial(language: Language, text: String) -> (Self, FoldIndex) {
        let Some(grammar_name) = syntax_grammar_name(language) else {
            let index = FoldIndex::from_text(language, &text);
            return (
                Self {
                    language,
                    parser: None,
                    tree: None,
                    text,
                },
                index,
            );
        };
        let Some(grammar) = arborium::get_language(grammar_name) else {
            return (
                Self {
                    language,
                    parser: None,
                    tree: None,
                    text,
                },
                FoldIndex::default(),
            );
        };
        let mut parser = Parser::new();
        if parser.set_language(&grammar).is_err() {
            return (
                Self {
                    language,
                    parser: None,
                    tree: None,
                    text,
                },
                FoldIndex::default(),
            );
        }
        let tree = parser.parse(text.as_str(), None);
        let index = tree
            .as_ref()
            .map(|tree| {
                let mut regions = Vec::new();
                collect_syntax_regions(tree.root_node(), &text, &mut regions);
                FoldIndex::from_regions(regions)
            })
            .unwrap_or_default();
        (
            Self {
                language,
                parser: Some(parser),
                tree,
                text,
            },
            index,
        )
    }
}

fn input_edit(old: &str, new: &str) -> InputEdit {
    let mut start_byte = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(left, right)| left == right)
        .count();
    while start_byte > 0 && (!old.is_char_boundary(start_byte) || !new.is_char_boundary(start_byte))
    {
        start_byte -= 1;
    }
    let max_suffix = old.len().min(new.len()).saturating_sub(start_byte);
    let mut suffix = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(max_suffix)
        .take_while(|(left, right)| left == right)
        .count();
    while suffix > 0
        && (!old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix))
    {
        suffix -= 1;
    }
    let old_end_byte = old.len() - suffix;
    let new_end_byte = new.len() - suffix;
    InputEdit {
        start_byte,
        old_end_byte,
        new_end_byte,
        start_position: point_for_byte(old, start_byte),
        old_end_position: point_for_byte(old, old_end_byte),
        new_end_position: point_for_byte(new, new_end_byte),
    }
}

fn point_for_byte(text: &str, byte: usize) -> Point {
    let prefix = &text[..byte.min(text.len())];
    let row = prefix.bytes().filter(|value| *value == b'\n').count();
    let column = prefix
        .rfind('\n')
        .map_or(prefix.len(), |line| prefix.len() - line - 1);
    Point::new(row, column)
}

#[derive(Clone, Debug, Default)]
pub struct FoldState {
    collapsed: BTreeSet<u64>,
    revision: u64,
}

impl FoldState {
    pub fn toggle(&mut self, line: usize, index: &FoldIndex) -> Option<bool> {
        let id = index.region_starting_at(line)?.id;
        let collapsed = if self.collapsed.remove(&id) {
            false
        } else {
            self.collapsed.insert(id);
            true
        };
        self.revision = self.revision.wrapping_add(1);
        Some(collapsed)
    }

    pub fn is_collapsed(&self, line: usize, index: &FoldIndex) -> bool {
        index
            .region_starting_at(line)
            .is_some_and(|region| self.collapsed.contains(&region.id))
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_line_hidden(&self, line: usize, index: &FoldIndex) -> bool {
        self.collapsed.iter().any(|id| {
            index
                .region_by_id(*id)
                .is_some_and(|region| line > region.start_line && line <= region.end_line)
        })
    }

    /// Expand every collapsed region that hides `line`.
    ///
    /// Navigation targets must never remain concealed inside a fold. Keeping
    /// this operation on `FoldState` also avoids exposing fold identities to
    /// editor views.
    pub fn unfold_line(&mut self, line: usize, index: &FoldIndex) -> bool {
        let before = self.collapsed.len();
        self.collapsed.retain(|id| {
            !index
                .region_by_id(*id)
                .is_some_and(|region| line > region.start_line && line <= region.end_line)
        });
        let changed = self.collapsed.len() != before;
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        changed
    }

    pub fn retain_valid(&mut self, index: &FoldIndex) {
        let before = self.collapsed.len();
        self.collapsed
            .retain(|id| index.region_by_id(*id).is_some());
        if self.collapsed.len() != before {
            self.revision = self.revision.wrapping_add(1);
        }
    }
}

fn syntax_regions(language: Language, text: &str) -> Option<Vec<FoldRegion>> {
    let total_started = std::time::Instant::now();
    let grammar_name = syntax_grammar_name(language)?;
    let grammar = arborium::get_language(grammar_name)?;
    let mut parser = Parser::new();
    parser.set_language(&grammar).ok()?;
    let parse_started = std::time::Instant::now();
    let tree = parser.parse(text, None)?;
    let parse_elapsed = parse_started.elapsed();
    let mut regions = Vec::new();
    let collect_started = std::time::Instant::now();
    collect_syntax_regions(tree.root_node(), text, &mut regions);
    let collect_elapsed = collect_started.elapsed();
    if total_started.elapsed() > std::time::Duration::from_millis(50) {
        log::warn!(
            "[editor-edit] stage=fold_discovery bytes={} parse_ms={:.3} collect_ms={:.3} total_ms={:.3}",
            text.len(),
            parse_elapsed.as_secs_f64() * 1_000.0,
            collect_elapsed.as_secs_f64() * 1_000.0,
            total_started.elapsed().as_secs_f64() * 1_000.0,
        );
    }
    Some(regions)
}

pub(crate) fn syntax_grammar_name(language: Language) -> Option<&'static str> {
    Some(match language {
        Language::Rust => "rust",
        // The current language enum groups .ts and .tsx. TSX accepts both and
        // provides the JSX nodes the editor needs.
        Language::TypeScript => "tsx",
        Language::JavaScript => "javascript",
        Language::Go => "go",
        Language::Json => "json",
        Language::Html => "html",
        Language::Css => "css",
        Language::Shell => "bash",
        Language::Toml => "toml",
        Language::Hcl => "hcl",
        _ => return None,
    })
}

fn collect_syntax_regions(node: Node<'_>, source: &str, output: &mut Vec<FoldRegion>) {
    let start = node.start_position();
    let end = node.end_position();
    if end.row > start.row {
        if let Some(region) = inline_brace_body_region(node, source) {
            output.push(region);
        } else if let Some((kind, placeholder)) = node_fold_presentation(node, source) {
            output.push(FoldRegion {
                id: fold_identity(
                    kind,
                    node.kind(),
                    source.get(node.byte_range()).unwrap_or_default(),
                    source,
                    node.start_byte(),
                ),
                start_line: start.row + 1,
                end_line: end.row + 1,
                start_byte: node.start_byte(),
                end_byte: node.end_byte(),
                indent_column: leading_columns_on_line(source, start.row + 1),
                kind,
                placeholder,
            });
        }
    }
    for child_index in 0..node.child_count() {
        if let Some(child) = node.child(child_index as u32) {
            collect_syntax_regions(child, source, output);
        }
    }
}

/// Some grammars have no separate block node for a statement's body: Go's
/// `switch`, `select` and type switch put `{`, the cases and `}` directly
/// inside the statement node, whose text starts with `switch`, not `{`. The
/// body is still the span between the node's own `{` and `}` tokens.
fn inline_brace_body_region(node: Node<'_>, source: &str) -> Option<FoldRegion> {
    if source.get(node.byte_range())?.trim_start().starts_with('{') {
        return None;
    }
    let mut open = None;
    let mut close = None;
    for index in 0..node.child_count() {
        let child = node.child(index as u32)?;
        match child.kind() {
            "{" if open.is_none() => open = Some(child),
            "}" => close = Some(child),
            _ => {}
        }
    }
    let (open, close) = (open?, close?);
    if close.start_position().row <= open.start_position().row {
        return None;
    }
    let kind = FoldRegionKind::Brace;
    Some(FoldRegion {
        id: fold_identity(
            kind,
            node.kind(),
            source
                .get(open.start_byte()..close.end_byte())
                .unwrap_or_default(),
            source,
            open.start_byte(),
        ),
        start_line: open.start_position().row + 1,
        end_line: close.start_position().row + 1,
        start_byte: open.start_byte(),
        end_byte: close.end_byte(),
        indent_column: leading_columns_on_line(source, open.start_position().row + 1),
        kind,
        placeholder: "{...}".to_string(),
    })
}

fn node_fold_presentation(node: Node<'_>, source: &str) -> Option<(FoldRegionKind, String)> {
    let text = source.get(node.byte_range())?;
    let trimmed = text.trim();
    if node.kind() == "jsx_element" || node.kind() == "element" {
        return tag_placeholder(trimmed).map(|placeholder| (FoldRegionKind::Tag, placeholder));
    }
    if node.kind() == "comment" && trimmed.starts_with("/*") && trimmed.ends_with("*/") {
        return Some((FoldRegionKind::Comment, "/*...*/".to_string()));
    }
    match (trimmed.as_bytes().first(), trimmed.as_bytes().last()) {
        (Some(b'{'), Some(b'}')) => Some((FoldRegionKind::Brace, "{...}".to_string())),
        (Some(b'['), Some(b']')) => Some((FoldRegionKind::Bracket, "[...]".to_string())),
        (Some(b'('), Some(b')')) => Some((FoldRegionKind::Parenthesis, "(...)".to_string())),
        _ => None,
    }
}

fn tag_placeholder(text: &str) -> Option<String> {
    let opening_end = text.find('>')?;
    let opening = &text[..=opening_end];
    if opening.ends_with("/>") {
        return None;
    }
    let name = opening
        .strip_prefix('<')?
        .chars()
        .take_while(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | ':' | '.')
        })
        .collect::<String>();
    if name.is_empty() {
        return None;
    }
    let attributes = opening[1 + name.len()..opening.len() - 1].trim();
    Some(if attributes.is_empty() && !opening.contains('\n') {
        format!("<{name}>...</{name}>")
    } else {
        format!("<{name} ...>...</{name}>")
    })
}

fn fold_identity(
    kind: FoldRegionKind,
    node_kind: &str,
    node_text: &str,
    source: &str,
    start_byte: usize,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    kind.hash(&mut hasher);
    node_kind.hash(&mut hasher);
    // Include the opener's visible prefix (function name, condition, binding)
    // and normalized node contents. This keeps a fold attached when lines are
    // inserted above it while avoiding raw line-number identities.
    let line_start = source[..start_byte.min(source.len())]
        .rfind('\n')
        .map_or(0, |byte| byte + 1);
    source[line_start..start_byte.min(source.len())]
        .trim()
        .hash(&mut hasher);
    for token in node_text.split_whitespace() {
        token.hash(&mut hasher);
    }
    hasher.finish()
}

fn indentation_regions(language: Language, text: &str) -> Vec<FoldRegion> {
    let lines = text.lines().collect::<Vec<_>>();
    let starts = line_starts(text);
    let mut regions = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let starts_block = matches!(language, Language::Python | Language::Yaml)
            && trimmed.ends_with(':')
            && !trimmed.starts_with('#');
        if !starts_block {
            continue;
        }
        let indent = leading_columns(line, 4);
        let mut end_index = index;
        for (next_index, next) in lines.iter().enumerate().skip(index + 1) {
            if next.trim().is_empty() {
                end_index = next_index;
                continue;
            }
            if leading_columns(next, 4) <= indent {
                break;
            }
            end_index = next_index;
        }
        if end_index > index {
            regions.push(FoldRegion {
                id: fold_identity(
                    FoldRegionKind::Indentation,
                    "indentation",
                    &text[starts[index]..starts[end_index] + lines[end_index].len()],
                    text,
                    starts[index],
                ),
                start_line: index + 1,
                end_line: end_index + 1,
                start_byte: starts[index] + line.len(),
                end_byte: starts[end_index] + lines[end_index].len(),
                indent_column: indent,
                kind: FoldRegionKind::Indentation,
                placeholder: " ...".to_string(),
            });
        }
    }
    regions
}

fn markdown_regions(text: &str) -> Vec<FoldRegion> {
    let lines = text.lines().collect::<Vec<_>>();
    let starts = line_starts(text);
    let mut regions = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(level) = heading_level(line) else {
            continue;
        };
        let mut end_index = lines.len().saturating_sub(1);
        for (next_index, next) in lines.iter().enumerate().skip(index + 1) {
            if heading_level(next).is_some_and(|next_level| next_level <= level) {
                end_index = next_index.saturating_sub(1);
                break;
            }
        }
        if end_index > index {
            regions.push(FoldRegion {
                id: fold_identity(
                    FoldRegionKind::Heading,
                    "heading",
                    &text[starts[index]..starts[end_index] + lines[end_index].len()],
                    text,
                    starts[index],
                ),
                start_line: index + 1,
                end_line: end_index + 1,
                start_byte: starts[index] + line.len(),
                end_byte: starts[end_index] + lines[end_index].len(),
                indent_column: leading_columns(line, 4),
                kind: FoldRegionKind::Heading,
                placeholder: " ...".to_string(),
            });
        }
    }
    regions
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(byte, _)| byte + 1));
    starts
}

fn line_start_byte(text: &str, line: usize) -> Option<usize> {
    line_starts(text).get(line.checked_sub(1)?).copied()
}

fn line_end_byte(text: &str, line: usize) -> Option<usize> {
    let start = line_start_byte(text, line)?;
    Some(
        text[start..]
            .find('\n')
            .map_or(text.len(), |offset| start + offset),
    )
}

fn leading_columns_on_line(text: &str, line: usize) -> usize {
    let Some(start) = line_start_byte(text, line) else {
        return 0;
    };
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |offset| start + offset);
    leading_columns(&text[start..end], 4)
}

fn leading_columns(line: &str, tab_width: usize) -> usize {
    line.chars()
        .take_while(|character| character.is_whitespace())
        .fold(0, |column, character| {
            if character == '\t' {
                column + tab_width - column % tab_width
            } else {
                column + 1
            }
        })
}

fn heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let level = trimmed
        .chars()
        .take_while(|character| *character == '#')
        .count();
    (level > 0 && trimmed.chars().nth(level).is_some_and(char::is_whitespace)).then_some(level)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn folded(language: Language, source: &str, line: usize) -> String {
        FoldIndex::from_text(language, source)
            .region_starting_at(line)
            .and_then(|region| region.folded_line(source))
            .expect("foldable line")
    }

    #[test]
    fn go_switch_body_is_a_region_even_without_a_block_node() {
        let source = "func f(x int) int {\n\tswitch x {\n\tcase 1:\n\t\treturn 1\n\tdefault:\n\t\treturn 0\n\t}\n}\n";
        let index = FoldIndex::from_text(Language::Go, source);
        let switch = index.region_starting_at(2).expect("switch body region");
        assert_eq!((switch.start_line, switch.end_line), (2, 7));
        assert_eq!(switch.indent_column, 4);
        assert_eq!(folded(Language::Go, source, 2), "\tswitch x {...}");
        // The function body keeps its own region.
        assert_eq!(index.region_starting_at(1).map(|r| r.end_line), Some(8));
    }

    #[test]
    fn syntax_aware_placeholders_preserve_prefix_and_suffix() {
        assert_eq!(
            folded(
                Language::TypeScript,
                "function test() {\n  return true;\n}\n",
                1
            ),
            "function test() {...}"
        );
        assert_eq!(
            folded(
                Language::TypeScript,
                "const user = {\n  name: 'John'\n};\n",
                1
            ),
            "const user = {...};"
        );
        assert_eq!(
            folded(
                Language::TypeScript,
                "const users = [\n  one,\n  two\n];\n",
                1
            ),
            "const users = [...];"
        );
        assert_eq!(
            folded(Language::TypeScript, "call(\n  one,\n  two\n);\n", 1),
            "call(...);"
        );
    }

    #[test]
    fn folds_jsx_elements_with_their_real_tag_name() {
        let source = "<div>\n  <span>Hello</span>\n</div>\n";
        assert_eq!(folded(Language::TypeScript, source, 1), "<div>...</div>");
    }

    #[test]
    fn complex_signature_folds_destructuring_without_removing_type() {
        let source = "function FieldValuePreview({\n  fieldType,\n  raw,\n}: {\n  fieldType: string;\n  raw: unknown;\n}) {\n  return null;\n}\n";
        assert_eq!(
            folded(Language::TypeScript, source, 1),
            "function FieldValuePreview({...}: {"
        );
        assert_eq!(folded(Language::TypeScript, source, 7), "}) {...}");
    }

    #[test]
    fn complex_tsx_keeps_comments_templates_and_nested_regions_structural() {
        let source = r####"function FieldValuePreview({
  fieldType,
  raw,
  onDocument = false,
}: {
  fieldType: string;
  raw: unknown;
  /** When true, style as ink stamped on the PDF */
  onDocument?: boolean;
}) {
  if (fieldType === "checkbox") {
    return (
      <span
        className={`text ${onDocument ? `nested ${raw}` : "other"}`}
      >
        ✓
      </span>
    );
  }
  return null;
}
"####;
        let index = FoldIndex::from_text(Language::TypeScript, source);
        assert_eq!(
            index
                .region_starting_at(1)
                .unwrap()
                .folded_line(source)
                .unwrap(),
            "function FieldValuePreview({...}: {"
        );
        assert_eq!(
            index
                .region_starting_at(10)
                .unwrap()
                .folded_line(source)
                .unwrap(),
            "}) {...}"
        );
        assert_eq!(
            index
                .region_starting_at(13)
                .unwrap()
                .folded_line(source)
                .unwrap(),
            "      <span ...>...</span>"
        );
        assert!(index
            .regions()
            .any(|region| region.start_line == 11 && region.kind == FoldRegionKind::Brace));
    }

    #[test]
    fn nested_fold_state_survives_parent_visibility_changes() {
        let source = "function test() {\n  if (ready) {\n    run();\n  }\n}\n";
        let index = FoldIndex::from_text(Language::TypeScript, source);
        let mut state = FoldState::default();
        assert_eq!(state.toggle(2, &index), Some(true));
        assert_eq!(state.toggle(1, &index), Some(true));
        assert!(state.is_line_hidden(2, &index));
        assert_eq!(state.toggle(1, &index), Some(false));
        assert!(state.is_collapsed(2, &index));
        assert!(state.is_line_hidden(3, &index));
    }

    #[test]
    fn navigation_unfolds_every_region_hiding_the_target_line() {
        let source = "function test() {\n  if (ready) {\n    run();\n  }\n}\n";
        let index = FoldIndex::from_text(Language::TypeScript, source);
        let mut state = FoldState::default();
        assert_eq!(state.toggle(2, &index), Some(true));
        assert_eq!(state.toggle(1, &index), Some(true));
        assert!(state.is_line_hidden(3, &index));

        assert!(state.unfold_line(3, &index));
        assert!(!state.is_line_hidden(3, &index));
        assert!(!state.is_collapsed(1, &index));
        assert!(!state.is_collapsed(2, &index));
    }

    #[test]
    fn fold_identity_survives_lines_inserted_above_the_region() {
        let source = "function test() {\n  return true;\n}\n";
        let index = FoldIndex::from_text(Language::TypeScript, source);
        let mut state = FoldState::default();
        assert_eq!(state.toggle(1, &index), Some(true));

        let moved = "// inserted above\n\nfunction test() {\n  return true;\n}\n";
        let moved_index = FoldIndex::from_text(Language::TypeScript, moved);
        assert!(state.is_collapsed(3, &moved_index));
        assert!(state.is_line_hidden(4, &moved_index));
    }

    #[test]
    fn single_line_constructs_are_not_foldable() {
        let source =
            "const user = { name: 'John' };\nconst values = [1, 2, 3];\n<span>Hello</span>\n";
        assert!(FoldIndex::from_text(Language::TypeScript, source)
            .regions()
            .next()
            .is_none());
    }

    #[test]
    fn incremental_fold_parser_updates_existing_tree() {
        let source = "function test() {\n  return true;\n}\n".to_string();
        let (parser, first) = FoldParser::initial(Language::TypeScript, source.clone());
        assert!(first.region_starting_at(1).is_some());

        let edited = format!("// above\n{source}");
        let (_, second) = parser.parse(Language::TypeScript, edited);
        assert!(second.region_starting_at(2).is_some());
    }

    /// Development-only scaling probe used when diagnosing editor stalls.
    /// Keep this ignored so normal test runs stay fast and deterministic.
    #[test]
    #[ignore = "manual editor performance profile"]
    fn profile_full_typescript_fold_rebuild() {
        for target_bytes in [100_000usize, 1_000_000, 5_000_000] {
            let unit = "export function component(value: number) {\n  if (value > 0) {\n    return { value, nested: [value, value + 1] };\n  }\n  return null;\n}\n";
            let source = unit.repeat(target_bytes.div_ceil(unit.len()));
            let started = Instant::now();
            let index = FoldIndex::from_text(Language::TypeScript, &source);
            eprintln!(
                "[editor-profile] fold_rebuild bytes={} regions={} elapsed_ms={:.3}",
                source.len(),
                index.regions().count(),
                started.elapsed().as_secs_f64() * 1_000.0
            );
        }
    }

    #[test]
    #[ignore = "manual editor performance profile"]
    fn profile_incremental_typescript_fold_update() {
        let target_bytes = 100_000usize;
        let unit = "export function component(value: number) {\n  if (value > 0) {\n    return { value, nested: [value, value + 1] };\n  }\n  return null;\n}\n";
        let source = unit.repeat(target_bytes.div_ceil(unit.len()));
        let (parser, _) = FoldParser::initial(Language::TypeScript, source.clone());
        let mut edited = source;
        edited.insert(edited.len() / 2, 'x');
        let started = Instant::now();
        let (_, index) = parser.parse(Language::TypeScript, edited);
        eprintln!(
            "[editor-profile] fold_incremental bytes={} regions={} elapsed_ms={:.3}",
            target_bytes,
            index.regions().count(),
            started.elapsed().as_secs_f64() * 1_000.0
        );
    }
}
