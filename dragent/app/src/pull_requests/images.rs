use super::*;
use base64::Engine;
use warpui_core::assets::asset_cache::{AssetSource, AsyncAssetId, AsyncAssetType};
struct ReviewImage;
impl AsyncAssetType for ReviewImage {}
pub(super) fn supported(path: &str) -> bool {
    ["png", "jpg", "jpeg", "webp"].contains(
        &path
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
    )
}
pub(super) fn source(target: Target, sha: &str) -> AssetSource {
    let sha = sha.to_string();
    let key = format!(
        "{}:{}:{sha}",
        dragent_paths::organisation_id(),
        target.root.display()
    );
    AssetSource::Async {
        id: AsyncAssetId::new::<ReviewImage>(key),
        fetch: std::sync::Arc::new(move || {
            let target = target.clone();
            let sha = sha.clone();
            Box::pin(async move {
                let (tx, rx) = futures::channel::oneshot::channel();
                crate::bg::detach(move || {
                    let _ = tx.send(fetch(&target, &sha));
                });
                rx.await
                    .map_err(|_| anyhow::anyhow!("Image preview worker stopped"))?
            })
        }),
    }
}
fn fetch(target: &Target, sha: &str) -> anyhow::Result<bytes::Bytes> {
    let value = json(target, Operation::Blob { sha: sha.into() }).map_err(anyhow::Error::msg)?;
    if text(&value, "encoding") != "base64" {
        anyhow::bail!("Unsupported repository blob encoding")
    }
    let raw = base64::engine::general_purpose::STANDARD.decode(
        text(&value, "content")
            .bytes()
            .filter(|c| !c.is_ascii_whitespace())
            .collect::<Vec<_>>(),
    )?;
    validate(&raw)?;
    Ok(bytes::Bytes::from(raw))
}
fn validate(raw: &[u8]) -> anyhow::Result<()> {
    if raw.len() > 6 * 1024 * 1024 {
        anyhow::bail!("Image preview is limited to 6 MB")
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(raw)).with_guessed_format()?;
    if !matches!(
        reader.format(),
        Some(image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP)
    ) {
        anyhow::bail!("Unsupported image format")
    }
    let (w, h) = reader.into_dimensions()?;
    if w == 0 || h == 0 || w > 8192 || h > 8192 || u64::from(w) * u64::from(h) > 16_000_000 {
        anyhow::bail!("Image dimensions exceed the preview limit")
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_non_image_and_oversized_payloads() {
        assert!(validate(b"not a raster image").is_err());
        assert!(validate(&vec![0; 6 * 1024 * 1024 + 1]).is_err());
        assert!(!supported("script.svg"));
        assert!(supported("Screenshot.PNG"));
    }
}
