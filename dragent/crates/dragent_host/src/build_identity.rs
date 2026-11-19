//! Cheap executable identity, captured before a running service can outlive an update.
//! Protocol versions describe wire compatibility; this identifies same-protocol rebuilds.
use std::path::Path;
use std::sync::OnceLock;

pub fn executable(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(format!("{}-{}", metadata.len(), modified.as_nanos()))
}

pub fn current() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| executable(&p))
            .unwrap_or_default()
    })
}
