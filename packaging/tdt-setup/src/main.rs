#![cfg_attr(not(test), windows_subsystem = "windows")]

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const MAGIC: &[u8; 8] = b"TDTZIP1\0";
const APP_NAME: &str = "TDT";

struct Args {
    silent: bool,
    uninstall: bool,
    skip_launch: bool,
}

fn main() {
    let args = parse_args();
    if let Err(error) = run(&args) {
        log_line(&format!("ERROR {error}"));
        if !args.silent {
            alert(&error);
        }
        std::process::exit(1);
    }
}

fn run(args: &Args) -> Result<(), String> {
    log_line("TDT Setup starting");
    if args.uninstall {
        return uninstall();
    }

    let payload = read_payload()?;
    let extract_dir = std::env::temp_dir().join(format!("TDT-payload-{}", std::process::id()));
    if extract_dir.exists() {
        fs::remove_dir_all(&extract_dir).map_err(|e| format!("Could not clear temp dir: {e}"))?;
    }
    fs::create_dir_all(&extract_dir).map_err(|e| format!("Could not create temp dir: {e}"))?;

    let zip_path = extract_dir.join("payload.zip");
    fs::write(&zip_path, payload).map_err(|e| format!("Could not write payload: {e}"))?;
    expand_zip(&zip_path, &extract_dir)?;
    let _ = fs::remove_file(&zip_path);

    let stage = find_stage(&extract_dir)?;
    let script = stage.join("Install-TDT.ps1");
    if !script.is_file() {
        return Err("Installer payload is missing Install-TDT.ps1".to_string());
    }

    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        script.to_str().ok_or("Invalid installer path")?,
        "-StageDir",
        stage.to_str().ok_or("Invalid stage path")?,
        "-CloseApplications",
    ]);
    if args.skip_launch {
        cmd.arg("-SkipLaunch");
    }
    let status = cmd
        .status()
        .map_err(|e| format!("Could not run installer script: {e}"))?;
    if !status.success() {
        return Err(format!(
            "Installer script failed with exit code {}",
            status.code().unwrap_or(1)
        ));
    }
    log_line("TDT Setup finished");
    Ok(())
}

fn uninstall() -> Result<(), String> {
    let install_dir = install_dir();
    let script = install_dir.join("uninstall-tdt.ps1");
    if !script.is_file() {
        return Err(format!("TDT is not installed at {}", install_dir.display()));
    }
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            script.to_str().ok_or("Invalid uninstall path")?,
            "-Uninstall",
        ])
        .status()
        .map_err(|e| format!("Could not run uninstaller: {e}"))?;
    if !status.success() {
        return Err("Uninstall failed".to_string());
    }
    Ok(())
}

fn install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").unwrap_or_else(|| ".".into());
    PathBuf::from(base).join(APP_NAME)
}

fn find_stage(extract_dir: &Path) -> Result<PathBuf, String> {
    if extract_dir.join("TDT.exe").is_file() {
        return Ok(extract_dir.to_path_buf());
    }
    let entries = fs::read_dir(extract_dir).map_err(|e| format!("Could not read payload: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("TDT.exe").is_file() {
            return Ok(path);
        }
    }
    Err("Installer payload is missing TDT.exe".to_string())
}

fn expand_zip(zip_path: &Path, dest: &Path) -> Result<(), String> {
    let zip = zip_path.to_str().ok_or("Invalid zip path")?;
    let dest_s = dest.to_str().ok_or("Invalid extract path")?;
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &format!(
                "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                zip.replace('\'', "''"),
                dest_s.replace('\'', "''")
            ),
        ])
        .status()
        .map_err(|e| format!("Could not extract payload: {e}"))?;
    if !status.success() {
        return Err("Could not extract installer payload".to_string());
    }
    Ok(())
}

fn read_payload() -> Result<Vec<u8>, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Could not locate installer: {e}"))?;
    let mut file = File::open(&exe).map_err(|e| format!("Could not read installer: {e}"))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .map_err(|e| format!("Could not read installer: {e}"))?;
    split_sfx(&data).map(|bytes| bytes.to_vec())
}

fn split_sfx(data: &[u8]) -> Result<&[u8], String> {
    if data.len() < 16 {
        return Err("Installer is incomplete".to_string());
    }
    let magic = &data[data.len() - 8..];
    if magic != MAGIC {
        return Err(
            "Installer payload is missing. Rebuild with packaging/build-installer.ps1.".to_string(),
        );
    }
    let size_bytes: [u8; 8] = data[data.len() - 16..data.len() - 8]
        .try_into()
        .map_err(|_| "Installer payload size is invalid".to_string())?;
    let size = u64::from_le_bytes(size_bytes) as usize;
    let start = data
        .len()
        .checked_sub(16 + size)
        .ok_or_else(|| "Installer payload size is invalid".to_string())?;
    Ok(&data[start..data.len() - 16])
}

fn parse_args() -> Args {
    let mut args = Args {
        silent: false,
        uninstall: false,
        skip_launch: false,
    };
    for raw in std::env::args().skip(1) {
        let upper = raw.to_ascii_uppercase();
        match upper.as_str() {
            "/SILENT" | "/VERYSILENT" | "/S" | "-S" | "--SILENT" => args.silent = true,
            "/UNINSTALL" | "-UNINSTALL" | "--UNINSTALL" => args.uninstall = true,
            "/NOLAUNCH" | "-NOLAUNCH" | "--NOLAUNCH" => args.skip_launch = true,
            "/CLOSEAPPLICATIONS" | "/NORESTART" | "/SP-" | "/SUPPRESSMSGBOXES" => {}
            _ => {}
        }
    }
    args
}

fn log_line(message: &str) {
    let path = std::env::temp_dir().join("TDT-setup.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{message}");
    }
}

#[cfg(windows)]
mod winmsg {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(
            hwnd: *mut core::ffi::c_void,
            text: *const u16,
            caption: *const u16,
            ty: u32,
        ) -> i32;
    }

    pub fn error(message: &str) {
        let text: Vec<u16> = OsStr::new(message)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let caption: Vec<u16> = OsStr::new("TDT Setup")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), 0x10);
        }
    }
}

fn alert(message: &str) {
    #[cfg(windows)]
    winmsg::error(message);
    #[cfg(not(windows))]
    eprintln!("{message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_sfx_reads_appended_zip() {
        let stub = b"stub-bytes";
        let zip = b"PK\x03\x04payload";
        let mut data = stub.to_vec();
        data.extend_from_slice(zip);
        data.extend_from_slice(&(zip.len() as u64).to_le_bytes());
        data.extend_from_slice(MAGIC);
        assert_eq!(split_sfx(&data).unwrap(), zip.as_slice());
    }

    #[test]
    fn split_sfx_rejects_bare_stub() {
        assert!(split_sfx(b"no-payload-here").is_err());
    }
}
