//! Reading the cookies other browsers on this computer keep, so the built-in
//! browser can start signed in to the same sites. Only the person starts an
//! import; nothing is uploaded, and the other browser's files are copied
//! before reading so they are never locked or changed.
//!
//! - Firefox (every platform) stores cookies in plain SQLite.
//! - Chromium-family browsers on macOS encrypt cookie values with a key kept
//!   in the login Keychain (one prompt, which the person answers). Values are
//!   AES-128-CBC with a key derived by PBKDF2-HMAC-SHA1. On Windows they are
//!   bound to the browser itself and cannot be read, so those browsers are
//!   not offered there.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// Cookies imported at most from one profile.
const MAX_COOKIES: usize = 20_000;
/// Seconds between 1601-01-01 (Chromium's epoch) and 1970-01-01.
const WINDOWS_EPOCH_OFFSET: i64 = 11_644_473_600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// Chromium-family; the Keychain item holding its cookie key (macOS).
    Chromium { keychain: &'static str },
    Firefox,
}

/// One browser profile cookies can be imported from.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// "Google Chrome", "Firefox"…
    pub browser: String,
    /// The profile's own name ("Default", "Work", "default-release").
    pub profile: String,
    pub family: Family,
    /// The cookie database.
    pub database: PathBuf,
}

