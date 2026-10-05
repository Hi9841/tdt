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
        asset_name: String,
        sums_url: Option<String>,
    },
    Downloading {
        done: u64,
        total: u64,
    },
    Ready {
        installer: PathBuf,
    },
    Failed(String),
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
    let url = format!("https://github.com/{repo}/releases/latest");
    let response = check_redirect_agent()
        .get(&url)
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => "404 Not Found".to_string(),
            other => clean_error_message(&format!("{other}")),
        })?;

    let location = match response.header("location") {
        Some(loc) => loc,
        None => return Ok(UpdatePhase::UpToDate),
    };

    let tag = match parse_tag_from_location(location) {
        Some(t) => t,
        None => return Ok(UpdatePhase::UpToDate),
    };

    let latest = tag.trim().trim_start_matches('v').to_string();
    if !version_newer(&latest, current_version()) {
        return Ok(UpdatePhase::UpToDate);
    }

    // Verify SHA256SUMS.txt exists before declaring the update available
    let sums_url = format!("https://github.com/{repo}/releases/download/{tag}/SHA256SUMS.txt");
    let sums_body = match http_get_string(&sums_url) {
        Ok(body) => body,
        Err(err) if is_no_release(&err) => {
            return Ok(UpdatePhase::UpToDate);
        }
        Err(err) => return Err(clean_error_message(&err)),
    };

    let asset_name = if expected_hash_for(&sums_body, "TDT.exe").is_some() {
        "TDT.exe".to_string()
    } else if expected_hash_for(&sums_body, "TDT-Setup.exe").is_some() {
        "TDT-Setup.exe".to_string()
    } else {
        return Ok(UpdatePhase::UpToDate);
    };

    let asset_url = format!("https://github.com/{repo}/releases/download/{tag}/{asset_name}");
    Ok(UpdatePhase::Available {
        version: latest,
        asset_url,
        asset_name,
        sums_url: Some(sums_url),
    })
}

/// Download a release binary, streaming to a temp file, and verify SHA256
/// against the published `SHA256SUMS.txt`. Prefers the slim `TDT.exe` app
/// binary (Prism-style in-place replace) over the full setup SFX.
pub fn download_installer(
    asset_url: &str,
    asset_name: &str,
    sums_url: Option<&str>,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    let Some(sums_url) = sums_url else {
        return Err(
            "Release has no SHA256SUMS.txt; refusing to run an unverified installer".to_string(),
        );
    };
    let sums_url = sums_url.to_string();
    let sums_job = std::thread::spawn(move || http_get_string(&sums_url));

    let file_name = if is_app_binary(asset_name) {
        "TDT.exe"
    } else {
        "TDT-Setup.exe"
    };
    let path = std::env::temp_dir().join(format!("{file_name}.download"));
    let mut file = File::create(&path).map_err(|e| format!("Could not write update: {e}"))?;
    let mut hasher = sha2::Sha256::new();

    let response = download_agent()
        .get(asset_url)
        .set("Accept", "application/octet-stream")
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;
    let content_len = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    on_progress(0, content_len.max(1));
    let mut reader = response.into_reader();
    let mut chunk = [0u8; 256 * 1024];
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
            .map_err(|e| format!("Could not write update: {e}"))?;
        total += read as u64;
        on_progress(total, content_len.max(total).max(1));
    }
    drop(file);

    if total < 64 {
        let _ = std::fs::remove_file(&path);
        return Err("Downloaded update was empty".to_string());
    }

    let mut head = [0u8; 2];
    File::open(&path)
        .and_then(|mut f| f.read_exact(&mut head))
        .map_err(|e| format!("Could not read update: {e}"))?;
    if !looks_like_pe(&head) {
        let _ = std::fs::remove_file(&path);
        return Err("Downloaded file is not a Windows executable".to_string());
    }

    let sums = match sums_job.join() {
        Ok(Ok(body)) => body,
        Ok(Err(error)) => {
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            return Err("Could not read release checksums".to_string());
        }
    };
    let expected = expected_hash_for(&sums, file_name)
        .or_else(|| expected_hash_for(&sums, asset_name))
        .or_else(|| expected_hash_for(&sums, &download_file_name(asset_url)));
    match expected {
        Some(expected) => {
            let actual = format!("{:x}", hasher.finalize());
            if actual != expected {
                let _ = std::fs::remove_file(&path);
                return Err(
                    "Downloaded update failed SHA256 verification; update aborted".to_string(),
                );
            }
        }
        None => {
            let _ = std::fs::remove_file(&path);
            return Err("Release checksums do not list the update; update aborted".to_string());
        }
    }

    let final_path = std::env::temp_dir().join(file_name);
    if let Err(error) = std::fs::rename(&path, &final_path) {
        std::fs::copy(&path, &final_path).map_err(|e| {
            format!("Could not finalize update download: {error}; copy failed: {e}")
        })?;
        let _ = std::fs::remove_file(&path);
    }
    Ok(final_path)
}

