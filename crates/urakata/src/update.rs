//! Update check against GitHub Releases.
//!
//! The latest published release (not a draft, not a pre-release) of
//! `Tokahiro/urakata` holds `urakata-setup-<version>.exe` and its
//! `.sha256` file (`.github/workflows/release.yml`). An update downloads
//! both, checks the hash and starts the installer silently. The installer
//! keeps a working driver and the logon start.

use serde::Deserialize;

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::{check, install};

/// GitHub API URL of the latest published release.
pub const LATEST_URL: &str = "https://api.github.com/repos/Tokahiro/urakata/releases/latest";

/// Web page of the latest release, for people.
pub const RELEASE_PAGE: &str = "https://github.com/Tokahiro/urakata/releases/latest";

/// Installer argument: open the settings window on its About page when
/// the update is done (`installer/urakata.iss`).
pub const OPEN_APP_ARG: &str = "/OPENAPP=1";

/// Settings window argument: open on the About page.
pub const ABOUT_ARG: &str = "--about";

/// Hosts that a request may start at.
pub const ALLOWED_HOSTS: [&str; 2] = ["api.github.com", "github.com"];

/// Hosts a request may end at after redirects: the start hosts and
/// GitHub's asset storage (`*.githubusercontent.com`).
pub fn final_host_allowed(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', ':', '?']).next().unwrap_or("");
    ALLOWED_HOSTS.contains(&host) || host.ends_with(".githubusercontent.com")
}

/// Version of this build.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// A release that is newer than this build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Version without the `v`, for example `0.2.0`.
    pub version: String,
    /// Installer file name.
    pub setup_name: String,
    /// Download URL of the installer.
    pub setup_url: String,
    /// Download URL of the `.sha256` file.
    pub sha256_url: String,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

/// `x.y.z` or `vx.y.z` as numbers. Anything else (pre-release suffixes
/// too) is `None`.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.').map(|p| {
        (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            .then(|| p.parse::<u64>().ok())
            .flatten()
    });
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

/// The release from the API answer, when it is newer than `current`.
/// `Ok(None)`: up to date. `Err`: the answer is not usable.
pub fn newer_release(json: &[u8], current: &str) -> Result<Option<Release>, String> {
    let api: ApiRelease =
        serde_json::from_slice(json).map_err(|e| format!("cannot read the release: {e}"))?;
    if api.draft || api.prerelease {
        return Ok(None);
    }
    let latest = parse_version(&api.tag_name)
        .ok_or_else(|| format!("release tag '{}' is not vX.Y.Z", api.tag_name))?;
    let current = parse_version(current).ok_or_else(|| format!("bad own version '{current}'"))?;
    if latest <= current {
        return Ok(None);
    }
    let version = format!("{}.{}.{}", latest.0, latest.1, latest.2);
    let setup_name = format!("urakata-setup-{version}.exe");
    let url = |name: &str| {
        api.assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.clone())
            .ok_or_else(|| format!("release {version} has no asset {name}"))
    };
    let setup_url = url(&setup_name)?;
    let sha256_url = url(&format!("{setup_name}.sha256"))?;
    for u in [&setup_url, &sha256_url] {
        split_url(u)?;
    }
    Ok(Some(Release {
        version,
        setup_name,
        setup_url,
        sha256_url,
    }))
}

/// Host and path of an `https://` URL on an allowed host.
pub fn split_url(url: &str) -> Result<(&str, &str), String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| format!("not an https URL: {url}"))?;
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    if !ALLOWED_HOSTS.contains(&host) {
        return Err(format!("download host {host} is not allowed"));
    }
    Ok((host, if path.is_empty() { "/" } else { path }))
}

