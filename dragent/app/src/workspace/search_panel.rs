//! Global project search — ripgrep-like, modern editors inspired.
//!
//! Stateless helpers + result types. View state (editors, toggles) lives in
//! `ProjectSidebarView` so the search surface stays in the single right-panel view.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub line: usize,
    pub col: usize,
    /// 1-indexed line number for display.
    pub line_text: String,
    /// Byte range of match inside `line_text`.
    pub match_range: Option<(usize, usize)>,
}

#[derive(Clone, Debug)]
pub struct SearchFileResult {
    pub relative_path: PathBuf,
    pub hits: Vec<SearchHit>,
}

#[derive(Clone, Debug)]
pub struct SearchResults {
    pub query: String,
    pub files: Vec<SearchFileResult>,
    pub total_hits: usize,
    /// Cached once by the background search worker. Aggregate rendering uses
    /// it for horizontal extent without rescanning every result on each frame.
    pub max_line_chars: usize,
    pub truncated: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ReplaceSummary {
    pub replacements: usize,
    pub files: usize,
}

impl SearchResults {
    pub fn empty(query: String) -> Self {
        Self {
            query,
            files: Vec::new(),
            total_hits: 0,
            max_line_chars: 0,
            truncated: false,
            error: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            whole_word: false,
            regex: false,
        }
    }
}

const MAX_FILES: usize = 5000;
const MAX_HITS_PER_FILE: usize = 500;
const MAX_TOTAL_HITS: usize = 100000;
// Search streams files line-by-line on a background worker, so it does not
// need the old preview-sized 2 MiB cap. Keep a high safety ceiling for truly
// extreme generated assets while allowing ordinary 50k+ line source files.
const MAX_FILE_SIZE: u64 = dragent_document::EXTREME_FILE_BYTES as u64;
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn matches_query(
    line: &str,
    query: &str,
    options: SearchOptions,
    regex: Option<&regex::Regex>,
) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    if let Some(re) = regex {
        return re.find_iter(line).map(|m| (m.start(), m.end())).collect();
    }
    // plain text search
    let mut hits = Vec::new();
    if options.case_sensitive {
        let mut start = 0;
        while let Some(idx) = line[start..].find(query) {
            let s = start + idx;
            let e = s + query.len();
            if options.whole_word {
                let before = s
                    .checked_sub(1)
                    .and_then(|i| line[i..s].chars().last())
                    .map(is_word_char)
                    .unwrap_or(false);
                let after = line[e..].chars().next().map(is_word_char).unwrap_or(false);
                if before || after {
                    start = s + 1;
                    if start >= line.len() {
                        break;
                    }
                    continue;
                }
            }
            hits.push((s, e));
            start = e;
            if start >= line.len() {
                break;
            }
        }
    } else {
        let lower_line = line.to_lowercase();
        let lower_query = query.to_lowercase();
        let mut start = 0;
        while let Some(idx) = lower_line[start..].find(&lower_query) {
            let s = start + idx;
            let e = s + lower_query.len();
            if options.whole_word {
                let before = s
                    .checked_sub(lower_query.len())
                    .and_then(|_| line[s..].chars().next()); // not needed, use chars before
                                                             // simpler: check original line chars around original byte range mapped via char count approximation
                                                             // For case-insensitive, whole_word check on original line using same char positions (query length equals match length because lowercasing preserves length for ASCII; for unicode lengths may differ but acceptable fallback)
                let before_ok = if s == 0 {
                    false
                } else {
                    line[..s].chars().last().map(is_word_char).unwrap_or(false)
                };
                let after_ok = line[e..].chars().next().map(is_word_char).unwrap_or(false);
                if before_ok || after_ok {
                    start = s + 1;
                    if start >= lower_line.len() {
                        break;
                    }
                    continue;
                }
                let _ = before; // avoid warning
            }
            hits.push((s, e));
            start = e;
            if start >= lower_line.len() {
                break;
            }
        }
    }
    hits
}

fn contains_glob_magic(pattern: &str) -> bool {
    pattern
        .chars()
        .any(|character| matches!(character, '*' | '?' | '[' | ']' | '{' | '}'))
}

