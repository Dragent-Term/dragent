use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FsEntry {
    pub name: String,
    pub is_dir: bool,
    pub display_path: String,
    pub description: Option<String>,
    pub match_type: MatchType,
    pub fuzzy_score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchType {
    Exact,
    CaseInsensitiveExact,
    Prefix,
    Fuzzy,
}

#[derive(Debug, Clone)]
pub struct FsSuggestRequest {
    pub replace_start: usize,
    pub replace_end: usize,
    pub entries: Vec<FsEntry>,
    pub browse_root_label: String,
}

/// Returns true when tab / live suggest should offer filesystem completions.
pub fn should_suggest_paths(buffer: &str) -> bool {
    if cd_path_token(buffer).is_some() {
        return true;
    }
    if buffer.ends_with(char::is_whitespace) {
        return false;
    }
    let Some(word) = buffer.split_whitespace().last() else {
        return false;
    };
    word.starts_with('/')
        || word.starts_with("./")
        || word.starts_with("../")
        || word.starts_with('~')
        || word.contains('/')
}

/// Path fragment for a `cd` command, if the buffer is completing one.
/// `cd` alone yields an empty fragment (list cwd).
pub fn cd_path_token(buffer: &str) -> Option<&str> {
    let trimmed = buffer.trim_end();
    if trimmed == "cd" {
        return Some("");
    }
    trimmed.strip_prefix("cd ")
}

pub fn build_fs_suggest(buffer: &str, cwd: &Path) -> Option<FsSuggestRequest> {
    let (replace_start, replace_end, last_word) = if let Some(path_token) = cd_path_token(buffer) {
        let trimmed_end = buffer.trim_end().len();
        let start = if buffer.trim_end() == "cd" {
            trimmed_end
        } else {
            "cd ".len()
        };
        // Include trailing whitespace in the replaced span so `cd ` → `cd <dir>`.
        (start, buffer.len(), path_token)
    } else if let Some(idx) = buffer.rfind(|c: char| c.is_whitespace() || c == '=' || c == ':') {
        (idx + 1, buffer.len(), &buffer[idx + 1..])
    } else {
        (0, buffer.len(), buffer)
    };

    let mut entries = Vec::new();
    let mut browse_root_label = String::new();

    if let Some(path_token) = cd_path_token(buffer) {
        // CDPATH logic
        let (dirs_to_search, prefix) = resolve_cdpath_directories(path_token, cwd);
        let mut seen = std::collections::HashSet::new();

        for dir in dirs_to_search {
            if browse_root_label.is_empty() {
                browse_root_label = dir.display().to_string();
            }
            if let Some(mut matched) = list_matching_entries(&dir, &prefix) {
                for entry in matched.drain(..) {
                    if entry.is_dir && seen.insert(entry.name.clone()) {
                        entries.push(entry);
                    }
                }
            }
        }
    } else {
        // Normal path completion logic
        if let Some((directory, prefix)) = resolve_directory_and_prefix(last_word, cwd) {
            browse_root_label = directory.display().to_string();
            if let Some(matched) = list_matching_entries(&directory, &prefix) {
                entries = matched;
            }
        }
    }

    if entries.is_empty() {
        return None;
    }

    // Sort entries: Exact > CaseInsensitiveExact > Prefix > Fuzzy, then by dir/file, then alphabetically.
    entries.sort_by(|a, b| {
        a.match_type
            .cmp(&b.match_type)
            .then_with(|| {
                b.fuzzy_score
                    .partial_cmp(&a.fuzzy_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            })
    });

    Some(FsSuggestRequest {
        replace_start,
        replace_end,
        entries,
        browse_root_label,
    })
}

fn resolve_cdpath_directories(token: &str, cwd: &Path) -> (Vec<PathBuf>, String) {
    let mut dirs_to_search = Vec::new();

    // If it's an absolute path, starts with ~ or . or .., CDPATH doesn't apply
    if token.starts_with('/')
        || token.starts_with('~')
        || token.starts_with("./")
        || token.starts_with("../")
        || token == "."
        || token == ".."
    {
        if let Some((dir, prefix)) = resolve_directory_and_prefix(token, cwd) {
            dirs_to_search.push(dir);
            return (dirs_to_search, prefix);
        }
        return (vec![], String::new());
    }

    // Determine prefix
    let (directory_relative, prefix) = match token.rfind('/') {
        Some(idx) => (&token[..idx + 1], &token[idx + 1..]),
        None => ("", token),
    };

    let cdpath = std::env::var("CDPATH").unwrap_or_default();
    let mut pwd_searched = false;

    if !cdpath.is_empty() {
        for entry in cdpath.split(':') {
            if entry.is_empty() || entry == "." {
                if !pwd_searched {
                    let dir = cwd.join(directory_relative);
                    dirs_to_search.push(dir);
                    pwd_searched = true;
                }
            } else {
                let expanded = expand_tilde(entry);
                let base_path = if expanded.starts_with('/') {
                    PathBuf::from(expanded)
                } else {
                    cwd.join(expanded)
                };
                dirs_to_search.push(base_path.join(directory_relative));
            }
        }
    }

    if !pwd_searched {
        dirs_to_search.push(cwd.join(directory_relative));
    }

    (dirs_to_search, prefix.to_string())
}

fn resolve_directory_and_prefix(token: &str, cwd: &Path) -> Option<(PathBuf, String)> {
    let mut expanded = expand_tilde(token);
    // Unescape escaped tildes for actual path resolution
    expanded = expanded.replace(r"\~", "~");

    let path = if expanded.starts_with('/') {
        PathBuf::from(&expanded)
    } else {
        cwd.join(&expanded)
    };

    if token.ends_with('/') || token == "/" || token == "~/" || token.is_empty() {
        return Some((path, String::new()));
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("/"));
    let prefix = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    Some((parent.to_path_buf(), prefix))
}

fn expand_tilde(token: &str) -> String {
    if let Some(rest) = token.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}/{}", home.display(), rest);
        }
    }
    if token == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.display().to_string();
        }
    }
    token.to_string()
}

