//! Clipboard paste stays on the desktop worker; the handle remains alive on Linux.
use std::time::Duration;

pub(crate) fn paste(
    slot: &mut Option<arboard::Clipboard>,
    text: &str,
    deliver: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if slot.is_none() {
        *slot = Some(
            arboard::Clipboard::new()
                .map_err(|e| format!("unsupported_capability: Clipboard unavailable: {e}"))?,
        );
    }
    let clipboard = slot.as_mut().unwrap();
    let old_text = clipboard.get_text().ok();
    let old_html = clipboard.get().html().ok();
    clipboard
        .set_text(text)
        .map_err(|e| format!("Clipboard write failed: {e}"))?;
    let result = deliver();
    std::thread::sleep(Duration::from_millis(300));
    // Preserve an unrelated copy the person made while the app handled paste.
    if clipboard.get_text().ok().as_deref() == Some(text) {
        if let Some(old) = old_text {
            if let Some(html) = old_html {
                let _ = clipboard.set_html(html, Some(old));
            } else {
                let _ = clipboard.set_text(old);
            }
        }
    }
    result
}