fn download_file_name(url: &str) -> String {
    url.rsplit('/').next().unwrap_or_default().to_string()
}

fn expected_hash_for(sums: &str, file_name: &str) -> Option<String> {
    for line in sums.lines() {
        let mut parts = line.split_whitespace();
        let (Some(hash), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
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

/// True when the release asset is the slim app binary, not the setup SFX.
pub fn is_app_binary(name: &str) -> bool {
    Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|file| file.eq_ignore_ascii_case("TDT.exe"))
}

/// Replace the running TDT.exe and relaunch, same idea as Prism's NSIS
/// updater: swap the binary, do not re-download models.
pub fn replace_running_exe(new_exe: &Path, version: &str) -> Result<(), String> {
    let current =
        std::env::current_exe().map_err(|error| format!("Could not locate TDT.exe: {error}"))?;
    let dir = current
        .parent()
        .ok_or_else(|| "Could not locate the TDT folder".to_string())?;
    let staged = dir.join("TDT.exe.new");
    std::fs::copy(new_exe, &staged)
        .map_err(|error| format!("Could not stage the update: {error}"))?;
    let _ = std::fs::write(dir.join("VERSION"), version.trim().trim_start_matches('v'));

    let script = dir.join("tdt-apply-update.cmd");
    let staged_s = staged.display().to_string().replace('"', "");
    let current_s = current.display().to_string().replace('"', "");
    let body = format!(
        "@echo off\r\n:retry\r\nping -n 2 127.0.0.1 >nul\r\nmove /Y \"{staged_s}\" \"{current_s}\"\r\nif exist \"{staged_s}\" goto retry\r\nstart \"\" \"{current_s}\"\r\ndel \"%~f0\"\r\n"
    );
    std::fs::write(&script, body)
        .map_err(|error| format!("Could not write the update script: {error}"))?;

    let mut cmd = Command::new("cmd.exe");
    cmd.args(["/C", &script.to_string_lossy()]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    cmd.spawn()
        .map_err(|error| format!("Could not start the update script: {error}"))?;
    Ok(())
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

fn check_redirect_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(15))
        .timeout_write(Duration::from_secs(15))
        .redirects(0)
        .user_agent(&user_agent())
        .build()
}

pub fn parse_tag_from_location(location: &str) -> Option<&str> {
    let marker = "/releases/tag/";
    let index = location.find(marker)?;
    let rest = &location[index + marker.len()..];
    let tag = rest.split(['/', '?', '#']).next()?;
    if tag.is_empty() {
        None
    } else {
        Some(tag)
    }
}

pub fn clean_error_message(error: &str) -> String {
    if error.contains("timed out")
        || error.contains("Connection refused")
        || error.contains("dns")
        || error.contains("Could not reach")
        || error.contains("network")
    {
        "Could not reach GitHub; check your connection.".to_string()
    } else {
        "Could not check for updates.".to_string()
    }
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
        .timeout_read(Duration::from_secs(7200))
        .timeout_write(Duration::from_secs(60))
        .user_agent(&user_agent())
        .build()
}

fn http_get_string(url: &str) -> Result<String, String> {
    match check_agent().get(url).call() {
        Ok(response) => response
            .into_string()
            .map_err(|e| format!("Read failed: {e}")),
        Err(ureq::Error::Status(404, _)) => Err("404 Not Found".to_string()),
        Err(error) => Err(format!("{error}")),
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
    fn sums_parsing_skips_malformed_lines() {
        let sums = "\nnot-a-hash\ndef456  tdt-setup.exe\n\nfeed99  TDT.exe\n";
        assert_eq!(
            expected_hash_for(sums, "TDT.exe"),
            Some("feed99".to_string())
        );
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

    #[test]
    fn parse_tag_from_location_extracts_tag() {
        assert_eq!(
            parse_tag_from_location("https://github.com/Hi9841/tdt/releases/tag/v0.2.16"),
            Some("v0.2.16")
        );
        assert_eq!(
            parse_tag_from_location("/releases/tag/0.2.17"),
            Some("0.2.17")
        );
        assert_eq!(
            parse_tag_from_location(
                "https://github.com/Hi9841/tdt/releases/tag/v0.2.17?foo=bar#hash"
            ),
            Some("v0.2.17")
        );
        assert_eq!(
            parse_tag_from_location("https://github.com/Hi9841/tdt"),
            None
        );
        assert_eq!(parse_tag_from_location("/releases/tag/"), None);
    }

    #[test]
    fn clean_error_message_is_user_friendly() {
        let net_err = clean_error_message("Connection timed out after 30s");
        assert_eq!(net_err, "Could not reach GitHub; check your connection.");

        let other = clean_error_message("something weird");
        assert_eq!(other, "Could not check for updates.");
    }

    #[test]
    #[ignore = "requires live internet connection"]
    fn check_latest_does_not_fail_on_rate_limit() {
        let res = check_latest();
        assert!(res.is_ok(), "check_latest should succeed: {:?}", res);
    }
}
