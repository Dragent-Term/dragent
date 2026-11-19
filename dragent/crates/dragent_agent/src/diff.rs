//! Line-level diff (unified-diff style hunks) shared by the write/edit
//! tools and the checkpoint store. Pure std-only logic; serde shapes
//! unchanged from the original tool implementation.

/// A unified-diff style hunk showing what changed.
#[derive(Clone, Debug)]
pub struct DiffHunk {
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug)]
pub enum DiffLine {
    Context(String),
    Added(String),
    Removed(String),
}

/// Compute a simple line-level diff between `old` and `new`.
pub fn line_diff(old: &str, new: &str) -> Vec<DiffHunk> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // Naive LCS-based diff using Myers' O(ND) algorithm approximation.
    // For correctness we use a simple patience-diff-lite approach.
    let mut hunks = Vec::new();
    let mut i = 0usize;
    let mut j = 0usize;

    while i < old_lines.len() || j < new_lines.len() {
        // Find common prefix
        if i < old_lines.len() && j < new_lines.len() && old_lines[i] == new_lines[j] {
            i += 1;
            j += 1;
            continue;
        }
        // Scan for next matching pair (up to 8 lines ahead)
        let mut hunk = DiffHunk {
            old_start: i + 1,
            new_start: j + 1,
            lines: Vec::new(),
        };
        let scan = 8usize;
        let mut found = false;
        'outer: for delta in 1..=scan {
            for di in 0..=delta {
                let dj = delta - di;
                if i + di < old_lines.len()
                    && j + dj < new_lines.len()
                    && old_lines[i + di] == new_lines[j + dj]
                {
                    for k in 0..di {
                        hunk.lines
                            .push(DiffLine::Removed(old_lines[i + k].to_string()));
                    }
                    for k in 0..dj {
                        hunk.lines
                            .push(DiffLine::Added(new_lines[j + k].to_string()));
                    }
                    i += di;
                    j += dj;
                    found = true;
                    break 'outer;
                }
            }
        }
        if !found {
            // Emit the rest as removed + added
            while i < old_lines.len() {
                hunk.lines.push(DiffLine::Removed(old_lines[i].to_string()));
                i += 1;
            }
            while j < new_lines.len() {
                hunk.lines.push(DiffLine::Added(new_lines[j].to_string()));
                j += 1;
            }
        }
        if !hunk.lines.is_empty() {
            hunks.push(hunk);
        }
    }

    hunks
}
