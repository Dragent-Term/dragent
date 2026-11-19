//! Site icons for browser tabs. The page reports the icon it declares; it is
//! fetched once, turned into a small PNG (SVG is kept as drawn) and kept in
//! the browser profile, one file per site, so every tab on that site and
//! later launches reuse it.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Icons larger than this are not icons.
const MAX_BYTES: u64 = 512 * 1024;
/// Side of the stored PNG: sharp at a tab's size on a Retina display.
const STORED_SIDE: u32 = 64;
/// A cached icon is fetched again after this long.
const REFRESH_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Where the icon for `page_url`'s site is (or will be) cached.
pub fn cached(profile: &Path, page_url: &str) -> Option<PathBuf> {
    let host = url::Url::parse(page_url).ok()?.host_str()?.to_ascii_lowercase();
    let name: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    let folder = profile.join("favicons");
    // An SVG icon is stored as one; everything else as PNG.
    let svg = folder.join(format!("{name}.svg"));
    Some(if svg.exists() { svg } else { folder.join(format!("{name}.png")) })
}

/// Whether the cached icon at `path` is there and recent.
pub fn is_fresh(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < REFRESH_AFTER)
}

/// Fetch `icon_url` and store it for `page_url`'s site. Blocking: run it off
/// the UI thread. Returns where it was stored.
pub fn fetch(profile: &Path, page_url: &str, icon_url: &str) -> Result<PathBuf, String> {
    if !(icon_url.starts_with("https://") || icon_url.starts_with("http://")) {
        return Err("not a web address".into());
    }
    let response = ureq::get(icon_url)
        .timeout(Duration::from_secs(8))
        .set("User-Agent", &super::user_agent::desktop())
        .set("Accept", "image/svg+xml,image/png,image/x-icon,image/*;q=0.8")
        .call()
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("icon too large".into());
    }
    let png = cached(profile, page_url).ok_or("no site")?.with_extension("png");
    std::fs::create_dir_all(png.parent().ok_or("no folder")?).map_err(|e| e.to_string())?;
    let (path, data) = match normalise(&bytes)? {
        Icon::Svg(svg) => (png.with_extension("svg"), svg),
        Icon::Png(png_bytes) => (png, png_bytes),
    };
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    // A site that moved from SVG to a bitmap (or back) keeps one file.
    let other = if path.extension().is_some_and(|ext| ext == "svg") { "png" } else { "svg" };
    let _ = std::fs::remove_file(path.with_extension(other));
    Ok(path)
}

enum Icon {
    Svg(Vec<u8>),
    Png(Vec<u8>),
}

/// An SVG as is; any bitmap (ICO, PNG, JPEG, WebP, GIF) as a square PNG.
fn normalise(bytes: &[u8]) -> Result<Icon, String> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    if head.trim_start().starts_with('<') && head.contains("<svg") {
        return Ok(Icon::Svg(bytes.to_vec()));
    }
    let image = image::load_from_memory(bytes).map_err(|error| error.to_string())?;
    let image = if image.width() > STORED_SIDE || image.height() > STORED_SIDE {
        image.resize(STORED_SIDE, STORED_SIDE, image::imageops::FilterType::Lanczos3)
    } else {
        image
    };
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(Icon::Png(png))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_icon_file_per_site() {
        let profile = Path::new("/profile");
        let a = cached(profile, "https://www.example.com/a?b").unwrap();
        let b = cached(profile, "https://WWW.example.com/other").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, Path::new("/profile/favicons/www.example.com.png"));
        assert_eq!(cached(profile, "about:blank"), None);
    }

    #[test]
    fn bitmaps_become_small_pngs_and_svg_stays_svg() {
        let big = image::RgbaImage::from_pixel(256, 256, image::Rgba([10, 20, 30, 255]));
        let mut ico = Vec::new();
        image::DynamicImage::ImageRgba8(big)
            .write_to(&mut std::io::Cursor::new(&mut ico), image::ImageFormat::Ico)
            .unwrap();
        let Icon::Png(png) = normalise(&ico).unwrap() else { panic!("expected a PNG") };
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (STORED_SIDE, STORED_SIDE));

        let svg = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg"/>"#;
        assert!(matches!(normalise(svg), Ok(Icon::Svg(_))));
        assert!(normalise(b"not an image").is_err());
    }
}
