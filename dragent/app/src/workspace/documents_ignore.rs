//! Vendored ignore-rule matching for project discovery.
//!
//! Moved verbatim; same behavior.

//! Bounded project-tree discovery for the future file surface.

use dragent_workspace_model::{DocumentEntry, DocumentRef, ProjectDocumentIndex};
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

// Version-control internals and OS thumbnails excluded from the tree —
// never user build output. Build outputs (`node_modules`, `target`,
// `dist`, `build`, `coverage`) are intentionally LISTED: the explorer
// shows everything scanned. The `is_ignored` flag + a future panel toggle
// can hide ignored paths on demand.
pub(crate) mod ignore {
    use std::path::{Path, PathBuf};

    const MAX_PATTERNS: usize = 5_000;

    struct Pattern {
        negated: bool,
        dir_only: bool,
        anchored: bool,
        parts: Vec<String>,
    }

    struct IgnoreFile {
        /// Directory (relative to scan root, `""` = root) holding the file.
        dir_rel: PathBuf,
        patterns: Vec<Pattern>,
    }

    pub struct IgnoreStack {
        files: Vec<IgnoreFile>,
    }

    impl IgnoreStack {
        /// Root-level stack: global excludes, `.git/info/exclude`, root
        /// `.gitignore`. Deeper `.gitignore` files are pushed while descending.
        pub fn for_root(root: &Path) -> Self {
            let mut stack = Self { files: Vec::new() };
            for path in global_exclude_paths() {
                stack.push_file(PathBuf::new(), &path);
            }
            stack.push_file(
                PathBuf::new(),
                &root.join(".git").join("info").join("exclude"),
            );
            stack.push_file(PathBuf::new(), &root.join(".gitignore"));
            stack
        }

        /// Stack for on-demand single-directory listing: root files plus every
        /// ancestor `.gitignore` down to (and including) `dir_rel`.
        pub fn for_dir(root: &Path, dir_rel: &Path) -> Self {
            let mut stack = Self::for_root(root);
            let mut prefix = PathBuf::new();
            for component in dir_rel.components() {
                prefix.push(component);
                let candidate = root.join(&prefix).join(".gitignore");
                stack.push_file(prefix.clone(), &candidate);
                if stack.pattern_count() >= MAX_PATTERNS {
                    break;
                }
            }
            stack
        }

        /// Called when the recursive walk descends into `dir_rel`: loads that
        /// directory's own `.gitignore` (applies to its children). Returns
        /// whether a file was pushed (caller pops after the subtree).
        pub fn descend(&mut self, root: &Path, dir_rel: &Path) -> bool {
            if self.pattern_count() >= MAX_PATTERNS {
                return false;
            }
            let before = self.files.len();
            self.push_file(
                dir_rel.to_path_buf(),
                &root.join(dir_rel).join(".gitignore"),
            );
            self.files.len() > before
        }

        pub fn ascend(&mut self) {
            self.files.pop();
        }

        fn pattern_count(&self) -> usize {
            self.files.iter().map(|file| file.patterns.len()).sum()
        }

        fn push_file(&mut self, dir_rel: PathBuf, path: &Path) {
            if self.pattern_count() >= MAX_PATTERNS {
                return;
            }
            let Ok(content) = std::fs::read_to_string(path) else {
                return;
            };
            let mut patterns = Vec::new();
            for line in content.lines() {
                if let Some(pattern) = parse_line(line) {
                    patterns.push(pattern);
                    if self.pattern_count() + patterns.len() >= MAX_PATTERNS {
                        break;
                    }
                }
            }
            if !patterns.is_empty() {
                self.files.push(IgnoreFile { dir_rel, patterns });
            }
        }

        /// Last matching pattern wins (`true` = ignored). Ancestor-directory
        /// matches propagate to everything beneath (a matched dir hides its
        /// subtree unless a later negation re-includes a specific path).
        pub fn is_ignored(&self, candidate_rel: &Path, is_dir: bool) -> bool {
            let mut ignored = false;
            for file in &self.files {
                let Ok(after) = candidate_rel.strip_prefix(&file.dir_rel) else {
                    continue;
                };
                if after.as_os_str().is_empty() {
                    continue;
                }
                for pattern in &file.patterns {
                    if pattern.matches(after, is_dir) {
                        ignored = !pattern.negated;
                    }
                }
            }
            ignored
        }
    }

