//! A picture for each project in the sidebar: the project's own icon or logo
//! when it has one, else its GitHub owner's avatar, else its first letter.
//!
//! Finding one reads the disk (and for an avatar, fetches it once from
//! GitHub and keeps it in Dragent's cache folder), so it is never done while
//! drawing: the sidebar asks [`icon_for`], which answers from memory and
//! notes unknown projects; a background pass ([`resolve`]) works them out.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Where projects keep an icon, most specific first. PNG, JPEG and SVG only:
/// those are what the sidebar can draw.
const CANDIDATES: &[&str] = &[
    "icon.svg",
    "icon.png",
    "logo.svg",
    "logo.png",
    ".github/logo.svg",
    ".github/logo.png",
    "favicon.svg",
    "favicon.png",
    "public/icon.svg",
    "public/icon.png",
    "public/logo.svg",
    "public/logo.png",
    "public/favicon.svg",
    "public/favicon.png",
    "static/logo.svg",
    "static/logo.png",
    "static/favicon.svg",
    "static/favicon.png",
    "assets/icon.svg",
    "assets/icon.png",
    "assets/logo.svg",
    "assets/logo.png",
    "assets/favicon.png",
];

/// Larger files aren't icons (or would be slow to draw at this size).
const MOST_BYTES: u64 = 256 * 1024;
/// An avatar is fetched again after this long.
const AVATAR_FRESH_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);

fn known() -> &'static Mutex<HashMap<PathBuf, Option<PathBuf>>> {
    static KNOWN: OnceLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> = OnceLock::new();
    KNOWN.get_or_init(Default::default)
}

fn pending() -> &'static Mutex<HashSet<PathBuf>> {
    static PENDING: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    PENDING.get_or_init(Default::default)
}

/// The picture for project `root`, if one was found. Never touches the disk:
/// a project not looked at yet is queued and shows its letter meanwhile.
pub(crate) fn icon_for(root: &Path) -> Option<PathBuf> {
    if let Some(found) = known().lock().ok()?.get(root) {
        return found.clone();
    }
    if let Ok(mut pending) = pending().lock() {
        pending.insert(root.to_path_buf());
    }
    None
}

/// The projects still to look at, taken for a background pass.
pub(crate) fn take_pending() -> Vec<PathBuf> {
    pending().lock().map(|mut pending| pending.drain().collect()).unwrap_or_default()
}

/// Keep what a background pass found. Answers whether any picture is new.
pub(crate) fn remember(found: Vec<(PathBuf, Option<PathBuf>)>) -> bool {
    let Ok(mut known) = known().lock() else {
        return false;
    };
    let mut changed = false;
    for (root, icon) in found {
        changed |= icon.is_some() && known.get(&root) != Some(&icon);
        known.insert(root, icon);
    }
    changed
}

/// Work out a project's picture. Blocking (disk, and maybe one download).
pub(crate) fn resolve(root: &Path) -> Option<PathBuf> {
    if crate::remote_fs::is_remote(root) {
        return None;
    }
    own_icon(root).or_else(|| github_avatar(root))
}

/// An icon file the project keeps itself.
fn own_icon(root: &Path) -> Option<PathBuf> {
    CANDIDATES.iter().map(|name| root.join(name)).find(|path| {
        std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0 && meta.len() <= MOST_BYTES)
            && !looks_blank(path)
    })
}

/// A picture of one flat colour (or nothing at all), like a template's
/// placeholder icon: it would draw as a plain box, so the letter is better.
/// Only raster images are checked; an SVG is taken as it is.
fn looks_blank(path: &Path) -> bool {
    let is_svg = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("svg"));
    if is_svg {
        return false;
    }
    let Ok(picture) = image::open(path) else {
        // Unreadable: it couldn't be drawn either.
        return true;
    };
    blank_pixels(&picture.thumbnail(24, 24).to_rgba8())
}

fn blank_pixels(pixels: &image::RgbaImage) -> bool {
    const SAME: u8 = 12;
    let mut seen = pixels.pixels().filter(|pixel| pixel[3] > 24);
    let Some(first) = seen.next() else {
        return true; // fully transparent
    };
    seen.all(|pixel| (0..3).all(|channel| pixel[channel].abs_diff(first[channel]) <= SAME))
}

/// The owner of a GitHub remote: `git@github.com:owner/repo.git`,
/// `https://github.com/owner/repo`.
pub(crate) fn github_owner(remote_url: &str) -> Option<String> {
    let url = remote_url.trim();
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.split_once("github.com/").map(|(_, rest)| rest))?;
    let owner = rest.split('/').next()?.trim();
    let valid = !owner.is_empty()
        && owner.len() <= 39
        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    valid.then(|| owner.to_string())
}

/// The URL of the remote named `origin` (else the first) in a git config.
fn remote_url(config: &str) -> Option<String> {
    let mut section = String::new();
    let mut first = None;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line.to_string();
            continue;
        }
        if !section.starts_with("[remote ") {
            continue;
        }
        let Some(value) = line.strip_prefix("url").map(str::trim_start).and_then(|rest| rest.strip_prefix('=')) else {
            continue;
        };
        let value = value.trim().to_string();
        if section == "[remote \"origin\"]" {
            return Some(value);
        }
        first.get_or_insert(value);
    }
    first
}

