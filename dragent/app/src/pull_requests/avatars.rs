//! Provider profile photos, shared by every person shown in Code reviews.
use crate::theme::Theme;
use serde_json::Value;
use warpui::elements::{Align, ConstrainedBox, Container, Element, Image, ParentElement, Text};
use warpui::fonts::FamilyId;
use warpui_core::image_cache::CacheOption;
use warpui_core::scene::{CornerRadius, Radius};

pub(super) fn photo_url(person: &Value, review: &Value) -> String {
    let base = url::Url::parse(review["url"].as_str().unwrap_or("")).ok();
    for key in ["avatarUrl", "avatar_url"] {
        if let Some(raw) = person[key].as_str().filter(|s| !s.is_empty()) {
            let parsed = url::Url::parse(raw)
                .ok()
                .or_else(|| base.as_ref()?.join(raw).ok());
            if let Some(url) = parsed.filter(|u| matches!(u.scheme(), "http" | "https")) {
                return url.to_string();
            }
        }
    }
    // gh's compact author/reviewer objects omit avatarUrl. GitHub's profile
    // photo endpoint redirects to that account's current image.
    if review["provider"] != "gitlab" {
        if let (Some(mut base), Some(login)) = (base, person["login"].as_str()) {
            if base.host_str() == Some("github.com")
                && !login.is_empty()
                && login
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'[' || b == b']')
            {
                base.set_path(&format!("/{login}.png"));
                base.set_query(Some("size=64"));
                base.set_fragment(None);
                return base.to_string();
            }
        }
    }
    String::new()
}

pub(super) fn render(
    theme: Theme,
    ui: FamilyId,
    name: &str,
    url: &str,
    size: f32,
) -> Box<dyn Element> {
    let initials = name
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|s| s.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let radius = CornerRadius::with_all(Radius::Pixels(size / 2.0));
    let fallback = || {
        Container::new(
            Align::new(
                Text::new(
                    if initials.is_empty() {
                        "?".into()
                    } else {
                        initials.clone()
                    },
                    ui,
                    crate::theme::text_size(size * 0.44),
                )
                .with_color(theme.main_text())
                .finish(),
            )
            .finish(),
        )
        .with_background_color(theme.surface_3())
        .with_corner_radius(radius)
        .finish()
    };
    let image = if url.is_empty() {
        fallback()
    } else {
        Image::new(
            crate::settings::pages::account::avatar_photo_source(url),
            CacheOption::BySize,
        )
        .cover()
        .with_corner_radius(radius)
        .before_load(fallback())
        .on_load_failure(fallback())
        .finish()
    };
    ConstrainedBox::new(image)
        .with_width(size)
        .with_height(size)
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn uses_provider_image_without_replacing_it_with_an_identicon() {
        assert_eq!(
            photo_url(
                &json!({"login":"ada","avatar_url":"https://cdn.example/ada.jpg"}),
                &json!({})
            ),
            "https://cdn.example/ada.jpg"
        );
    }
    #[test]
    fn resolves_self_hosted_gitlab_image_against_its_host() {
        assert_eq!(
            photo_url(
                &json!({"avatarUrl":"/uploads/ada.png"}),
                &json!({"provider":"gitlab","url":"https://gitlab.example/team/app/-/merge_requests/1"})
            ),
            "https://gitlab.example/uploads/ada.png"
        );
    }
    #[test]
    fn compact_github_users_use_their_actual_profile_photo_endpoint() {
        assert_eq!(
            photo_url(
                &json!({"login":"octocat"}),
                &json!({"url":"https://github.com/acme/app/pull/1"})
            ),
            "https://github.com/octocat.png?size=64"
        );
    }
    #[test]
    fn missing_gitlab_photo_never_uses_a_github_account_with_the_same_name() {
        assert!(photo_url(
            &json!({"login":"ada"}),
            &json!({"provider":"gitlab","url":"https://gitlab.example/team/app"})
        )
        .is_empty());
        assert!(photo_url(&json!({"avatarUrl":"file:///etc/passwd"}), &json!({})).is_empty());
    }
}
