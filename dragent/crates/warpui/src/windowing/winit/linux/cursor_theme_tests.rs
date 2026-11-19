use std::fs;
use std::path::PathBuf;

use super::CursorThemeCrawler;

fn with_temp_icons<F: FnOnce(PathBuf)>(setup: impl FnOnce(&PathBuf), test: F) {
    let base =
        std::env::temp_dir().join(format!("dragent-cursor-theme-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    setup(&base);
    test(base.clone());
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_no_themes_found() {
    with_temp_icons(
        |dirs| {
            fs::create_dir_all(dirs.join("icons")).unwrap();
        },
        |dirs| {
            let crawler = CursorThemeCrawler {
                directories: vec![dirs.join("icons")],
            };
            assert_eq!(crawler.determine_cursor_theme(), None);
        },
    );
}

#[test]
fn test_default_theme_found() {
    with_temp_icons(
        |dirs| {
            fs::create_dir_all(dirs.join("icons/default/cursors")).unwrap();
        },
        |dirs| {
            let crawler = CursorThemeCrawler {
                directories: vec![dirs.join("icons")],
            };
            assert_eq!(
                crawler.determine_cursor_theme(),
                Some("default".to_string())
            );
        },
    );
}

#[test]
fn test_known_theme_found() {
    with_temp_icons(
        |dirs| {
            fs::create_dir_all(dirs.join("icons/Yaru/cursors")).unwrap();
        },
        |dirs| {
            let crawler = CursorThemeCrawler {
                directories: vec![dirs.join("icons")],
            };
            assert_eq!(crawler.determine_cursor_theme(), Some("Yaru".to_string()));
        },
    );
}