fn normalized_project_path(root: &Path, pattern: &str) -> Option<(PathBuf, bool)> {
    if pattern.is_empty() || contains_glob_magic(pattern) {
        return None;
    }
    let normalized = pattern.trim().trim_end_matches(['/', '\\']);
    if normalized.is_empty() {
        return None;
    }
    let candidate = Path::new(normalized);
    let relative_candidate = if candidate.is_absolute() {
        candidate.strip_prefix(root).ok()?
    } else {
        candidate
    };
    let mut relative = PathBuf::new();
    for component in relative_candidate.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(component) => relative.push(component),
            // Search scopes are project-relative. Never allow a scope to walk
            // above the active project, even if that outside path exists.
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => return None,
        }
    }
    let absolute = root.join(&relative);
    let metadata = absolute.metadata().ok()?;
    Some((relative, metadata.is_dir()))
}

fn add_glob(builder: &mut globset::GlobSetBuilder, pattern: &str) {
    if let Ok(glob) = globset::GlobBuilder::new(pattern)
        .literal_separator(false)
        .case_insensitive(false)
        .build()
    {
        builder.add(glob);
    }
}

fn build_include_matcher(root: &Path, pattern: &str) -> Option<globset::GlobSet> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut builder = globset::GlobSetBuilder::new();
    for part in split_patterns(trimmed) {
        let pat = part.trim();
        if pat.is_empty() {
            continue;
        }
        if let Some((relative, is_directory)) = normalized_project_path(root, pat) {
            let relative = relative.to_string_lossy().replace('\\', "/");
            if relative.is_empty() {
                add_glob(&mut builder, "**");
            } else if is_directory {
                // A search path is a directory query, not an exact-name glob.
                // Matching descendants here also keeps mixed path/glob entries
                // as a straightforward union.
                add_glob(&mut builder, &format!("{relative}/**"));
            } else {
                add_glob(&mut builder, &relative);
            }
        } else {
            // Preserve the existing VS Code-like convenience where a bare
            // filename pattern can match at any depth.
            let glob_pattern = if !pat.contains('/') && !contains_glob_magic(pat) {
                format!("**/{pat}")
            } else {
                pat.to_string()
            };
            add_glob(&mut builder, &glob_pattern);
        }
    }
    builder.build().ok()
}

fn build_exclude_matcher(root: &Path, pattern: &str) -> Option<globset::GlobSet> {
    build_include_matcher(root, pattern)
}

/// Resolve an include list made entirely of existing files/directories into
/// concrete walk roots. When a glob is present we retain the project root as
/// the traversal root and apply the union matcher to candidates.
fn include_walk_roots(root: &Path, pattern: &str) -> Option<Vec<PathBuf>> {
    let mut roots = Vec::new();
    for part in split_patterns(pattern.trim()) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (relative, _) = normalized_project_path(root, part)?;
        let absolute = root.join(relative);
        if !roots.iter().any(|existing: &PathBuf| {
            absolute == *existing || (existing.is_dir() && absolute.starts_with(existing))
        }) {
            roots.retain(|existing| !(absolute.is_dir() && existing.starts_with(&absolute)));
            roots.push(absolute);
        }
    }
    (!roots.is_empty()).then_some(roots)
}

fn build_search_regex(query: &str, options: SearchOptions) -> Result<regex::Regex, String> {
    let query_pattern = if options.regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    let pattern = if options.whole_word {
        format!(r"\b(?:{})\b", query_pattern)
    } else {
        query_pattern
    };
    let mut builder = regex::RegexBuilder::new(&pattern);
    builder.case_insensitive(!options.case_sensitive);
    builder
        .build()
        .map_err(|error| format!("Invalid regex: {error}"))
}

fn split_patterns(text: &str) -> Vec<&str> {
    // comma-separated like VS Code, handle brace depth
    let mut patterns = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    let mut escaped = false;
    for (index, character) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' | '\n' | '\r' if depth == 0 => {
                patterns.push(&text[start..index]);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    patterns.push(&text[start..]);
    patterns
}

fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | "target" | ".next" | "dist" | "build" | ".turbo" | "out"
    )
}

