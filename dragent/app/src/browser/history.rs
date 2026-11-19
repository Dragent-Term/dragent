//! Pages visited in browser panes, newest first, one list per browser
//! profile (kept beside that profile's cookies and storage, so history never
//! crosses organisations and is cleared with the rest of its data).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Visits kept per profile; the oldest fall off.
const MAX_VISITS: usize = 300;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Visit {
    pub url: String,
    pub title: String,
    /// Unix seconds of the latest visit.
    pub at: u64,
}

thread_local! {
    static LOADED: RefCell<HashMap<PathBuf, Vec<Visit>>> = RefCell::new(HashMap::new());
}

fn file(profile: &Path) -> PathBuf {
    profile.join("history.json")
}

fn with_visits<T>(profile: &Path, run: impl FnOnce(&mut Vec<Visit>) -> T) -> T {
    LOADED.with(|loaded| {
        let mut loaded = loaded.borrow_mut();
        let visits = loaded.entry(profile.to_path_buf()).or_insert_with(|| {
            std::fs::read(file(profile))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default()
        });
        run(visits)
    })
}

fn save(profile: &Path, visits: &[Visit]) {
    let Ok(bytes) = serde_json::to_vec(visits) else {
        return;
    };
    let path = file(profile);
    let tmp = path.with_extension("tmp");
    if std::fs::create_dir_all(profile).is_ok() && std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Note a visit to `url`; visiting a page again moves it to the top.
pub fn record(profile: &Path, url: &str, title: &str) {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let changed = with_visits(profile, |visits| add(visits, url, title, now()));
    if let Some(visits) = changed {
        save(profile, &visits);
    }
}

/// Give the newest visit to `url` its page title once the page reports it.
pub fn retitle(profile: &Path, url: &str, title: &str) {
    let changed = with_visits(profile, |visits| {
        let visit = visits.first_mut().filter(|visit| visit.url == url)?;
        if title.is_empty() || visit.title == title {
            return None;
        }
        visit.title = title.to_string();
        Some(visits.clone())
    });
    if let Some(visits) = changed {
        save(profile, &visits);
    }
}

/// The newest `limit` visits.
pub fn recent(profile: &Path, limit: usize) -> Vec<Visit> {
    with_visits(profile, |visits| visits.iter().take(limit).cloned().collect())
}

/// Visited pages matching what is typed in the address bar, best first:
/// sites that start with it, then addresses, then title words, then any
/// match; newer first within each.
pub fn suggest(profile: &Path, typed: &str, limit: usize) -> Vec<Visit> {
    with_visits(profile, |visits| rank(visits, typed, limit))
}

fn rank(visits: &[Visit], typed: &str, limit: usize) -> Vec<Visit> {
    let typed = typed.trim().to_lowercase();
    let typed = typed
        .strip_prefix("https://")
        .or_else(|| typed.strip_prefix("http://"))
        .unwrap_or(&typed);
    let typed = typed.strip_prefix("www.").unwrap_or(typed);
    if typed.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(u8, usize, &Visit)> = visits
        .iter()
        .enumerate()
        .filter_map(|(age, visit)| {
            let address = visit.url.to_lowercase();
            let address = address.split_once("://").map_or(address.as_str(), |(_, rest)| rest);
            let address = address.strip_prefix("www.").unwrap_or(address);
            let title = visit.title.to_lowercase();
            let host = address.split('/').next().unwrap_or_default();
            let score = if address.starts_with(typed) {
                0
            } else if host.contains(&format!(".{typed}")) {
                1
            } else if address.contains(&format!("/{typed}")) {
                2
            } else if title.split(|c: char| !c.is_alphanumeric()).any(|word| word.starts_with(typed)) {
                3
            } else if address.contains(typed) || title.contains(typed) {
                4
            } else {
                return None;
            };
            Some((score, age, visit))
        })
        .collect();
    scored.sort_by_key(|(score, age, _)| (*score, *age));
    scored.into_iter().take(limit).map(|(_, _, visit)| visit.clone()).collect()
}

pub fn clear(profile: &Path) {
    with_visits(profile, Vec::clear);
    let _ = std::fs::remove_file(file(profile));
}

/// The list after a visit, or `None` when nothing changed.
fn add(visits: &mut Vec<Visit>, url: &str, title: &str, at: u64) -> Option<Vec<Visit>> {
    if visits.first().is_some_and(|visit| visit.url == url) {
        return None;
    }
    let previous = visits.iter().position(|visit| visit.url == url);
    let title = match previous {
        Some(index) if title.is_empty() => visits.remove(index).title,
        Some(index) => {
            visits.remove(index);
            title.to_string()
        }
        None => title.to_string(),
    };
    visits.insert(0, Visit { url: url.to_string(), title, at });
    visits.truncate(MAX_VISITS);
    Some(visits.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_revisit_moves_to_the_top_without_duplicating() {
        let mut visits = Vec::new();
        add(&mut visits, "https://a.test/", "A", 1);
        add(&mut visits, "https://b.test/", "B", 2);
        assert!(add(&mut visits, "https://b.test/", "B", 3).is_none());
        add(&mut visits, "https://a.test/", "", 4);
        let urls: Vec<_> = visits.iter().map(|visit| visit.url.as_str()).collect();
        assert_eq!(urls, ["https://a.test/", "https://b.test/"]);
        // A revisit before the title arrives keeps the title it had.
        assert_eq!(visits[0].title, "A");
        assert_eq!(visits[0].at, 4);
    }

    #[test]
    fn suggestions_put_sites_that_start_with_the_text_first() {
        let visit = |url: &str, title: &str| Visit { url: url.into(), title: title.into(), at: 0 };
        let visits = [
            visit("https://docs.rs/github", "Docs about GitHub"),
            visit("https://www.github.com/x", "Repo"),
            visit("https://example.com/", "My hub page"),
            visit("https://gist.github.com/", "Gists"),
        ];
        let urls = |typed: &str| -> Vec<String> {
            rank(&visits, typed, 6).into_iter().map(|visit| visit.url).collect()
        };
        assert_eq!(
            urls("git"),
            ["https://www.github.com/x", "https://gist.github.com/", "https://docs.rs/github"]
        );
        // A typed scheme and www. are ignored, so the site still leads.
        assert_eq!(urls("https://www.github")[0], "https://www.github.com/x");
        assert_eq!(urls("hub"), ["https://example.com/", "https://docs.rs/github", "https://www.github.com/x", "https://gist.github.com/"]);
        assert!(urls("  ").is_empty());
    }

    #[test]
    fn keeps_a_bounded_list() {
        let mut visits = Vec::new();
        for n in 0..MAX_VISITS + 20 {
            add(&mut visits, &format!("https://site.test/{n}"), "", n as u64);
        }
        assert_eq!(visits.len(), MAX_VISITS);
        assert_eq!(visits[0].url, format!("https://site.test/{}", MAX_VISITS + 19));
    }

    #[test]
    fn records_and_clears_a_profile_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile");
        record(&profile, "https://example.com/", "");
        retitle(&profile, "https://example.com/", "Example");
        record(&profile, "about:blank", "ignored");
        assert_eq!(recent(&profile, 10).len(), 1);
        LOADED.with(|loaded| loaded.borrow_mut().clear());
        let reloaded = recent(&profile, 10);
        assert_eq!(reloaded[0].title, "Example");
        clear(&profile);
        assert!(recent(&profile, 10).is_empty());
        assert!(!profile.join("history.json").exists());
    }
}