/// The project's git config: its own, or its main checkout's for a worktree.
fn git_config(root: &Path) -> Option<String> {
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        super::project_naming::worktree_main_checkout(root)?.join(".git")
    };
    std::fs::read_to_string(git_dir.join("config")).ok()
}

/// The GitHub owner's avatar, fetched once and kept in the cache folder.
fn github_avatar(root: &Path) -> Option<PathBuf> {
    let owner = github_owner(&remote_url(&git_config(root)?)?)?;
    let folder = dragent_paths::home().join("cache").join("project-icons");
    let path = folder.join(format!("github-{}.png", owner.to_ascii_lowercase()));
    let fresh = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age < AVATAR_FRESH_FOR);
    if fresh {
        return Some(path);
    }
    let response = ureq::get(&format!("https://github.com/{owner}.png?size=64"))
        .timeout(Duration::from_secs(8))
        .call();
    let bytes = match response {
        Ok(response) => {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut std::io::Read::take(response.into_reader(), MOST_BYTES), &mut bytes).ok()?;
            bytes
        }
        // Offline: an older copy still does.
        Err(_) => return path.is_file().then_some(path),
    };
    if image::guess_format(&bytes).is_err() {
        return path.is_file().then_some(path);
    }
    std::fs::create_dir_all(&folder).ok()?;
    let partial = path.with_extension("part");
    std::fs::write(&partial, &bytes).ok()?;
    std::fs::rename(&partial, &path).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_owners_come_from_either_remote_form() {
        assert_eq!(github_owner("git@github.com:Dragent-Term/dragent.git").as_deref(), Some("Dragent-Term"));
        assert_eq!(github_owner("https://github.com/rust-lang/rust").as_deref(), Some("rust-lang"));
        assert_eq!(github_owner("ssh://git@github.com/acme/app.git").as_deref(), Some("acme"));
        assert_eq!(github_owner("https://gitlab.com/acme/app.git"), None);
        assert_eq!(github_owner("https://github.com/"), None);
    }

    #[test]
    fn origin_wins_over_other_remotes() {
        let config = "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://github.com/up/app\n\
                      [remote \"origin\"]\n\turl = git@github.com:me/app.git\n";
        assert_eq!(remote_url(config).as_deref(), Some("git@github.com:me/app.git"));
        assert_eq!(remote_url("[remote \"fork\"]\n url = x\n").as_deref(), Some("x"));
        assert_eq!(remote_url("[core]\n"), None);
    }

    #[test]
    fn a_project_icon_file_is_preferred_and_big_files_are_skipped() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(own_icon(temp.path()), None);
        std::fs::create_dir_all(temp.path().join("public")).unwrap();
        let mut logo = image::RgbaImage::new(8, 8);
        for (x, _, pixel) in logo.enumerate_pixels_mut() {
            *pixel = image::Rgba([if x < 4 { 255 } else { 0 }, 40, 90, 255]);
        }
        logo.save(temp.path().join("public/logo.png")).unwrap();
        assert_eq!(own_icon(temp.path()), Some(temp.path().join("public/logo.png")));
        image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 255, 255, 255]))
            .save(temp.path().join("assets-blank.png"))
            .unwrap();
        std::fs::create_dir_all(temp.path().join("assets")).unwrap();
        std::fs::rename(temp.path().join("assets-blank.png"), temp.path().join("assets/icon.png")).unwrap();
        assert!(looks_blank(&temp.path().join("assets/icon.png")), "a plain white square is a placeholder");
        std::fs::write(temp.path().join("icon.png"), vec![1u8; (MOST_BYTES + 1) as usize]).unwrap();
        assert_eq!(own_icon(temp.path()), Some(temp.path().join("public/logo.png")), "too big to be an icon");
        std::fs::write(temp.path().join("logo.svg"), "<svg/>").unwrap();
        assert_eq!(own_icon(temp.path()), Some(temp.path().join("logo.svg")));
    }

    #[test]
    fn flat_or_empty_pictures_count_as_blank() {
        let flat = image::RgbaImage::from_pixel(4, 4, image::Rgba([250, 250, 250, 255]));
        assert!(blank_pixels(&flat));
        let empty = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 0]));
        assert!(blank_pixels(&empty));
        let mut mark = flat.clone();
        mark.put_pixel(1, 1, image::Rgba([20, 120, 200, 255]));
        assert!(!blank_pixels(&mark));
    }

    #[test]
    fn unknown_projects_are_queued_once_and_remembered() {
        let root = PathBuf::from(format!("/nowhere/{}", uuid::Uuid::new_v4()));
        assert_eq!(icon_for(&root), None);
        assert!(take_pending().contains(&root));
        assert!(remember(vec![(root.clone(), Some(PathBuf::from("/x/icon.png")))]));
        assert_eq!(icon_for(&root), Some(PathBuf::from("/x/icon.png")));
        assert!(!take_pending().contains(&root));
    }
}