impl Source {
    pub fn label(&self) -> String {
        format!("{} — {}", self.browser, self.profile)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SameSite {
    None,
    Lax,
    Strict,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportedCookie {
    pub name: String,
    pub value: String,
    /// As the browser stored it: a leading dot covers subdomains.
    pub domain: String,
    pub path: String,
    /// Unix seconds; `None` for a cookie that ends with the browsing session.
    pub expires: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<SameSite>,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

// ── Finding browsers ─────────────────────────────────────────────────────────

/// Chromium-family browsers: (name, folder under Application Support, Keychain item).
#[cfg(target_os = "macos")]
const CHROMIUM: &[(&str, &str, &str)] = &[
    ("Google Chrome", "Google/Chrome", "Chrome Safe Storage"),
    ("Microsoft Edge", "Microsoft Edge", "Microsoft Edge Safe Storage"),
    ("Brave", "BraveSoftware/Brave-Browser", "Brave Safe Storage"),
    ("Arc", "Arc/User Data", "Arc Safe Storage"),
    ("Chromium", "Chromium", "Chromium Safe Storage"),
    ("Vivaldi", "Vivaldi", "Vivaldi Safe Storage"),
];

/// Profile folder → the name the person gave it, from Chromium's Local State.
fn chromium_profile_names(user_data: &Path) -> std::collections::HashMap<String, String> {
    let Ok(bytes) = std::fs::read(user_data.join("Local State")) else { return Default::default() };
    let Ok(state) = serde_json::from_slice::<Value>(&bytes) else { return Default::default() };
    state
        .pointer("/profile/info_cache")
        .and_then(Value::as_object)
        .map(|cache| {
            cache
                .iter()
                .filter_map(|(dir, info)| Some((dir.clone(), info.get("name")?.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn chromium_sources(out: &mut Vec<Source>) {
    let Some(support) = dirs::data_dir() else { return };
    for (browser, folder, keychain) in CHROMIUM {
        let user_data = support.join(folder);
        let names = chromium_profile_names(&user_data);
        let mut profiles: Vec<PathBuf> = std::fs::read_dir(&user_data)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name().and_then(|n| n.to_str()).is_some_and(|name| name == "Default" || name.starts_with("Profile "))
            })
            .collect();
        // By the names people gave them, not folder order ("Profile 12" < "Profile 2").
        profiles.sort_by_key(|profile| {
            let dir = profile.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            (dir != "Default", names.get(&dir).cloned().unwrap_or(dir).to_lowercase())
        });
        for profile in profiles {
            // Newer versions keep the database under Network/.
            let Some(database) = [profile.join("Network/Cookies"), profile.join("Cookies")]
                .into_iter()
                .find(|path| path.is_file())
            else {
                continue;
            };
            let dir = profile.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            out.push(Source {
                browser: browser.to_string(),
                profile: names.get(&dir).cloned().unwrap_or(dir),
                family: Family::Chromium { keychain },
                database,
            });
        }
    }
}

fn firefox_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if cfg!(target_os = "macos") {
        roots.extend(dirs::data_dir().map(|d| d.join("Firefox/Profiles")));
    } else if cfg!(windows) {
        roots.extend(dirs::config_dir().map(|d| d.join("Mozilla/Firefox/Profiles")));
    } else {
        roots.extend(dirs::home_dir().map(|h| h.join(".mozilla/firefox")));
        roots.extend(dirs::home_dir().map(|h| h.join("snap/firefox/common/.mozilla/firefox")));
    }
    roots
}

fn firefox_sources(out: &mut Vec<Source>) {
    for root in firefox_roots() {
        let mut profiles: Vec<PathBuf> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.join("cookies.sqlite").is_file())
            .collect();
        profiles.sort();
        for profile in profiles {
            // Folders are "<random>.<name>"; the name is what people recognise.
            let folder = profile.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let name = folder.split_once('.').map_or(folder.as_str(), |(_, name)| name).to_string();
            out.push(Source {
                browser: "Firefox".into(),
                profile: name,
                family: Family::Firefox,
                database: profile.join("cookies.sqlite"),
            });
        }
    }
}

/// Browser profiles on this computer whose cookies can be imported.
pub fn sources() -> Vec<Source> {
    let mut sources = Vec::new();
    #[cfg(target_os = "macos")]
    chromium_sources(&mut sources);
    firefox_sources(&mut sources);
    sources
}

// ── Reading ──────────────────────────────────────────────────────────────────

/// A private copy of a browser's database (with its write-ahead log), so a
/// running browser's lock never blocks the read and nothing of theirs changes.
struct Copy {
    folder: PathBuf,
    database: PathBuf,
}

impl Copy {
    fn of(database: &Path) -> Result<Self, String> {
        let folder = std::env::temp_dir().join(format!("dragent-cookies-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let name = database.file_name().ok_or("no database")?;
        let copy = folder.join(name);
        std::fs::copy(database, &copy).map_err(|e| format!("Couldn't read the browser's cookies: {e}"))?;
        for suffix in ["-wal", "-shm"] {
            let mut side = database.as_os_str().to_owned();
            side.push(suffix);
            let mut target = copy.as_os_str().to_owned();
            target.push(suffix);
            let _ = std::fs::copy(PathBuf::from(side), PathBuf::from(target));
        }
        Ok(Self { folder, database: copy })
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

fn open(path: &Path) -> Result<diesel::SqliteConnection, String> {
    use diesel::Connection;
    diesel::SqliteConnection::establish(&path.to_string_lossy())
        .map_err(|error| format!("Couldn't open the browser's cookies: {error}"))
}

/// Read every unexpired cookie of `source`. Blocking, and on macOS may show
/// the Keychain's prompt for a Chromium browser: run it off the UI thread.
pub fn read(source: &Source) -> Result<Vec<ImportedCookie>, String> {
    let copy = Copy::of(&source.database)?;
    let mut connection = open(&copy.database)?;
    let mut cookies = match source.family {
        Family::Firefox => read_firefox(&mut connection)?,
        Family::Chromium { keychain } => read_chromium(&mut connection, keychain)?,
    };
    let now = now();
    cookies.retain(|cookie| cookie.expires.is_none_or(|at| at > now) && !cookie.name.is_empty());
    cookies.truncate(MAX_COOKIES);
    Ok(cookies)
}

fn read_firefox(connection: &mut diesel::SqliteConnection) -> Result<Vec<ImportedCookie>, String> {
    use diesel::prelude::*;
    use diesel::sql_types::{BigInt, Integer, Text};
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Text)]
        host: String,
        #[diesel(sql_type = Text)]
        name: String,
        #[diesel(sql_type = Text)]
        value: String,
        #[diesel(sql_type = Text)]
        path: String,
        #[diesel(sql_type = BigInt)]
        expiry: i64,
        #[diesel(sql_type = Integer)]
        secure: i32,
        #[diesel(sql_type = Integer)]
        http_only: i32,
        #[diesel(sql_type = Integer)]
        same_site: i32,
    }
    let rows: Vec<Row> = diesel::sql_query(
        "SELECT host, name, value, path, expiry, isSecure AS secure, isHttpOnly AS http_only, \
         sameSite AS same_site FROM moz_cookies",
    )
    .load(connection)
    .map_err(|error| format!("Couldn't read Firefox's cookies: {error}"))?;
    Ok(rows
        .into_iter()
        .map(|row| ImportedCookie {
            name: row.name,
            value: row.value,
            domain: row.host,
            path: row.path,
            // Seconds, or milliseconds in newer versions.
            expires: Some(if row.expiry > 100_000_000_000 { row.expiry / 1000 } else { row.expiry }),
            secure: row.secure != 0,
            http_only: row.http_only != 0,
            same_site: match row.same_site {
                0 => Some(SameSite::None),
                1 => Some(SameSite::Lax),
                2 => Some(SameSite::Strict),
                _ => None,
            },
        })
        .collect())
}

fn read_chromium(connection: &mut diesel::SqliteConnection, keychain: &str) -> Result<Vec<ImportedCookie>, String> {
    use diesel::prelude::*;
    use diesel::sql_types::{BigInt, Binary, Integer, Text};
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Text)]
        host_key: String,
        #[diesel(sql_type = Text)]
        name: String,
        #[diesel(sql_type = Text)]
        value: String,
        #[diesel(sql_type = Binary)]
        encrypted_value: Vec<u8>,
        #[diesel(sql_type = Text)]
        path: String,
        #[diesel(sql_type = BigInt)]
        expires_utc: i64,
        #[diesel(sql_type = Integer)]
        is_secure: i32,
        #[diesel(sql_type = Integer)]
        is_httponly: i32,
        #[diesel(sql_type = Integer)]
        samesite: i32,
    }
    #[derive(QueryableByName)]
    struct Meta {
        #[diesel(sql_type = Text)]
        value: String,
    }
    let rows: Vec<Row> = diesel::sql_query(
        "SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly, samesite FROM cookies",
    )
    .load(connection)
    .map_err(|error| format!("Couldn't read the browser's cookies: {error}"))?;
    // From database version 24 the decrypted value starts with a 32-byte
    // digest of the cookie's domain.
    let version: i64 = diesel::sql_query("SELECT value FROM meta WHERE key = 'version'")
        .load::<Meta>(connection)
        .ok()
        .and_then(|rows| rows.first()?.value.parse().ok())
        .unwrap_or(0);
    let needs_key = rows.iter().any(|row| row.encrypted_value.starts_with(b"v10"));
    let key = if needs_key { Some(chromium_key(keychain)?) } else { None };
    let mut cookies = Vec::with_capacity(rows.len());
    for row in rows {
        let value = if row.encrypted_value.starts_with(b"v10") {
            let Some(key) = &key else { continue };
            match decrypt_v10(key, &row.encrypted_value[3..], version >= 24) {
                Some(value) => value,
                None => continue,
            }
        } else {
            row.value
        };
        cookies.push(ImportedCookie {
            name: row.name,
            value,
            domain: row.host_key,
            path: row.path,
            expires: (row.expires_utc > 0).then(|| row.expires_utc / 1_000_000 - WINDOWS_EPOCH_OFFSET),
            secure: row.is_secure != 0,
            http_only: row.is_httponly != 0,
            same_site: match row.samesite {
                0 => Some(SameSite::None),
                1 => Some(SameSite::Lax),
                2 => Some(SameSite::Strict),
                _ => None,
            },
        });
    }
    Ok(cookies)
}

/// The browser's cookie key from the login Keychain (the person is asked
/// to allow it), derived as the browser derives it.
#[cfg(target_os = "macos")]
fn chromium_key(keychain: &str) -> Result<[u8; 16], String> {
    let output = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-w", "-s", keychain])
        .output()
        .map_err(|error| format!("Couldn't ask the Keychain: {error}"))?;
    if !output.status.success() {
        return Err("The Keychain didn't give access to the browser's cookies (the request was denied or the item is missing).".into());
    }
    let password = String::from_utf8_lossy(&output.stdout).trim_end_matches(['\n', '\r']).to_string();
    Ok(pbkdf2_sha1_16(password.as_bytes(), b"saltysalt", 1003))
}

#[cfg(not(target_os = "macos"))]
fn chromium_key(_: &str) -> Result<[u8; 16], String> {
    Err("Importing from this browser isn't supported on this platform.".into())
}

/// PBKDF2-HMAC-SHA1 for a single 16-byte block (RFC 8018).
fn pbkdf2_sha1_16(password: &[u8], salt: &[u8], rounds: u32) -> [u8; 16] {
    use hmac::{Hmac, Mac};
    type HmacSha1 = Hmac<sha1::Sha1>;
    let prf = |data: &[u8]| -> [u8; 20] {
        let mut mac = HmacSha1::new_from_slice(password).expect("HMAC takes any key length");
        mac.update(data);
        mac.finalize().into_bytes().into()
    };
    let mut first = salt.to_vec();
    first.extend_from_slice(&1u32.to_be_bytes());
    let mut u = prf(&first);
    let mut block = u;
    for _ in 1..rounds {
        u = prf(&u);
        for (out, byte) in block.iter_mut().zip(u) {
            *out ^= byte;
        }
    }
    let mut key = [0u8; 16];
    key.copy_from_slice(&block[..16]);
    key
}

/// AES-128-CBC with a 16-space IV and PKCS#7 padding; `digest_prefix` drops
/// the 32-byte domain digest newer databases put first.
fn decrypt_v10(key: &[u8; 16], data: &[u8], digest_prefix: bool) -> Option<String> {
    use aes::cipher::{BlockDecryptMut, KeyIvInit};
    type Decryptor = cbc::Decryptor<aes::Aes128>;
    let mut buffer = data.to_vec();
    let plain = Decryptor::new(key.into(), &[b' '; 16].into())
        .decrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buffer)
        .ok()?;
    let plain = if digest_prefix { plain.get(32..)? } else { plain };
    String::from_utf8(plain.to_vec()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbkdf2_matches_the_rfc_6070_vector() {
        // RFC 6070: "password" / "salt", 2 rounds → ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957.
        assert_eq!(
            pbkdf2_sha1_16(b"password", b"salt", 2),
            [0xea, 0x6c, 0x01, 0x4d, 0xc7, 0x2d, 0x6f, 0x8c, 0xcd, 0x1e, 0xd9, 0x2a, 0xce, 0x1d, 0x41, 0xf0]
        );
    }

    fn encrypt(key: &[u8; 16], plain: &[u8]) -> Vec<u8> {
        use aes::cipher::{BlockEncryptMut, KeyIvInit};
        type Encryptor = cbc::Encryptor<aes::Aes128>;
        let mut buffer = vec![0u8; plain.len() + 16];
        buffer[..plain.len()].copy_from_slice(plain);
        Encryptor::new(key.into(), &[b' '; 16].into())
            .encrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buffer, plain.len())
            .unwrap()
            .to_vec()
    }

    #[test]
    fn decrypts_values_with_and_without_the_domain_digest() {
        let key = pbkdf2_sha1_16(b"peanuts", b"saltysalt", 1003);
        assert_eq!(decrypt_v10(&key, &encrypt(&key, b"session=abc"), false).as_deref(), Some("session=abc"));
        let mut prefixed = vec![7u8; 32];
        prefixed.extend_from_slice(b"token");
        assert_eq!(decrypt_v10(&key, &encrypt(&key, &prefixed), true).as_deref(), Some("token"));
        // The wrong key never yields a value.
        let other = pbkdf2_sha1_16(b"other", b"saltysalt", 1003);
        assert_ne!(decrypt_v10(&other, &encrypt(&key, b"session=abc"), false).as_deref(), Some("session=abc"));
    }

    #[test]
    fn reads_a_firefox_cookie_database_without_touching_it() {
        use diesel::RunQueryDsl;
        let folder = std::env::temp_dir().join(format!("dragent-firefox-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&folder).unwrap();
        let database = folder.join("cookies.sqlite");
        let mut connection = open(&database).unwrap();
        diesel::sql_query(
            "CREATE TABLE moz_cookies (host TEXT, name TEXT, value TEXT, path TEXT, expiry INTEGER, \
             isSecure INTEGER, isHttpOnly INTEGER, sameSite INTEGER)",
        )
        .execute(&mut connection)
        .unwrap();
        let future = now() + 3600;
        diesel::sql_query(format!(
            "INSERT INTO moz_cookies VALUES ('.example.com','sid','v1','/',{future},1,1,1), \
             ('old.test','gone','x','/',100,0,0,0), ('ms.test','m','y','/',{},0,0,2)",
            future * 1000
        ))
        .execute(&mut connection)
        .unwrap();
        drop(connection);
        let before = std::fs::read(&database).unwrap();
        let source = Source {
            browser: "Firefox".into(),
            profile: "default".into(),
            family: Family::Firefox,
            database: database.clone(),
        };
        let cookies = read(&source).unwrap();
        assert_eq!(cookies.len(), 2, "the expired cookie is left out");
        assert_eq!(cookies[0].domain, ".example.com");
        assert!(cookies[0].secure && cookies[0].http_only);
        assert_eq!(cookies[0].same_site, Some(SameSite::Lax));
        assert_eq!(cookies[1].expires, Some(future), "milliseconds are read as seconds");
        assert_eq!(std::fs::read(&database).unwrap(), before, "the browser's file is unchanged");
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn chromium_profile_names_come_from_local_state() {
        let folder = std::env::temp_dir().join(format!("dragent-chromium-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("Local State"),
            r#"{"profile":{"info_cache":{"Default":{"name":"Personal"},"Profile 1":{"name":"Work"}}}}"#,
        )
        .unwrap();
        let names = chromium_profile_names(&folder);
        assert_eq!(names.get("Profile 1").map(String::as_str), Some("Work"));
        let _ = std::fs::remove_dir_all(folder);
    }
}