/// The hash for `name` from a `.sha256` file (`<hex>  <name>` or just
/// `<hex>`), upper case.
pub fn parse_sha256_file(text: &str, name: &str) -> Option<String> {
    let mut words = text.split_whitespace();
    let hash = words.next()?;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match words.next() {
        None => {}
        Some(n) if n.trim_start_matches('*') == name => {}
        Some(_) => return None,
    }
    Some(hash.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn release_json(tag: &str, draft: bool, assets: &[&str]) -> Vec<u8> {
        let assets: Vec<_> = assets
            .iter()
            .map(|n| {
                serde_json::json!({
                    "name": n,
                    "browser_download_url":
                        format!("https://github.com/Tokahiro/urakata/releases/download/{tag}/{n}"),
                })
            })
            .collect();
        serde_json::to_vec(&serde_json::json!({
            "tag_name": tag, "draft": draft, "prerelease": false, "assets": assets,
        }))
        .unwrap()
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.2.10"), Some((0, 2, 10)));
        assert_eq!(parse_version("1.0.0"), Some((1, 0, 0)));
        assert_eq!(parse_version("v1.0"), None);
        assert_eq!(parse_version("v1.0.0-rc1"), None);
        assert_eq!(parse_version("v1.0.0.1"), None);
        assert_eq!(parse_version("v1..0"), None);
        assert_eq!(parse_version("v+1.0.0"), None);
        assert!(parse_version(CURRENT).is_some());
    }

    #[test]
    fn newer_release_found() {
        let json = release_json(
            "v0.2.0",
            false,
            &["urakata-setup-0.2.0.exe", "urakata-setup-0.2.0.exe.sha256"],
        );
        let r = newer_release(&json, "0.1.0").unwrap().unwrap();
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.setup_name, "urakata-setup-0.2.0.exe");
        assert!(r.sha256_url.ends_with("/urakata-setup-0.2.0.exe.sha256"));
    }

    #[test]
    fn same_older_or_draft_is_none() {
        let assets = ["urakata-setup-0.1.0.exe", "urakata-setup-0.1.0.exe.sha256"];
        assert_eq!(
            newer_release(&release_json("v0.1.0", false, &assets), "0.1.0"),
            Ok(None)
        );
        assert_eq!(
            newer_release(&release_json("v0.1.0", false, &assets), "0.10.0"),
            Ok(None)
        );
        assert_eq!(
            newer_release(&release_json("v9.0.0", true, &assets), "0.1.0"),
            Ok(None)
        );
    }

    #[test]
    fn missing_asset_is_error() {
        let json = release_json("v0.2.0", false, &["urakata-setup-0.2.0.exe"]);
        assert!(
            newer_release(&json, "0.1.0")
                .unwrap_err()
                .contains(".sha256")
        );
        assert!(newer_release(b"{}", "0.1.0").is_err());
    }

    #[test]
    fn foreign_host_is_refused() {
        let json = serde_json::to_vec(&serde_json::json!({
            "tag_name": "v0.2.0",
            "assets": [
                {"name": "urakata-setup-0.2.0.exe", "browser_download_url": "https://evil.example/x.exe"},
                {"name": "urakata-setup-0.2.0.exe.sha256", "browser_download_url": "https://github.com/a"},
            ],
        }))
        .unwrap();
        assert!(
            newer_release(&json, "0.1.0")
                .unwrap_err()
                .contains("not allowed")
        );
    }

    #[test]
    fn urls() {
        assert_eq!(
            split_url("https://api.github.com/repos/x"),
            Ok(("api.github.com", "/repos/x"))
        );
        assert_eq!(split_url("https://github.com"), Ok(("github.com", "/")));
        assert!(split_url("http://github.com/x").is_err());
        assert!(split_url("https://github.com.evil.example/x").is_err());
        assert!(split_url(LATEST_URL).is_ok());
    }

    #[test]
    fn redirect_targets() {
        assert!(final_host_allowed(
            "https://release-assets.githubusercontent.com/github-production/x?sig=1"
        ));
        assert!(final_host_allowed("https://github.com/a"));
        assert!(!final_host_allowed(
            "https://githubusercontent.com.evil.example/x"
        ));
        assert!(!final_host_allowed("https://evilgithubusercontent.com/x"));
        assert!(!final_host_allowed(
            "http://objects.githubusercontent.com/x"
        ));
    }

    #[test]
    fn sha256_files() {
        let name = "urakata-setup-0.2.0.exe";
        let upper = HASH.to_ascii_uppercase();
        assert_eq!(
            parse_sha256_file(&format!("{HASH}  {name}"), name),
            Some(upper.clone())
        );
        assert_eq!(
            parse_sha256_file(&format!("{HASH} *{name}\n"), name),
            Some(upper.clone())
        );
        assert_eq!(parse_sha256_file(HASH, name), Some(upper));
        assert_eq!(parse_sha256_file(&format!("{HASH}  other.exe"), name), None);
        assert_eq!(parse_sha256_file("abc  x", name), None);
        assert_eq!(parse_sha256_file("", name), None);
    }
}