fn list_matching_entries(directory: &Path, prefix: &str) -> Option<Vec<FsEntry>> {
    let entries: Vec<FsEntry> = std::fs::read_dir(directory)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            // Skip hidden files unless prefix starts with '.'
            if name.starts_with('.') && !prefix.starts_with('.') {
                return None;
            }

            let (match_type, fuzzy_score) = get_match_type_and_score(prefix, &name)?;

            let file_type = entry.file_type().ok();
            let is_dir = file_type.map(|t| t.is_dir()).unwrap_or(false);
            let display_path = entry.path().display().to_string();
            Some(FsEntry {
                name,
                is_dir,
                display_path,
                description: None,
                match_type,
                fuzzy_score,
            })
        })
        .collect();

    Some(entries)
}

pub(crate) fn get_match_type_and_score(prefix: &str, name: &str) -> Option<(MatchType, f32)> {
    if prefix.is_empty() {
        return Some((MatchType::Prefix, 0.0));
    }

    if name == prefix {
        return Some((MatchType::Exact, 100.0));
    }

    if name.eq_ignore_ascii_case(prefix) {
        return Some((MatchType::CaseInsensitiveExact, 90.0));
    }

    if name.starts_with(prefix) {
        return Some((MatchType::Prefix, 80.0));
    }

    if name.to_lowercase().starts_with(&prefix.to_lowercase()) {
        return Some((MatchType::Prefix, 70.0));
    }

    // Fuzzy subsequence match
    let name_lower = name.to_lowercase();
    let prefix_lower = prefix.to_lowercase();
    let name_chars: Vec<char> = name_lower.chars().collect();
    let prefix_chars: Vec<char> = prefix_lower.chars().collect();

    let mut match_positions = Vec::with_capacity(prefix_chars.len());
    let mut ci = 0usize;
    for &pc in &prefix_chars {
        let found = name_chars[ci..].iter().position(|&nc| nc == pc);
        match found {
            Some(rel) => {
                ci += rel;
                match_positions.push(ci);
                ci += 1; // Move past the matched char
            }
            None => return None, // Not a subsequence
        }
    }

    // Calculate score
    let mut score = 0.0;
    let mut consecutive = 1;
    for i in 1..match_positions.len() {
        if match_positions[i] == match_positions[i - 1] + 1 {
            consecutive += 1;
            score += consecutive as f32 * 3.0;
        } else {
            consecutive = 1;
        }
    }

    // Word boundary bonus
    for &pos in &match_positions {
        if pos == 0 {
            score += 5.0;
        } else {
            let prev = name_chars[pos - 1];
            if prev == ' ' || prev == '-' || prev == '_' || prev == '.' {
                score += 4.0;
            }
        }
    }

    // Early position bonus
    if let Some(&first) = match_positions.first() {
        score += (20.0 - first.min(20) as f32).max(0.0);
    }

    Some((MatchType::Fuzzy, score))
}

