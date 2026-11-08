//! Self-update from GitHub Releases.
//!
//! Every push to `main` builds Viewer on GitHub Actions and publishes a
//! release tagged `build-<N>` with the Windows and Linux binaries and their
//! SHA-256 checksums. This module asks GitHub for the latest release, and if
//! its build number is newer than ours, downloads the binary for this
//! platform, checks its SHA-256, swaps it in place of the running program and
//! restarts.
//!
//! Swapping a running program: on Windows a running .exe cannot be
//! overwritten but it can be renamed, so the old one becomes
//! `viewer.old.exe` (deleted on the next start). On Linux the new file is
//! renamed over the old path; the running process keeps its open copy.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const REPO: &str = "pink-bourbon-estate/viewer";

/// The CI build number baked in at compile time (0 for local builds).
pub fn current_build() -> u32 {
    option_env!("VIEWER_BUILD").and_then(|s| s.parse().ok()).unwrap_or(0)
}

pub fn version_label() -> String {
    match current_build() {
        0 => format!("{} (local build)", env!("CARGO_PKG_VERSION")),
        n => format!("{} build {n}", env!("CARGO_PKG_VERSION")),
    }
}

fn asset_name() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("viewer.exe")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("viewer-linux-x86_64")
    } else {
        None
    }
}

#[derive(Clone, Debug)]
pub struct Release {
    pub build: u32,
    pub title: String,
    pub notes: String,
    pub page: String,
    pub url: String,
    pub sha_url: Option<String>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .proxy(ureq::Proxy::try_from_env())
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .user_agent(concat!("viewer-updater/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn get_bytes(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut resp = agent().get(url).header("Accept", "application/octet-stream").call().map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    resp.body_mut()
        .as_reader()
        .take(limit)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

/// Ask GitHub for the newest release. `Ok(None)` = already up to date.
pub fn check() -> Result<Option<Release>, String> {
    let Some(asset) = asset_name() else {
        return Err("Automatic updates are only built for Windows and Linux (x86-64).".into());
    };
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut resp = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => "No releases have been published yet.".to_string(),
            other => format!("Could not reach GitHub: {other}"),
        })?;
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let rel: GhRelease = serde_json::from_str(&text).map_err(|e| format!("Unexpected reply from GitHub: {e}"))?;
    let build: u32 = rel.tag_name.trim_start_matches("build-").parse().map_err(|_| format!("Unrecognised release tag {}", rel.tag_name))?;
    let Some(bin) = rel.assets.iter().find(|a| a.name == asset) else {
        return Err(format!("The latest release has no {asset}."));
    };
    let sha = rel.assets.iter().find(|a| a.name == format!("{asset}.sha256")).map(|a| a.browser_download_url.clone());
    // Local builds (0) always offer the newest release.
    if build <= current_build() {
        return Ok(None);
    }
    Ok(Some(Release {
        build,
        title: rel.name.unwrap_or_else(|| rel.tag_name.clone()),
        notes: rel.body.unwrap_or_default(),
        page: rel.html_url,
        url: bin.browser_download_url.clone(),
        sha_url: sha,
    }))
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let stem = exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "viewer".into());
    let ext = exe.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    exe.with_file_name(format!("{stem}{suffix}{ext}"))
}

/// Download, verify and install. Returns the path of the new program.
pub fn install(rel: &Release) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Cannot find the running program: {e}"))?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let data = get_bytes(&rel.url, 300 * 1024 * 1024)?;
    if data.len() < 100_000 {
        return Err("The download looks incomplete; nothing was changed.".into());
    }
    if let Some(sha_url) = &rel.sha_url {
        let expected = String::from_utf8_lossy(&get_bytes(sha_url, 4096)?).split_whitespace().next().unwrap_or("").to_lowercase();
        let actual: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
        if expected.len() != 64 || expected != actual {
            return Err("The download failed its checksum; nothing was changed.".into());
        }
    }
    let new_path = sibling(&exe, ".new");
    std::fs::write(&new_path, &data).map_err(|e| {
        format!("Cannot write next to {} ({e}). Move Viewer to a folder you can write to, e.g. C:\\Tools\\Viewer.", exe.display())
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&new_path, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(windows)]
    {
        let old = sibling(&exe, ".old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&exe, &old).map_err(|e| {
            let _ = std::fs::remove_file(&new_path);
            format!("Cannot replace the running program: {e}")
        })?;
        if let Err(e) = std::fs::rename(&new_path, &exe) {
            // Put the old one back so Viewer still starts.
            let _ = std::fs::rename(&old, &exe);
            return Err(format!("Cannot install the update: {e}"));
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(&new_path, &exe).map_err(|e| {
            let _ = std::fs::remove_file(&new_path);
            format!("Cannot install the update: {e}")
        })?;
    }
    Ok(exe)
}

/// Remove the previous version left behind by an update on Windows.
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(sibling(&exe, ".old"));
        let _ = std::fs::remove_file(sibling(&exe, ".new"));
    }
}

/// Start the freshly installed program, reopening `open` if given.
pub fn relaunch(exe: &Path, open: Option<&Path>) -> Result<(), String> {
    let mut cmd = std::process::Command::new(exe);
    if let Some(p) = open {
        cmd.arg(p);
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("Updated, but could not restart: {e}. Please start Viewer again."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sibling_names() {
        assert_eq!(sibling(Path::new("C:/Tools/viewer.exe"), ".old"), PathBuf::from("C:/Tools/viewer.old.exe"));
        assert_eq!(sibling(Path::new("/home/a/bin/viewer"), ".new"), PathBuf::from("/home/a/bin/viewer.new"));
    }

    #[test]
    fn parses_release_json() {
        let j = r#"{"tag_name":"build-12","name":"Viewer build 12","body":"notes","html_url":"https://x",
            "assets":[{"name":"viewer.exe","browser_download_url":"https://a"},
                      {"name":"viewer.exe.sha256","browser_download_url":"https://b"}]}"#;
        let r: Result<GhRelease, _> = serde_json::from_str(j);
        assert!(r.is_ok_and(|r| r.tag_name == "build-12" && r.assets.len() == 2));
    }
}