/// Search a remote project on its machine with `git grep` (through the
/// routed git command), shaped like the local results.
#[cfg(not(target_family = "wasm"))]
fn run_remote_search(
    root: &Path,
    query: &str,
    include_pattern: &str,
    exclude_pattern: &str,
    options: SearchOptions,
) -> SearchResults {
    let failure = |error: String| SearchResults {
        error: Some(error),
        ..SearchResults::empty(query.to_string())
    };
    let regex = match build_search_regex(query, options) {
        Ok(regex) => regex,
        Err(error) => return failure(error),
    };
    let mut args: Vec<String> = vec![
        "grep".into(),
        "-n".into(),
        "-I".into(),
        "--untracked".into(),
        "--no-color".into(),
    ];
    if !options.case_sensitive {
        args.push("-i".into());
    }
    if options.whole_word {
        args.push("-w".into());
    }
    args.push(if options.regex { "-E".into() } else { "-F".into() });
    args.push("-e".into());
    args.push(query.to_string());
    args.push("--".into());
    // `split_patterns` keeps blank entries; git rejects an empty pathspec.
    let patterns = |text| {
        split_patterns(text)
            .into_iter()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
    };
    let includes = patterns(include_pattern);
    if includes.is_empty() {
        args.push(".".into());
    }
    args.extend(includes.iter().map(|p| p.to_string()));
    args.extend(patterns(exclude_pattern).iter().map(|p| format!(":(exclude){p}")));
    let output = match crate::git_cmd::GitCommand::new()
        .args(&args)
        .current_dir(root)
        .output()
    {
        Ok(output) => output,
        Err(error) => return failure(format!("Search failed: {error}")),
    };
    // Exit code 1 means "no matches".
    if !output.status.success() && output.status.code() != Some(1) {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return failure(if detail.is_empty() { "Search failed".into() } else { detail });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut results = SearchResults::empty(query.to_string());
    for line in text.lines() {
        let mut parts = line.splitn(3, ':');
        let (Some(path), Some(number), Some(content)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let Ok(number) = number.parse::<usize>() else {
            continue;
        };
        let relative = PathBuf::from(path);
        let file = match results.files.iter_mut().position(|f| f.relative_path == relative) {
            Some(index) => &mut results.files[index],
            None => {
                if results.files.len() >= MAX_FILES {
                    results.truncated = true;
                    break;
                }
                results.files.push(SearchFileResult {
                    relative_path: relative,
                    hits: Vec::new(),
                });
                results.files.last_mut().expect("just pushed")
            }
        };
        if file.hits.len() >= MAX_HITS_PER_FILE || results.total_hits >= MAX_TOTAL_HITS {
            results.truncated = true;
            continue;
        }
        let range = regex.find(content).map(|m| (m.start(), m.end()));
        file.hits.push(SearchHit {
            // 1-based, like local hits and `git grep -n`.
            line: number,
            col: range.map(|(start, _)| content[..start].encode_utf16().count()).unwrap_or(0),
            line_text: content.to_string(),
            match_range: range,
        });
        results.total_hits += 1;
        results.max_line_chars = results.max_line_chars.max(content.chars().count());
    }
    results
}

#[cfg(target_family = "wasm")]
pub fn run_search(
    _root: &Path,
    query: &str,
    _include_pattern: &str,
    _exclude_pattern: &str,
    _options: SearchOptions,
) -> SearchResults {
    SearchResults::empty(query.to_string())
}

#[cfg(not(target_family = "wasm"))]
pub fn run_search(
    root: &Path,
    query: &str,
    include_pattern: &str,
    exclude_pattern: &str,
    options: SearchOptions,
) -> SearchResults {
    if query.trim().is_empty() {
        return SearchResults::empty(query.to_string());
    }
    if crate::remote_fs::is_remote(root) {
        return run_remote_search(root, query, include_pattern, exclude_pattern, options);
    }
    // Short queries (1 char) are extremely expensive and usually not intentional;
    // still allow but require explicit include filter or will be truncated early.
    let include_set = build_include_matcher(root, include_pattern);
    let exclude_set = build_exclude_matcher(root, exclude_pattern);

    let regex_compiled = match build_search_regex(query, options) {
        Ok(regex) => Some(regex),
        Err(error) => {
            return SearchResults {
                query: query.to_string(),
                files: Vec::new(),
                total_hits: 0,
                max_line_chars: 0,
                truncated: false,
                error: Some(error),
            };
        }
    };

    let mut files: Vec<SearchFileResult> = Vec::new();
    let mut total_hits = 0usize;
    let mut max_line_chars = 0usize;
    let mut truncated = false;
    // Use `ignore` crate: respects .gitignore, .ignore, skips hidden via config.
    // Fallback to walkdir if ignore feature not available is already handled by walkdir path above,
    // but we prefer ignore for large repos (it skips target/ via gitignore).
    let search_roots = include_walk_roots(root, include_pattern);
    let first_search_root = search_roots
        .as_ref()
        .and_then(|roots| roots.first())
        .map(PathBuf::as_path)
        .unwrap_or(root);
    let mut builder = ignore::WalkBuilder::new(first_search_root);
    if let Some(roots) = search_roots.as_ref() {
        for search_root in roots.iter().skip(1) {
            builder.add(search_root);
        }
    }
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .follow_links(false)
        .require_git(false);

    let walker = builder.build();

    let mut visited_files: usize = 0;
    const MAX_VISITED_FILES: usize = 100000;

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            // Skip .git directory explicitly (extra safety)
            if let Some(name) = entry.file_name().to_str() {
                if name == ".git" {
                    continue;
                }
            }
            // Extra guard: skip heavy dirs even if ignore didn't
            if let Some(name) = entry.file_name().to_str() {
                if should_skip_dir(name) {
                    // ignore crate already handles, but keep as safety
                    continue;
                }
            }
            continue;
        }
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        visited_files += 1;
        if visited_files > MAX_VISITED_FILES
            || files.len() >= MAX_FILES
            || total_hits >= MAX_TOTAL_HITS
        {
            truncated = true;
            break;
        }
        let path = entry.path();
        // Quick extension filter before opening metadata
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            if matches!(
                ext,
                "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "webp"
                    | "ico"
                    | "woff"
                    | "woff2"
                    | "ttf"
                    | "otf"
                    | "mp4"
                    | "mov"
                    | "zip"
                    | "tar"
                    | "gz"
                    | "wasm"
                    | "pdf"
                    | "lock"
                    | "svg"
            ) {
                continue;
            }
        }
        // quick size gate via metadata without opening
        if let Ok(meta) = entry.metadata() {
            if meta.len() > MAX_FILE_SIZE {
                continue;
            }
        } else {
            continue;
        }
        let rel = match path.strip_prefix(root) {
            Ok(r) => r.to_path_buf(),
            Err(_) => continue,
        };
        let rel_str = rel.to_string_lossy().to_string();
        if let Some(ref inc) = include_set {
            if !inc.is_match(&rel) && !inc.is_match(rel_str.as_str()) && !inc.is_match(path) {
                continue;
            }
        }
        if let Some(ref exc) = exclude_set {
            if exc.is_match(&rel) || exc.is_match(rel_str.as_str()) || exc.is_match(path) {
                continue;
            }
        }
        // Read file lazily line-by-line to avoid loading huge files fully;
        // also detect binary (contains NUL) early.
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = std::io::BufReader::new(file);
        use std::io::BufRead;
        let mut hits: Vec<SearchHit> = Vec::new();
        let mut is_binary = false;
        let mut line_idx = 0usize;
        for line_res in reader.lines() {
            let line = match line_res {
                Ok(l) => l,
                Err(_) => {
                    is_binary = true;
                    break;
                }
            };
            if line.contains('\0') {
                is_binary = true;
                break;
            }
            let ranges = matches_query(&line, query, options, regex_compiled.as_ref());
            for (s, e) in ranges {
                let column = line[..s].encode_utf16().count();
                max_line_chars = max_line_chars.max(line.chars().count());
                hits.push(SearchHit {
                    line: line_idx + 1,
                    col: column + 1,
                    line_text: line.clone(),
                    match_range: Some((s, e)),
                });
                if hits.len() >= MAX_HITS_PER_FILE {
                    truncated = true;
                    break;
                }
            }
            line_idx += 1;
            if hits.len() >= MAX_HITS_PER_FILE {
                break;
            }
        }
        if is_binary || hits.is_empty() {
            continue;
        }
        total_hits += hits.len();
        files.push(SearchFileResult {
            relative_path: rel,
            hits,
        });
        if total_hits >= MAX_TOTAL_HITS {
            truncated = true;
            break;
        }
    }

    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    SearchResults {
        query: query.to_string(),
        files,
        total_hits,
        max_line_chars,
        truncated,
        error: None,
    }
}