    /// Global excludes (best effort, no git subprocess): XDG + home fallbacks.
    fn global_exclude_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Ok(home) = std::env::var("HOME") {
            let home = PathBuf::from(home);
            paths.push(home.join(".config").join("git").join("ignore"));
            paths.push(home.join(".gitignore_global"));
            paths.push(home.join(".gitignore"));
        }
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                paths.push(PathBuf::from(xdg).join("git").join("ignore"));
            }
        }
        paths
    }

    fn parse_line(raw: &str) -> Option<Pattern> {
        // Strip trailing spaces unless escaped (`\ ` keeps the space).
        let mut line = raw.to_string();
        while line.ends_with(' ') && !line.ends_with("\\ ") {
            line.pop();
        }
        if line.is_empty() {
            return None;
        }
        let mut chars = line.as_str();
        // Escaped leading `#` / `!` are literals, not comment/negation.
        let mut negated = false;
        if chars.starts_with("\\#") || chars.starts_with("\\!") {
            chars = &chars[1..];
        } else if chars.starts_with('#') {
            return None;
        } else if chars.starts_with('!') {
            negated = true;
            chars = &chars[1..];
            if chars.starts_with("\\#") || chars.starts_with("\\!") {
                chars = &chars[1..];
            }
        }
        if chars.is_empty() {
            return None;
        }
        let mut anchored = false;
        if chars.starts_with('/') {
            anchored = true;
            chars = chars.trim_start_matches('/');
        }
        let mut dir_only = false;
        if chars.ends_with('/') {
            dir_only = true;
            chars = chars.trim_end_matches('/');
        }
        if chars.is_empty() {
            return None;
        }
        if chars.contains('/') {
            anchored = true;
        }
        let parts: Vec<String> = chars.split('/').map(|part| part.to_string()).collect();
        if parts.iter().all(|part| part.is_empty()) {
            return None;
        }
        Some(Pattern {
            negated,
            dir_only,
            anchored,
            parts,
        })
    }

    impl Pattern {
        fn matches(&self, rel_after: &Path, is_dir: bool) -> bool {
            let names: Vec<String> = rel_after
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect();
            if names.is_empty() {
                return false;
            }
            let pat: Vec<&str> = self.parts.iter().map(|part| part.as_str()).collect();
            // Unanchored patterns match at any depth: implicit leading `**/`.
            let effective: Vec<&str> = if self.anchored {
                pat.clone()
            } else {
                let mut eff = vec!["**"];
                eff.extend(pat);
                eff
            };
            // Full-path match (dir-only patterns require a directory target).
            if match_parts(&effective, &names) {
                return !self.dir_only || is_dir;
            }
            // A pattern matching an ancestor directory ignores the whole
            // subtree beneath it (ancestors are directories by construction).
            for end in 1..names.len() {
                if match_parts(&effective, &names[..end]) {
                    return true;
                }
            }
            false
        }
    }

    /// Component-wise glob match with `**` spanning zero or more components.
    fn match_parts(pat: &[&str], path: &[String]) -> bool {
        if pat.is_empty() {
            return path.is_empty();
        }
        if pat[0] == "**" {
            // `**` matches zero or more components (consecutive `**` collapse).
            let mut rest = &pat[1..];
            while rest.first() == Some(&"**") {
                rest = &rest[1..];
            }
            if rest.is_empty() {
                return true;
            }
            for skip in 0..=path.len() {
                if match_parts(rest, &path[skip..]) {
                    return true;
                }
            }
            return false;
        }
        if path.is_empty() {
            return false;
        }
        if !match_component(pat[0], &path[0]) {
            return false;
        }
        match_parts(&pat[1..], &path[1..])
    }

    /// Single-component match: `*` (no `/`), `?`, and `[...]` classes.
    fn match_component(pat: &str, name: &str) -> bool {
        let pat: Vec<char> = pat.chars().collect();
        let name: Vec<char> = name.chars().collect();
        match_component_chars(&pat, &name)
    }

    fn match_component_chars(pat: &[char], name: &[char]) -> bool {
        if pat.is_empty() {
            return name.is_empty();
        }
        match pat[0] {
            '*' => {
                // `*` spans any (possibly empty) run within the component.
                let mut rest = &pat[1..];
                while rest.first() == Some(&'*') {
                    rest = &rest[1..];
                }
                if rest.is_empty() {
                    return true;
                }
                for skip in 0..=name.len() {
                    if match_component_chars(rest, &name[skip..]) {
                        return true;
                    }
                }
                false
            }
            '?' => {
                if name.is_empty() {
                    return false;
                }
                match_component_chars(&pat[1..], &name[1..])
            }
            '[' => {
                if name.is_empty() {
                    return false;
                }
                let Some(close) = pat.iter().position(|&char| char == ']') else {
                    return pat[0] == name[0] && match_component_chars(&pat[1..], &name[1..]);
                };
                if close <= 1 {
                    return false;
                }
                let (mut class, rest) = (pat[1..close].to_vec(), &pat[close + 1..]);
                let mut negated = false;
                if class.first() == Some(&'!') || class.first() == Some(&'^') {
                    negated = true;
                    class = class[1..].to_vec();
                }
                let mut hit = false;
                let mut index = 0;
                while index < class.len() {
                    if index + 2 < class.len() && class[index + 1] == '-' {
                        if class[index] <= name[0] && name[0] <= class[index + 2] {
                            hit = true;
                        }
                        index += 3;
                    } else {
                        if class[index] == name[0] {
                            hit = true;
                        }
                        index += 1;
                    }
                }
                if hit == negated {
                    return false;
                }
                match_component_chars(rest, &name[1..])
            }
            '\\' => {
                // Escaped literal (`\*`, `\?`, `\[`, `\\`).
                if pat.len() >= 2 {
                    if name.is_empty() || pat[1] != name[0] {
                        return false;
                    }
                    match_component_chars(&pat[2..], &name[1..])
                } else {
                    name.is_empty()
                }
            }
            literal => {
                if name.is_empty() || literal != name[0] {
                    return false;
                }
                match_component_chars(&pat[1..], &name[1..])
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn ignored_with_files(root_files: &[(&str, &str)], candidate: &str, is_dir: bool) -> bool {
            let dir = std::env::temp_dir().join(format!(
                "dragent-ignore-test-{}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                candidate.replace('/', "_")
            ));
            for (rel, content) in root_files {
                let path = dir.join(rel);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, content).unwrap();
            }
            let stack = IgnoreStack::for_root(&dir);
            let result = stack.is_ignored(Path::new(candidate), is_dir);
            let _ = std::fs::remove_dir_all(&dir);
            result
        }

        #[test]
        fn basename_star_and_negation() {
            let files = [(".gitignore", "*.log\n!important.log\n")];
            assert!(ignored_with_files(&files, "debug.log", false));
            assert!(ignored_with_files(&files, "sub/debug.log", false));
            assert!(!ignored_with_files(&files, "important.log", false));
        }

        #[test]
        fn dir_only_slash_and_subtree() {
            let files = [(".gitignore", "build/\n")];
            assert!(ignored_with_files(&files, "build", true));
            assert!(ignored_with_files(&files, "build/out.js", false));
            assert!(!ignored_with_files(&files, "buildx", true));
            // No slash in pattern: matches a `build` dir at any depth.
            assert!(ignored_with_files(&files, "src/build/out.js", false));
        }

        #[test]
        fn anchored_leading_slash() {
            let files = [(".gitignore", "/root-only.log\n")];
            assert!(ignored_with_files(&files, "root-only.log", false));
            assert!(!ignored_with_files(&files, "sub/root-only.log", false));
        }

        #[test]
        fn double_star_recursive() {
            let files = [(".gitignore", "a/**/b\n")];
            assert!(ignored_with_files(&files, "a/b", false));
            assert!(ignored_with_files(&files, "a/x/y/b", false));
            assert!(!ignored_with_files(&files, "a/x", false));
        }
    }
}
