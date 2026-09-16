use serde::Deserialize;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const DEFAULT_GITHUB_REPO: &str = "Hi9841/tdt";

#[derive(Debug, Clone, PartialEq)]
pub enum UpdatePhase {
    Idle,
    Checking,
    UpToDate,
    Available { version: String, asset_url: String },
    Downloading,
    Ready { installer: PathBuf },
    Failed(String),
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

pub fn github_repo() -> String {
    std::env::var("TDT_GITHUB_REPO").unwrap_or_else(|_| DEFAULT_GITHUB_REPO.to_string())
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn updates_disabled() -> bool {
    matches!(
        std::env::var("TDT_DISABLE_UPDATES").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

pub fn version_newer(latest: &str, current: &str) -> bool {
    match (parse_semver(latest), parse_semver(current)) {
        (Some(left), Some(right)) => left > right,
        _ => false,
    }
}

pub fn check_latest() -> Result<UpdatePhase, String> {
    if updates_disabled() {
        return Ok(UpdatePhase::UpToDate);
    }

    let repo = github_repo();
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let body = match http_get_string(&url) {
        Ok(body) => body,
        Err(error) if is_no_release(&error) => return Ok(UpdatePhase::UpToDate),
        Err(error) => return Err(error),
    };
    let release: GithubRelease =
        serde_json::from_str(&body).map_err(|e| format!("Invalid GitHub release JSON: {e}"))?;
    let latest = release.tag_name.trim().trim_start_matches('v').to_string();
    if !version_newer(&latest, current_version()) {
        return Ok(UpdatePhase::UpToDate);
    }
    let asset = pick_setup_asset(&release.assets)
        .ok_or_else(|| "Latest release has no TDT-Setup.exe asset".to_string())?;
    Ok(UpdatePhase::Available {
        version: latest,
        asset_url: asset.browser_download_url.clone(),
    })
}

pub fn download_installer(asset_url: &str) -> Result<PathBuf, String> {
    let bytes = http_get_bytes(asset_url)?;
    if bytes.len() < 64 {
        return Err("Downloaded installer was empty".to_string());
    }
    if !looks_like_pe(&bytes) {
        return Err("Downloaded file is not a Windows installer".to_string());
    }
    let path = std::env::temp_dir().join("TDT-Setup.exe");
    let mut file = File::create(&path).map_err(|e| format!("Could not write installer: {e}"))?;
    file.write_all(&bytes)
        .map_err(|e| format!("Could not write installer: {e}"))?;
    Ok(path)
}

pub fn launch_installer(path: &Path) -> Result<(), String> {
    Command::new(path)
        .args(["/SILENT", "/CLOSEAPPLICATIONS", "/NORESTART"])
        .spawn()
        .map_err(|e| format!("Could not start installer: {e}"))?;
    Ok(())
}

fn pick_setup_asset(assets: &[GithubAsset]) -> Option<&GithubAsset> {
    assets
        .iter()
        .find(|asset| asset.name.eq_ignore_ascii_case("TDT-Setup.exe"))
        .or_else(|| {
            assets.iter().find(|asset| {
                let lower = asset.name.to_ascii_lowercase();
                lower.ends_with(".exe") && lower.contains("setup")
            })
        })
}

#[cfg(test)]
fn is_setup_asset(name: &str) -> bool {
    pick_setup_asset(&[GithubAsset {
        name: name.to_string(),
        browser_download_url: String::new(),
    }])
    .is_some()
}

fn parse_semver(raw: &str) -> Option<(u64, u64, u64)> {
    let trimmed = raw.trim().trim_start_matches('v');
    let mut parts = trimmed.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts
        .next()
        .unwrap_or("0")
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()?;
    Some((major, minor, patch))
}

fn user_agent() -> String {
    format!(
        "TDT/{} (+https://github.com/{})",
        current_version(),
        github_repo()
    )
}

fn is_no_release(error: &str) -> bool {
    error.contains("404") || error.contains("Not Found")
}

fn looks_like_pe(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == b'M' && bytes[1] == b'Z'
}

fn check_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(30))
        .timeout_write(Duration::from_secs(30))
        .user_agent(&user_agent())
        .build()
}

fn download_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(120))
        .timeout_write(Duration::from_secs(60))
        .user_agent(&user_agent())
        .build()
}

fn http_get_string(url: &str) -> Result<String, String> {
    match check_agent()
        .get(url)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
    {
        Ok(response) => response
            .into_string()
            .map_err(|e| format!("Update check failed: {e}")),
        Err(ureq::Error::Status(404, _)) => Err("404 Not Found".to_string()),
        Err(error) => Err(format!("Update check failed: {error}")),
    }
}

fn http_get_bytes(url: &str) -> Result<Vec<u8>, String> {
    let response = download_agent()
        .get(url)
        .set("Accept", "application/octet-stream")
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Download failed: {e}"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_newer_compares_semver() {
        assert!(version_newer("0.2.0", "0.1.0"));
        assert!(version_newer("v1.0.0", "0.9.9"));
        assert!(!version_newer("0.1.0", "0.1.0"));
        assert!(!version_newer("0.1.0", "0.2.0"));
    }

    #[test]
    fn setup_asset_matches_installer_names() {
        assert!(is_setup_asset("TDT-Setup.exe"));
        assert!(is_setup_asset("tdt-0.1.0-setup.exe"));
        assert!(!is_setup_asset("notes.md"));
        assert!(!is_setup_asset("TDT.exe"));
    }

    #[test]
    fn pe_signature_rejects_html() {
        assert!(looks_like_pe(b"MZ\x90\x00"));
        assert!(!looks_like_pe(b"<!doctype html>"));
    }

    #[test]
    fn missing_release_is_not_an_error() {
        assert!(is_no_release("404 Not Found"));
        assert!(!is_no_release("Update check failed: timed out"));
    }
}