#[cfg(not(target_family = "wasm"))]
pub fn replace_search_results(
    root: &Path,
    query: &str,
    replacement: &str,
    include_pattern: &str,
    exclude_pattern: &str,
    options: SearchOptions,
    only: Option<(&Path, &SearchHit)>,
) -> Result<ReplaceSummary, String> {
    if query.is_empty() {
        return Ok(ReplaceSummary::default());
    }
    let regex = build_search_regex(query, options)?;
    if let Some((relative_path, hit)) = only {
        let path = root.join(relative_path);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("Could not read {}: {error}", relative_path.display()))?;
        let line_start = if hit.line <= 1 {
            0
        } else {
            text.split_inclusive('\n')
                .take(hit.line - 1)
                .map(str::len)
                .sum()
        };
        let raw_line = text[line_start..]
            .split_once('\n')
            .map_or(&text[line_start..], |(line, _)| line);
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let (start, end) = hit
            .match_range
            .filter(|(start, end)| {
                start < end
                    && *end <= line.len()
                    && line.is_char_boundary(*start)
                    && line.is_char_boundary(*end)
            })
            .ok_or_else(|| "The selected result is no longer valid. Search again.".to_string())?;
        let candidate = &line[start..end];
        if !regex.is_match(candidate) {
            return Err("The selected result changed on disk. Search again.".to_string());
        }
        let replaced = if options.regex {
            regex.replace(candidate, replacement).into_owned()
        } else {
            replacement.to_string()
        };
        let absolute = line_start + start..line_start + end;
        let mut updated = text;
        updated.replace_range(absolute, &replaced);
        std::fs::write(&path, updated)
            .map_err(|error| format!("Could not update {}: {error}", relative_path.display()))?;
        return Ok(ReplaceSummary {
            replacements: 1,
            files: 1,
        });
    }

    let fresh = run_search(root, query, include_pattern, exclude_pattern, options);
    if let Some(error) = fresh.error {
        return Err(error);
    }
    if fresh.truncated {
        return Err(
            "Replace all was stopped because the search result set is truncated. Narrow the search first."
                .to_string(),
        );
    }
    let mut summary = ReplaceSummary::default();
    for file in fresh.files {
        let path = root.join(&file.relative_path);
        let mut text = std::fs::read_to_string(&path)
            .map_err(|error| format!("Could not read {}: {error}", file.relative_path.display()))?;
        let mut edits = Vec::with_capacity(file.hits.len());
        for hit in &file.hits {
            let line_start = if hit.line <= 1 {
                0
            } else {
                text.split_inclusive('\n')
                    .take(hit.line - 1)
                    .map(str::len)
                    .sum()
            };
            let Some((start, end)) = hit.match_range else {
                continue;
            };
            let absolute = line_start + start..line_start + end;
            if absolute.end > text.len()
                || !text.is_char_boundary(absolute.start)
                || !text.is_char_boundary(absolute.end)
            {
                return Err(format!(
                    "{} changed while replacements were being prepared. Search again.",
                    file.relative_path.display()
                ));
            }
            let candidate = &text[absolute.clone()];
            let replaced = if options.regex {
                regex.replace(candidate, replacement).into_owned()
            } else {
                replacement.to_string()
            };
            edits.push((absolute, replaced));
        }
        if edits.is_empty() {
            continue;
        }
        let count = edits.len();
        for (range, replacement) in edits.into_iter().rev() {
            text.replace_range(range, &replacement);
        }
        std::fs::write(&path, text).map_err(|error| {
            format!("Could not update {}: {error}", file.relative_path.display())
        })?;
        summary.replacements += count;
        summary.files += 1;
    }
    Ok(summary)
}

