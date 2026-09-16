use serde::Deserialize;
use sha2::Digest;
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
    Available {
        version: String,
        asset_url: String,
        sums_url: Option<String>,
    },
    Downloading,
    Ready {
        installer: PathBuf,
    },
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

static UPDATE_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Start a new update-check generation and return its ticket. Any thread
/// holding a superseded ticket must drop its result instead of writing it,
/// so a slow boot-time check can never overwrite a newer user-initiated one.
pub fn begin_update_check() -> u64 {
    UPDATE_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
}

/// True when the caller's ticket is still the newest update-check generation.
pub fn update_check_is_current(ticket: u64) -> bool {
    UPDATE_EPOCH.load(std::sync::atomic::Ordering::SeqCst) == ticket
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
    let sums_url = release
        .assets
        .iter()
        .find(|asset| asset.name.eq_ignore_ascii_case("SHA256SUMS.txt"))
        .map(|asset| asset.browser_download_url.clone());
    Ok(UpdatePhase::Available {
        version: latest,
        asset_url: asset.browser_download_url.clone(),
        sums_url,
    })
}

/// Download the setup installer, streaming to a temp file, and verify its
/// SHA256 against the release's published `SHA256SUMS.txt` when available.
/// Returns the verified installer path.
pub fn download_installer(asset_url: &str, sums_url: Option<&str>) -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join("TDT-Setup.exe.download");
    let mut file = File::create(&path).map_err(|e| format!("Could not write installer: {e}"))?;
    let mut hasher = sha2::Sha256::new();

    let response = download_agent()
        .get(asset_url)
        .set("Accept", "application/octet-stream")
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;
    let mut reader = response.into_reader();
    let mut chunk = [0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let read = reader
            .read(&mut chunk)
            .map_err(|e| format!("Download failed: {e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
        file.write_all(&chunk[..read])
            .map_err(|e| format!("Could not write installer: {e}"))?;
        total += read as u64;
    }
    drop(file);

    if total < 64 {
        let _ = std::fs::remove_file(&path);
        return Err("Downloaded installer was empty".to_string());
    }

    let mut head = [0u8; 2];
    File::open(&path)
        .and_then(|mut f| f.read_exact(&mut head))
        .map_err(|e| format!("Could not read installer: {e}"))?;
    if !looks_like_pe(&head) {
        let _ = std::fs::remove_file(&path);
        return Err("Downloaded file is not a Windows installer".to_string());
    }

    if let Some(sums_url) = sums_url {
        let sums = http_get_string(sums_url)?;
        let expected = expected_hash_for(&sums, "TDT-Setup.exe")
            .or_else(|| expected_hash_for(&sums, &download_file_name(asset_url)));
        match expected {
            Some(expected) => {
                let actual = format!("{:x}", hasher.finalize());
                if actual != expected {
                    let _ = std::fs::remove_file(&path);
                    return Err(
                        "Downloaded installer failed SHA256 verification; update aborted"
                            .to_string(),
                    );
                }
            }
            None => {
                // Sums file exists but lists no installer entry; do not run
                // an unverifiable executable.
                let _ = std::fs::remove_file(&path);
                return Err(
                    "Release checksums do not list the installer; update aborted".to_string(),
                );
            }
        }
    } else {
        let _ = std::fs::remove_file(&path);
        return Err(
            "Release has no SHA256SUMS.txt; refusing to run an unverified installer".to_string(),
        );
    }

    let final_path = std::env::temp_dir().join("TDT-Setup.exe");
    std::fs::rename(&path, &final_path)
        .map_err(|e| format!("Could not finalize installer download: {e}"))?;
    Ok(final_path)
}

fn download_file_name(url: &str) -> String {
    url.rsplit('/').next().unwrap_or_default().to_string()
}

fn expected_hash_for(sums: &str, file_name: &str) -> Option<String> {
    for line in sums.lines() {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?;
        if name.eq_ignore_ascii_case(file_name) {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
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
    fn sums_parsing_matches_name_case_insensitively() {
        let sums = "abc123  TDT-0.1.0-windows-x64.zip\ndef456  tdt-setup.exe\n";
        assert_eq!(
            expected_hash_for(sums, "TDT-Setup.exe"),
            Some("def456".to_string())
        );
        assert_eq!(
            expected_hash_for(sums, "TDT-0.1.0-windows-x64.zip"),
            Some("abc123".to_string())
        );
        assert_eq!(expected_hash_for(sums, "missing.zip"), None);
    }

    #[test]
    fn missing_release_is_not_an_error() {
        assert!(is_no_release("404 Not Found"));
        assert!(!is_no_release("Update check failed: timed out"));
    }

    #[test]
    fn superseded_check_ticket_is_not_current() {
        let first = begin_update_check();
        assert!(update_check_is_current(first));
        let second = begin_update_check();
        assert!(!update_check_is_current(first));
        assert!(update_check_is_current(second));
    }
}