fn shell_escape(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            ' ' | '\''
                | '"'
                | '\\'
                | '$'
                | '&'
                | '|'
                | ';'
                | '<'
                | '>'
                | '('
                | ')'
                | '`'
                | '#'
                | '*'
                | '?'
                | '['
                | ']'
                | '{'
                | '}'
                | '~'
                | '!'
        ) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

pub fn apply_fs_insert(buffer: &str, request: &FsSuggestRequest, entry: &FsEntry) -> String {
    let before = &buffer[..request.replace_start];
    let after = &buffer[request.replace_end..];
    let token = &buffer[request.replace_start..request.replace_end];

    // We only want to escape the file name we are inserting
    let mut escaped_name = shell_escape(&entry.name);
    if entry.is_dir {
        escaped_name.push('/');
    }

    let path = if let Some(idx) = token.trim().rfind('/') {
        let prefix = &token.trim()[..=idx];
        format!("{prefix}{}", escaped_name)
    } else if cd_path_token(buffer).is_some() {
        if token.trim().is_empty() {
            format!(" {}", escaped_name)
        } else {
            escaped_name
        }
    } else if token.trim().is_empty() && !before.ends_with(char::is_whitespace) {
        format!(" {}", escaped_name)
    } else {
        escaped_name
    };

    format!("{before}{path}{after}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_absolute_path() {
        assert!(should_suggest_paths("cd /usr/l"));
    }

    #[test]
    fn detects_bare_cd_command() {
        assert!(should_suggest_paths("cd"));
        assert!(should_suggest_paths("cd "));
        assert!(should_suggest_paths("cd b"));
    }

    #[test]
    fn ignores_trailing_space_on_non_cd_paths() {
        assert!(!should_suggest_paths("ls /usr "));
    }

    #[test]
    fn cd_completion_lists_cwd() {
        let cwd = std::env::current_dir().unwrap();
        let request = build_fs_suggest("cd", &cwd).expect("cwd entries");
        assert_eq!(request.replace_start, 2);
        assert!(!request.entries.is_empty());
    }

    #[test]
    fn cd_insert_adds_space_after_bare_cd() {
        let cwd = std::env::current_dir().unwrap();
        let request = build_fs_suggest("cd", &cwd).expect("cwd entries");
        let entry = &request.entries[0];
        let inserted = apply_fs_insert("cd", &request, entry);
        assert!(inserted.starts_with("cd "));
        assert!(inserted.len() > 3);
    }

    #[test]
    fn cd_insert_adds_space_when_buffer_has_trailing_space() {
        let cwd = std::env::current_dir().unwrap();
        let request = build_fs_suggest("cd ", &cwd).expect("cwd entries");
        let entry = &request.entries[0];
        let inserted = apply_fs_insert("cd ", &request, entry);
        let expected_name = if entry.is_dir {
            format!("{}/", shell_escape(&entry.name))
        } else {
            shell_escape(&entry.name)
        };
        assert_eq!(inserted, format!("cd {}", expected_name));
    }

    #[test]
    fn cd_prefix_completion_filters_entries() {
        let cwd = std::env::current_dir().unwrap();
        let request = build_fs_suggest("cd b", &cwd);
        if let Some(request) = request {
            assert!(request
                .entries
                .iter()
                .all(|entry| entry.name.to_lowercase().contains('b')));
        }
    }

    #[test]
    fn detects_fuzzy_match() {
        assert!(get_match_type_and_score("xyz", "source").is_none());
        let (mt, _) = get_match_type_and_score("mn", "main.rs").unwrap();
        assert_eq!(mt, MatchType::Fuzzy);
    }

    #[test]
    fn escapes_spaces_in_filepaths() {
        assert_eq!(shell_escape("my folder"), "my\\ folder");
        assert_eq!(shell_escape("test!file"), "test\\!file");
    }
}