#[cfg(target_family = "wasm")]
pub fn replace_search_results(
    _root: &Path,
    _query: &str,
    _replacement: &str,
    _include_pattern: &str,
    _exclude_pattern: &str,
    _options: SearchOptions,
    _only: Option<(&Path, &SearchHit)>,
) -> Result<ReplaceSummary, String> {
    Err("Project replacement is unavailable in this environment".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_word_filter() {
        let opts = SearchOptions {
            case_sensitive: true,
            whole_word: true,
            regex: false,
        };
        assert_eq!(
            matches_query("foobar foo fooBar", "foo", opts, None).len(),
            1
        );
    }

    #[test]
    fn case_insensitive() {
        let opts = SearchOptions {
            case_sensitive: false,
            whole_word: false,
            regex: false,
        };
        assert_eq!(
            matches_query("Hello HELLO hello", "hello", opts, None).len(),
            3
        );
    }

    #[test]
    fn unicode_case_insensitive_ranges_slice_the_original_line() {
        let mut builder = regex::RegexBuilder::new(&regex::escape("école"));
        builder.case_insensitive(true);
        let regex = builder.build().unwrap();
        let line = "École and école";
        let ranges = matches_query(line, "école", SearchOptions::default(), Some(&regex));

        assert_eq!(ranges.len(), 2);
        assert_eq!(&line[ranges[0].0..ranges[0].1], "École");
        assert_eq!(&line[ranges[1].0..ranges[1].1], "école");
    }

    #[test]
    fn include_glob() {
        let m = build_include_matcher(Path::new("."), "src/**/*.rs").unwrap();
        assert!(m.is_match(Path::new("src/app/mod.rs")));
        assert!(!m.is_match(Path::new("marketing/page.tsx")));
    }

    #[test]
    fn split_patterns_keeps_unicode_byte_boundaries() {
        assert_eq!(
            split_patterns("src/é/**,tests/**"),
            vec!["src/é/**", "tests/**"]
        );
        assert_eq!(
            split_patterns("{src,test}/**,docs/**"),
            vec!["{src,test}/**", "docs/**"]
        );
        assert_eq!(
            split_patterns("src/services\nsrc/components"),
            vec!["src/services", "src/components"]
        );
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn folder_and_file_scopes_limit_search_before_matching() {
        let root = std::env::temp_dir().join(format!(
            "dragent-search-folder-scope-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for directory in [
            "src/services/nested",
            "src/components",
            "src/admin/services",
            "src/user/services",
            "tests",
        ] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        for file in [
            "src/services/auth.rs",
            "src/services/nested/user.rs",
            "src/components/button.rs",
            "src/admin/services/admin.rs",
            "src/user/services/user.rs",
            "tests/auth_test.rs",
        ] {
            std::fs::write(root.join(file), "scope needle\n").unwrap();
        }

        let folder = run_search(
            &root,
            "needle",
            "./src/services/",
            "",
            SearchOptions::default(),
        );
        assert_eq!(
            folder
                .files
                .iter()
                .map(|file| file.relative_path.as_path())
                .collect::<Vec<_>>(),
            vec![
                Path::new("src/services/auth.rs"),
                Path::new("src/services/nested/user.rs"),
            ]
        );

        let multiple = run_search(
            &root,
            "needle",
            "src/admin/services, tests/auth_test.rs",
            "",
            SearchOptions::default(),
        );
        assert_eq!(
            multiple
                .files
                .iter()
                .map(|file| file.relative_path.as_path())
                .collect::<Vec<_>>(),
            vec![
                Path::new("src/admin/services/admin.rs"),
                Path::new("tests/auth_test.rs"),
            ]
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn finds_a_match_far_down_in_a_large_file() {
        let root =
            std::env::temp_dir().join(format!("dragent-search-large-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        let target_line = 48_321usize;
        let mut source = String::with_capacity(900_000);
        for line in 1..=50_000 {
            if line == target_line {
                source.push_str("let token = refresh_token().await?;\n");
            } else {
                source.push_str("let value = 1;\n");
            }
        }
        std::fs::write(root.join("src/large.rs"), source).unwrap();

        let results = run_search(&root, "refresh_token", "", "", SearchOptions::default());

        assert_eq!(results.total_hits, 1);
        assert_eq!(results.files[0].hits[0].line, target_line);
        assert_eq!(results.max_line_chars, 35);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn replaces_one_selected_match_without_touching_others() {
        let root =
            std::env::temp_dir().join(format!("dragent-search-replace-one-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "foo\nfoo\n").unwrap();
        let hit = SearchHit {
            line: 2,
            col: 1,
            line_text: "foo".to_string(),
            match_range: Some((0, 3)),
        };
        let summary = replace_search_results(
            &root,
            "foo",
            "bar",
            "",
            "",
            SearchOptions::default(),
            Some((Path::new("src/main.rs"), &hit)),
        )
        .unwrap();
        assert_eq!(summary.replacements, 1);
        assert_eq!(
            std::fs::read_to_string(root.join("src/main.rs")).unwrap(),
            "foo\nbar\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn replace_all_obeys_include_and_exclude_filters() {
        let root =
            std::env::temp_dir().join(format!("dragent-search-replace-all-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("tests")).unwrap();
        std::fs::write(root.join("src/main.rs"), "foo foo\n").unwrap();
        std::fs::write(root.join("tests/main.rs"), "foo\n").unwrap();
        let summary = replace_search_results(
            &root,
            "foo",
            "bar",
            "**/*.rs",
            "tests/**",
            SearchOptions::default(),
            None,
        )
        .unwrap();
        assert_eq!(summary.replacements, 2);
        assert_eq!(summary.files, 1);
        assert_eq!(
            std::fs::read_to_string(root.join("src/main.rs")).unwrap(),
            "bar bar\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("tests/main.rs")).unwrap(),
            "foo\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
