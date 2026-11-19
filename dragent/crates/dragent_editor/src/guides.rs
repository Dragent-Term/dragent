//! Structural indentation-guide geometry shared by editor surfaces.
//!
//! Guide ranges come from [`FoldIndex`]. Leading whitespace only places a
//! structurally identified range on the horizontal text grid.

use crate::{FoldIndex, Language};

const DEFAULT_TAB_WIDTH: usize = 4;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndentGuideIndex {
    columns_by_line: Vec<Vec<usize>>,
}

impl IndentGuideIndex {
    /// Plain text has no syntax ranges and therefore no structural guides.
    pub fn from_text(text: &str) -> Self {
        Self::for_language(Language::PlainText, text)
    }

    pub fn with_tab_width(text: &str, tab_width: usize) -> Self {
        Self::for_language_with_tab_width(Language::PlainText, text, tab_width)
    }

    pub fn for_language(language: Language, text: &str) -> Self {
        Self::for_language_with_tab_width(language, text, DEFAULT_TAB_WIDTH)
    }

    pub fn for_language_with_tab_width(language: Language, text: &str, tab_width: usize) -> Self {
        let tab_width = tab_width.max(1);
        let lines = text.lines().collect::<Vec<_>>();
        let mut columns_by_line = vec![Vec::new(); lines.len()];
        let ranges = FoldIndex::from_text(language, text);

        for region in ranges.regions() {
            let Some(guide_lines) = region.guide_line_range() else {
                continue;
            };
            // Re-measure the opener when a caller uses a non-default tab width.
            // The parser still owns the vertical range; this is geometry only.
            let column = lines
                .get(region.start_line.saturating_sub(1))
                .map(|line| leading_columns(line, tab_width))
                .unwrap_or(region.indent_column);
            for line in guide_lines {
                if let Some(columns) = columns_by_line.get_mut(line.saturating_sub(1)) {
                    columns.push(column);
                }
            }
        }

        for columns in &mut columns_by_line {
            columns.sort_unstable();
            columns.dedup();
        }
        Self { columns_by_line }
    }

    /// One-based source line, matching gutter and breakpoint coordinates.
    pub fn columns(&self, line: usize) -> &[usize] {
        line.checked_sub(1)
            .and_then(|index| self.columns_by_line.get(index))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

fn leading_columns(line: &str, tab_width: usize) -> usize {
    line.chars()
        .take_while(|character| character.is_whitespace())
        .fold(0, |columns, character| {
            if character == '\t' {
                columns + (tab_width - columns % tab_width)
            } else {
                columns + 1
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_nested_syntax_ranges_onto_source_lines() {
        let index = IndentGuideIndex::for_language(
            Language::Rust,
            "fn main() {\n    if ready {\n        run();\n    }\n}\n",
        );
        assert_eq!(index.columns(1), &[]);
        assert_eq!(index.columns(2), &[0]);
        assert_eq!(index.columns(3), &[0, 4]);
        assert_eq!(index.columns(4), &[0]);
        assert_eq!(index.columns(5), &[]);
    }

    #[test]
    fn connects_structural_guides_across_blank_lines() {
        let index = IndentGuideIndex::for_language(
            Language::Rust,
            "fn main() {\n    one();\n\n    two();\n}\n",
        );
        assert_eq!(index.columns(3), &[0]);
    }

    #[test]
    fn supports_parentheses_and_jsx_but_not_single_line_regions() {
        let index = IndentGuideIndex::for_language(
            Language::TypeScript,
            "call(\n    <div>\n        <span>value</span>\n    </div>\n);\n",
        );
        assert_eq!(index.columns(2), &[0]);
        assert_eq!(index.columns(3), &[0, 4]);
        assert_eq!(index.columns(4), &[0]);
        assert_eq!(index.columns(5), &[]);
    }

    #[test]
    fn indentation_without_a_syntax_range_does_not_create_guides() {
        let index = IndentGuideIndex::for_language(
            Language::Rust,
            "root\n    child\n        grandchild\nroot\n",
        );
        assert!(index.columns(2).is_empty());
        assert!(index.columns(3).is_empty());
    }

    #[test]
    fn tabs_place_nested_ranges_at_the_requested_tab_stop() {
        let index = IndentGuideIndex::for_language_with_tab_width(
            Language::Rust,
            "fn main() {\n\tif ready {\n\t\trun();\n\t}\n}\n",
            8,
        );
        assert_eq!(index.columns(3), &[0, 8]);
    }
}
