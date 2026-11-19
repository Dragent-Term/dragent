//! Extract sign-in callback text from the OS clipboard (plain text, HTML, multi-line).

use warpui_core::clipboard::ClipboardContent;
use warpui_core::clipboard_utils;

/// Best-effort extraction of a callback URL or refresh token from clipboard contents.
pub fn extract_auth_paste(content: &ClipboardContent) -> Option<String> {
    let plain = content.plain_text.trim();
    if let Some(found) = find_auth_payload(plain) {
        return Some(found);
    }

    if let Some(html) = content.html.as_deref() {
        if let Some(href) = extract_href(html) {
            if let Some(found) = find_auth_payload(&href) {
                return Some(found);
            }
        }
        let stripped = clipboard_utils::strip_html_to_plain_text(html);
        if let Some(found) = find_auth_payload(stripped.trim()) {
            return Some(found);
        }
    }

    None
}

fn find_auth_payload(text: &str) -> Option<String> {
    if let Some(url) = extract_embedded_url(text) {
        return Some(url);
    }

    let trimmed = text.trim().trim_matches('"').trim_matches('\'');
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.contains("://") {
        return Some(trimmed.to_string());
    }

    for line in trimmed.lines() {
        let line = line.trim().trim_matches('"').trim_matches('\'');
        if line.is_empty() {
            continue;
        }
        if line.contains("://") {
            return Some(line.to_string());
        }
        if super::completion::looks_like_refresh_token(line) {
            return Some(line.to_string());
        }
    }

    if super::completion::looks_like_refresh_token(trimmed) {
        return Some(trimmed.to_string());
    }

    None
}

fn extract_href(html: &str) -> Option<String> {
    for needle in ["href=\"", "href='"] {
        let Some(start) = html.find(needle) else {
            continue;
        };
        let value_start = start + needle.len();
        let quote = needle.as_bytes()[needle.len() - 1] as char;
        let rest = &html[value_start..];
        let end = rest.find(quote)?;
        let href = rest[..end].trim();
        if href.contains("://") {
            return Some(href.to_string());
        }
    }
    None
}

fn extract_embedded_url(text: &str) -> Option<String> {
    for scheme in ["dragent://", "https://", "http://"] {
        let Some(start) = text.find(scheme) else {
            continue;
        };
        let rest = &text[start..];
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '<' || c == '>')
            .unwrap_or(rest.len());
        let url = rest[..end].trim();
        if !url.is_empty() {
            return Some(url.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use warpui_core::clipboard::ClipboardContent;

    #[test]
    fn finds_url_on_second_line() {
        let content = ClipboardContent::plain_text(
            "Copy this link:\ndragent://auth/callback?refresh_token=rt&state=s1".to_string(),
        );
        let found = extract_auth_paste(&content).unwrap();
        assert!(found.starts_with("dragent://"));
    }

    #[test]
    fn extracts_href_from_html_clipboard() {
        let mut content = ClipboardContent::plain_text(String::new());
        content.html = Some(
            r#"<a href="dragent://auth/callback?refresh_token=rt&state=s1">Sign in</a>"#
                .to_string(),
        );
        let found = extract_auth_paste(&content).unwrap();
        assert!(found.contains("refresh_token=rt"));
    }

    #[test]
    fn extracts_url_from_labeled_clipboard_line() {
        let content = ClipboardContent::plain_text(
            "Success! dragent://auth/callback?refresh_token=rt&state=s1".to_string(),
        );
        let found = extract_auth_paste(&content).unwrap();
        assert!(found.starts_with("dragent://"));
    }
}
