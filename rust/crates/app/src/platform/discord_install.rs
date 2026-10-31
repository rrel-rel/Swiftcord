//! Reads the installed Discord desktop client's versions at startup, so Swiftcord describes itself as that
//! client (see `swiftcord_core::identity`). Each value falls back to the last one found (kept in the cache
//! folder) and then to the built-in default, so a missing client or an unreadable file changes nothing.

use std::path::{Path, PathBuf};

use swiftcord_core::identity::{self, Identity};

/// What was found last time, and the desktop app version the Chrome and Electron versions were read from
/// (scanning its 200 MB executable is only worth doing when that changes).
#[derive(serde::Serialize, serde::Deserialize)]
struct Cached {
    identity: Identity,
    scanned_version: String,
}

fn cache_path() -> PathBuf {
    super::data_dir().join("cache").join("discord-identity.json")
}

/// Reads the installed client and sets the identity. Call before anything connects.
pub fn apply() {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let roaming = std::env::var_os("APPDATA").map(PathBuf::from);
    let cached: Option<Cached> = std::fs::read(cache_path()).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let mut id = cached.as_ref().map_or_else(Identity::default, |c| c.identity.clone());
    let mut scanned_version = cached.as_ref().map(|c| c.scanned_version.clone()).unwrap_or_default();

    if let Some((version, folder)) = local.as_deref().and_then(installed_app) {
        if version != scanned_version {
            match exe_versions(&folder.join("Discord.exe")) {
                Some((chrome, electron)) => {
                    id.chrome_version = chrome;
                    id.electron_version = electron;
                    scanned_version = version.clone();
                }
                None => tracing::warn!("couldn't read Chrome and Electron versions from Discord {version}"),
            }
        }
        id.client_version = version;
    }
    if let Some(n) = roaming.as_deref().and_then(|r| native_build(&r.join("discord").join("sentry").join("scope_v3.json"))) {
        id.native_build_number = n;
    }
    if let Some(n) = roaming.as_deref().and_then(|r| web_build(&r.join("discord").join("Cache").join("Cache_Data"))) {
        id.client_build_number = n;
    }
    if let Some(build) = windows_build() {
        id.os_version = format!("10.0.{build}");
    }

    tracing::info!(
        "identifying as Discord {} (web build {}, native build {}, Chrome {}, Electron {}, Windows {})",
        id.client_version,
        id.client_build_number,
        id.native_build_number,
        id.chrome_version,
        id.electron_version,
        id.os_version
    );
    let save = Cached { identity: id.clone(), scanned_version };
    if let Ok(json) = serde_json::to_vec(&save) {
        let _ = std::fs::create_dir_all(cache_path().parent().expect("cache dir"));
        let _ = std::fs::write(cache_path(), json);
    }
    identity::set(id);
}

/// The newest `app-X.Y.Z` folder under `%LOCALAPPDATA%\Discord`: its version and path.
fn installed_app(local: &Path) -> Option<(String, PathBuf)> {
    let parse = |v: &str| v.split('.').map(|p| p.parse::<u32>().ok()).collect::<Option<Vec<u32>>>();
    std::fs::read_dir(local.join("Discord"))
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let version = name.strip_prefix("app-")?.to_owned();
            Some((parse(&version)?, version, e.path()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, version, path)| (version, path))
}

/// The digits and dots that follow `prefix` at its first occurrence in `bytes` followed by a digit.
fn version_after(bytes: &[u8], prefix: &[u8]) -> Option<String> {
    memchr::memmem::find_iter(bytes, prefix).find_map(|at| {
        let rest = &bytes[at + prefix.len()..];
        let len = rest.iter().take(32).take_while(|b| b.is_ascii_digit() || **b == b'.').count();
        let v = std::str::from_utf8(&rest[..len]).ok()?.trim_end_matches('.');
        (v.contains('.') && v.starts_with(|c: char| c.is_ascii_digit())).then(|| v.to_owned())
    })
}

/// The Chrome and Electron versions Electron writes into its executable (in its user agent).
fn exe_versions(exe: &Path) -> Option<(String, String)> {
    let file = std::fs::File::open(exe).ok()?;
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    Some((version_after(&map, b"Chrome/")?, version_after(&map, b"Electron/")?))
}

/// The first number after `key` (past any quotes, colon, equals or spaces), if it has 4 to 8 digits.
fn number_after(text: &[u8], key: &[u8]) -> Option<u32> {
    memchr::memmem::find_iter(text, key).find_map(|at| {
        let rest = &text[at + key.len()..];
        let skip = rest.iter().take(8).take_while(|b| matches!(b, b'"' | b'\'' | b':' | b'=' | b',' | b' ')).count();
        let digits = rest[skip..].iter().take_while(|b| b.is_ascii_digit()).count();
        (4..=8).contains(&digits).then(|| std::str::from_utf8(&rest[skip..skip + digits]).ok()?.parse().ok()).flatten()
    })
}

/// `nativeBuildNumber` from the desktop app's crash-reporting scope.
fn native_build(scope: &Path) -> Option<u32> {
    number_after(&std::fs::read(scope).ok()?, b"nativeBuildNumber")
}

/// The web client's build number, from the newest of its cached scripts that mentions one.
fn web_build(cache: &Path) -> Option<u32> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(cache)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("f_"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    // Discord keeps its own index files open; the scripts it has fetched are readable.
    files.iter().take(400).find_map(|(_, path)| number_after(&std::fs::read(path).ok()?, b"buildNumber"))
}

/// Windows' build number (e.g. 26200), from the registry.
fn windows_build() -> Option<u32> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::w;
    let mut buf = [0u16; 32];
    let mut len = (buf.len() * 2) as u32;
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
        .ok()
        .ok()?;
    }
    let text = String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]);
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_build_numbers_are_found_in_text() {
        let exe = b"junk Chrome/ nope Chrome/152.0.7977.167 Safari Electron/44.9.3\0";
        assert_eq!(version_after(exe, b"Chrome/").as_deref(), Some("152.0.7977.167"));
        assert_eq!(version_after(exe, b"Electron/").as_deref(), Some("44.9.3"));
        assert_eq!(number_after(br#"{"nativeBuildNumber":"97026"}"#, b"nativeBuildNumber"), Some(97026));
        assert_eq!(number_after(b"x.buildNumber = 657850;", b"buildNumber"), Some(657850));
        // Too short to be a build number.
        assert_eq!(number_after(b"buildNumber:1", b"buildNumber"), None);
    }

    /// Reads the client installed here (run by hand on a machine with Discord).
    #[test]
    #[ignore]
    fn reads_the_installed_client() {
        let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap());
        let roaming = PathBuf::from(std::env::var_os("APPDATA").unwrap());
        let (version, folder) = installed_app(&local).expect("an installed client");
        let (chrome, electron) = exe_versions(&folder.join("Discord.exe")).expect("versions in the exe");
        println!("Discord {version}, Chrome {chrome}, Electron {electron}");
        println!("native build {:?}", native_build(&roaming.join(r"discord\sentry\scope_v3.json")));
        println!("web build {:?}", web_build(&roaming.join(r"discord\Cache\Cache_Data")));
        println!("Windows build {:?}", windows_build());
    }
}
