//! The user agent browser panes present.
//!
//! A bare WKWebView leaves the `Version/… Safari/…` tokens out of its user
//! agent, so sites that sniff for a known browser (Google search among them)
//! treat the pane as an unknown, older engine and serve a reduced page. The
//! pane says what it really is: the system's Safari engine, at the version of
//! the Safari installed alongside it.

use std::sync::OnceLock;

/// What every Safari user agent ends with; WebKit froze this build number.
const WEBKIT: &str = "AppleWebKit/605.1.15 (KHTML, like Gecko)";

/// The desktop user agent for a macOS pane.
pub fn desktop() -> String {
    format!(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) {WEBKIT} Version/{} Safari/605.1.15",
        safari_version()
    )
}

/// The user agent for the pane's mobile view: a phone running the same
/// Safari, so sites serve their phone layout and not only a narrow one.
pub fn mobile() -> String {
    let version = safari_version();
    let ios = version.replace('.', "_");
    format!(
        "Mozilla/5.0 (iPhone; CPU iPhone OS {ios} like Mac OS X) {WEBKIT} Version/{version} Mobile/15E148 Safari/604.1"
    )
}

/// `26.5`: the installed Safari's version, else one matching the system.
fn safari_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        installed_safari_version()
            .or_else(system_safari_version)
            .unwrap_or_else(|| "18.0".to_string())
    })
}

fn installed_safari_version() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let plist = std::fs::read_to_string("/Applications/Safari.app/Contents/Info.plist").ok()?;
    short_version(&plist)
}

/// Since macOS 26 Safari's major version is the system's.
fn system_safari_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::NSProcessInfo;
        let version = NSProcessInfo::processInfo().operatingSystemVersion();
        if version.majorVersion >= 26 {
            return Some(format!("{}.{}", version.majorVersion, version.minorVersion));
        }
    }
    None
}

/// `CFBundleShortVersionString` from an XML property list, as `major.minor`.
fn short_version(plist: &str) -> Option<String> {
    let after_key = plist.split("<key>CFBundleShortVersionString</key>").nth(1)?;
    let value = after_key.trim_start().strip_prefix("<string>")?;
    let value = &value[..value.find("</string>")?];
    let mut parts = value.trim().split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next().and_then(|minor| minor.parse().ok()).unwrap_or(0);
    Some(format!("{major}.{minor}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_safari_version_from_its_property_list() {
        let plist = "<dict>\n\t<key>CFBundleName</key>\n\t<string>Safari</string>\n\t<key>CFBundleShortVersionString</key>\n\t<string>26.5.1</string>\n</dict>";
        assert_eq!(short_version(plist).as_deref(), Some("26.5"));
        assert_eq!(
            short_version("<key>CFBundleShortVersionString</key><string>18</string>").as_deref(),
            Some("18.0")
        );
        assert_eq!(short_version("<key>CFBundleVersion</key><string>1</string>"), None);
    }

    #[test]
    fn user_agents_carry_the_safari_tokens_sites_look_for() {
        let desktop = desktop();
        assert!(desktop.starts_with("Mozilla/5.0 (Macintosh;"));
        assert!(desktop.contains(" Version/"));
        assert!(desktop.ends_with(" Safari/605.1.15"));
        let mobile = mobile();
        assert!(mobile.contains("iPhone"));
        assert!(mobile.contains(" Mobile/"));
        assert!(mobile.contains(" Safari/"));
    }
}
