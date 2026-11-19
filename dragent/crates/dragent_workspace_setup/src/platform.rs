//! The current platform as config `platforms` values use it.

/// `macos`, `linux` or `windows`.
pub fn current() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

/// Whether `platforms` (omitted means all) includes this machine.
pub fn applies(platforms: Option<&[String]>) -> bool {
    match platforms {
        None => true,
        Some(list) => list.iter().any(|platform| platform == current()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_means_everywhere_and_current_matches() {
        assert!(applies(None));
        assert!(applies(Some(&[current().to_string()])));
        let other = if current() == "windows" {
            "macos"
        } else {
            "windows"
        };
        assert!(!applies(Some(&[other.to_string()])));
    }
}
