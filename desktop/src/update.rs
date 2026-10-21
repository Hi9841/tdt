//! In-app updates, the same way as Prism: read the latest GitHub release,
//! download its `TDT-Setup.exe`, verify it, start it silently, and quit.
//! Setup replaces TDT.exe and starts the new version. Speech models stay.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use parking_lot::Mutex;
use sha2::Digest;

/// Fixed at compile time, so a release build cannot be redirected at runtime.
const REPO: &str = match option_env!("TDT_UPDATE_REPO") {
    Some(repo) => repo,
    None => "Hi9841/tdt",
};
const ASSET: &str = "TDT-Setup.exe";
/// Automatic checks (start, every hour, panel open) run at most this often.
const AUTO_CHECK_EVERY: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq)]
pub enum UpdatePhase {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading {
        done: u64,
        total: u64,
    },
    /// Setup is running. TDT quits so that setup can replace it.
    Installing,
    Failed(String),
}

impl UpdatePhase {
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Installing
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: String,
    url: String,
    size: u64,
    sha256: Option<String>,
}

impl Release {
    /// A frozen release for the preview captures.
    pub fn preview(version: &str) -> Self {
        Self {
            version: version.into(),
            url: String::new(),
            size: 7_900_000,
            sha256: None,
        }
    }
}

#[derive(serde::Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(serde::Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    /// `sha256:<hex>`, which GitHub computes for each uploaded asset.
    digest: Option<String>,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn updates_disabled() -> bool {
    matches!(
        std::env::var("TDT_DISABLE_UPDATES")
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref(),
        Ok("1" | "true" | "yes" | "on")
    )
}

/// Starts one check. The phase changes on the calling thread, so a second
/// call finds it busy; the network work runs on a new thread. A manual
/// check always runs and shows its errors. An automatic check runs only when
/// the last one is `AUTO_CHECK_EVERY` old, and a failure keeps the previous
/// state.
pub fn check(cell: &Arc<Mutex<UpdatePhase>>, manual: bool, ping: &Sender<()>) {
    if updates_disabled() {
        return;
    }
    let previous = {
        let mut phase = cell.lock();
        if phase.is_busy() || !claim_check_slot(manual) {
            return;
        }
        std::mem::replace(&mut *phase, UpdatePhase::Checking)
    };
    let _ = ping.send(());
    let (cell, ping) = (Arc::clone(cell), ping.clone());
    std::thread::spawn(move || {
        let next = match latest_release() {
            Ok(Some(release)) => UpdatePhase::Available(release),
            Ok(None) => UpdatePhase::UpToDate,
            Err(error) if manual => UpdatePhase::Failed(error),
            Err(error) => {
                eprintln!("TDT: update check failed: {error}");
                previous
            }
        };
        *cell.lock() = next;
        let _ = ping.send(());
    });
}

/// Downloads the available release on a new thread and starts its setup.
/// On success the phase becomes `Installing`, and the main loop quits TDT.
pub fn install(cell: &Arc<Mutex<UpdatePhase>>, ping: &Sender<()>) {
    let release = {
        let mut phase = cell.lock();
        let UpdatePhase::Available(release) = &*phase else {
            return;
        };
        let release = release.clone();
        *phase = UpdatePhase::Downloading {
            done: 0,
            total: release.size,
        };
        release
    };
    let _ = ping.send(());
    let (cell, ping) = (Arc::clone(cell), ping.clone());
    std::thread::spawn(move || {
        let mut last_ping = Instant::now();
        let result = download(&release, |done| {
            *cell.lock() = UpdatePhase::Downloading {
                done,
                total: release.size,
            };
            if last_ping.elapsed() >= Duration::from_millis(100) {
                let _ = ping.send(());
                last_ping = Instant::now();
            }
        })
        .and_then(|setup| {
            std::process::Command::new(&setup)
                .arg("/S")
                .spawn()
                .map(drop)
                .map_err(|e| format!("Could not start the update: {e}"))
        });
        *cell.lock() = match result {
            Ok(()) => UpdatePhase::Installing,
            Err(error) => UpdatePhase::Failed(error),
        };
        let _ = ping.send(());
    });
}

fn claim_check_slot(manual: bool) -> bool {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock();
    if !manual && last.is_some_and(|at| at.elapsed() < AUTO_CHECK_EVERY) {
        return false;
    }
    *last = Some(Instant::now());
    true
}

fn latest_release() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let response = match agent(Duration::from_secs(15))
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(response) => response,
        // No published release yet.
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(ureq::Error::Status(403 | 429, _)) => {
            return Err("GitHub is busy. Try again in a few minutes.".into())
        }
        Err(_) => return Err("Could not reach GitHub. Check your connection.".into()),
    };
    let release: GitHubRelease = response
        .into_json()
        .map_err(|_| "GitHub sent an unexpected answer.".to_string())?;
    let version = release.tag_name.trim().trim_start_matches(['v', 'V']);
    if !is_newer(version, current_version()) {
        return Ok(None);
    }
    // A release that is still uploading has no setup yet.
    let Some(asset) = release.assets.into_iter().find(|a| a.name == ASSET) else {
        return Ok(None);
    };
    Ok(Some(Release {
        version: version.to_string(),
        url: asset.browser_download_url,
        size: asset.size,
        sha256: asset
            .digest
            .and_then(|d| d.strip_prefix("sha256:").map(str::to_ascii_lowercase)),
    }))
}

fn download(release: &Release, mut on_progress: impl FnMut(u64)) -> Result<PathBuf, String> {
    let failed = |e: &dyn std::fmt::Display| format!("Download failed: {e}");
    let path = std::env::temp_dir().join(format!("TDT-Setup-{}.exe", release.version));
    let response = agent(Duration::from_secs(10 * 60))
        .get(&release.url)
        .set("Accept", "application/octet-stream")
        .call()
        .map_err(|e| failed(&e))?;
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&path).map_err(|e| failed(&e))?;
    let mut hasher = sha2::Sha256::new();
    let mut chunk = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    let mut head = [0u8; 2];
    loop {
        let read = reader.read(&mut chunk).map_err(|e| failed(&e))?;
        if read == 0 {
            break;
        }
        if done == 0 && read >= 2 {
            head.copy_from_slice(&chunk[..2]);
        }
        hasher.update(&chunk[..read]);
        file.write_all(&chunk[..read]).map_err(|e| failed(&e))?;
        done += read as u64;
        on_progress(done);
    }
    drop(file);

    let hash = format!("{:x}", hasher.finalize());
    let problem = if done != release.size {
        Some("The download is incomplete. Try again.")
    } else if release
        .sha256
        .as_ref()
        .is_some_and(|expected| *expected != hash)
    {
        Some("The download failed its integrity check. Try again.")
    } else if &head != b"MZ" {
        Some("The download is not a Windows program.")
    } else {
        None
    };
    if let Some(problem) = problem {
        let _ = std::fs::remove_file(&path);
        return Err(problem.into());
    }
    Ok(path)
}

fn is_newer(latest: &str, current: &str) -> bool {
    fn parse(raw: &str) -> Option<(u64, u64, u64)> {
        let mut parts = raw.split(['.', '-', '+']);
        Some((
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next().unwrap_or("0").parse().ok()?,
        ))
    }
    matches!((parse(latest), parse(current)), (Some(l), Some(c)) if l > c)
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(timeout)
        .user_agent(&format!(
            "TDT/{} (+https://github.com/{REPO})",
            current_version()
        ))
        .build()
}

/// Deletes the setup that an earlier update downloaded.
pub fn remove_downloaded_setups() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("TDT-Setup-") && name.ends_with(".exe") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
