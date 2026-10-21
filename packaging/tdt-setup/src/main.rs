//! TDT-Setup.exe, the one file that a release ships.
//!
//! `packaging/build-installer.ps1` appends the app to this stub:
//!
//! `[stub][deflate(TDT.exe)][SHA-256 of TDT.exe: 32][version: 16, zero padded][deflate length: u64 LE][MAGIC]`
//!
//! Run it to install or update TDT for the current user in `%LOCALAPPDATA%\TDT`.
//! No admin rights. `/S` shows no messages; the in-app updater uses it.
//! The stub without the app is the uninstaller. Setup writes it as
//! `uninstall.exe`, and Windows' app list starts it. Uninstall keeps the
//! user's data in the install folder: the speech models and the dictionary.

#![windows_subsystem = "windows"]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use sha2::Digest;
use windows::core::{w, GUID, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, E_ACCESSDENIED};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteTreeW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_ACCESS_RIGHTS};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Programs, FOLDERID_Startup, SHChangeNotify, SHGetKnownFolderPath,
    KF_FLAG_DEFAULT, SHCNE_ASSOCCHANGED, SHCNF_IDLIST,
};

const MAGIC: &[u8; 8] = b"TDTSFX2\0";
const TRAILER: usize = 32 + 16 + 8 + 8;
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\TDT";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_RUN: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const APPROVED_FOLDER: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder";
/// The single-instance mutex that a running TDT.exe holds.
const INSTANCE_MUTEX: PCWSTR = w!("Local\\TDT-TalkDontType");
/// Files that older TDT versions left in the install folder.
const STALE_FILES: &[&str] = &[
    "Install-TDT.ps1",
    "uninstall-tdt.ps1",
    "payload-sha256.txt",
    "VERSION",
    "README.md",
    "TDT.exe.new",
    "tdt-apply-update.cmd",
    "photon_worker.py",
    "photon-python.txt",
];
/// User data in the install folder that uninstall keeps.
const USER_DATA: &[&str] = &["models", "dictionary.txt"];
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct Payload {
    stub: Vec<u8>,
    app: Option<App>,
}

struct App {
    exe: Vec<u8>,
    version: String,
}

fn main() {
    let args: Vec<String> = std::env::args()
        .skip(1)
        .map(|arg| arg.to_ascii_uppercase())
        .collect();
    let has = |names: &[&str]| args.iter().any(|arg| names.contains(&arg.as_str()));
    // /SILENT and /VERYSILENT are what TDT 0.2.32 and older pass.
    let silent = has(&["/S", "/SILENT", "/VERYSILENT"]);

    let result = read_payload().and_then(|payload| match payload.app {
        Some(app) if !has(&["/UNINSTALL"]) => install(&payload.stub, app, has(&["/NOLAUNCH"])),
        _ => uninstall(silent),
    });
    if let Err(error) = result {
        log(&format!("ERROR {error}"));
        if !silent {
            message(&error, true);
        }
        std::process::exit(1);
    }
}

fn install(stub: &[u8], app: App, no_launch: bool) -> Result<(), String> {
    log(&format!("Installing TDT {}", app.version));
    let dir = install_dir()?;
    close_tdt()?;
    fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    remove_stale_files(&dir);

    let exe = dir.join("TDT.exe");
    replace_file(&exe, &app.exe)?;
    replace_file(&dir.join("uninstall.exe"), stub)?;
    for (name, bytes) in [
        ("LICENSE", &include_bytes!("../../../LICENSE")[..]),
        ("NOTICE", &include_bytes!("../../../NOTICE")[..]),
        (
            "THIRD_PARTY_NOTICES.md",
            &include_bytes!("../../../THIRD_PARTY_NOTICES.md")[..],
        ),
    ] {
        replace_file(&dir.join(name), bytes)?;
    }

    let programs = known_folder(&FOLDERID_Programs)?.join("TDT");
    fs::create_dir_all(&programs)
        .map_err(|e| format!("Could not create the Start menu folder: {e}"))?;
    write_shortcut(&exe, &programs.join("TDT.lnk"))?;
    register(&dir, &app.version, app.exe.len())?;
    log(&format!(
        "Installed TDT {} to {}",
        app.version,
        dir.display()
    ));

    if !no_launch {
        Command::new(&exe)
            .current_dir(&dir)
            .spawn()
            .map_err(|e| format!("TDT is installed, but it did not start: {e}"))?;
    }
    Ok(())
}

fn uninstall(silent: bool) -> Result<(), String> {
    let dir = install_dir()?;
    close_tdt()?;

    if let Ok(programs) = known_folder(&FOLDERID_Programs) {
        let _ = fs::remove_dir_all(programs.join("TDT"));
    }
    for folder in [&FOLDERID_Startup, &FOLDERID_Desktop] {
        if let Ok(folder) = known_folder(folder) {
            let _ = fs::remove_file(folder.join("TDT.lnk"));
        }
    }
    for (key, value) in [
        (RUN_KEY, "TDT"),
        (APPROVED_RUN, "TDT"),
        (APPROVED_FOLDER, "TDT.lnk"),
    ] {
        unsafe {
            let _ = RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(key),
                &HSTRING::from(value),
            );
        }
    }
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(UNINSTALL_KEY));
    }

    let me = std::env::current_exe().ok();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let keep = USER_DATA.iter().any(|name| entry.file_name() == *name);
            if keep || me.as_deref().is_some_and(|me| same_file(me, &path)) {
                continue;
            }
            let _ = if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
        }
    }
    // A running exe cannot delete itself, so cmd removes it after this
    // process exits. rmdir leaves the folder when user data is still in it.
    if let Some(me) = me.filter(|me| me.starts_with(&dir)) {
        let _ = Command::new("cmd.exe")
            .raw_arg(format!(
                "/C ping -n 3 127.0.0.1 >nul & del /f /q \"{}\" & rmdir \"{}\"",
                me.display(),
                dir.display()
            ))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    log("TDT removed");

    if !silent {
        let note = if USER_DATA.iter().any(|name| dir.join(name).exists()) {
            format!(
                "TDT was removed.\n\nYour speech models and dictionary stay in {}. Delete that folder to free the space.",
                dir.display()
            )
        } else {
            "TDT was removed.".to_string()
        };
        message(&note, false);
    }
    Ok(())
}

fn read_payload() -> Result<Payload, String> {
    let me = std::env::current_exe().map_err(|e| format!("Could not locate setup: {e}"))?;
    let data = fs::read(&me).map_err(|e| format!("Could not read setup: {e}"))?;
    if data.len() < TRAILER || &data[data.len() - 8..] != MAGIC {
        return Ok(Payload {
            stub: data,
            app: None,
        });
    }
    let trailer = &data[data.len() - TRAILER..];
    let expected_hash = &trailer[..32];
    let version = String::from_utf8_lossy(&trailer[32..48])
        .trim_end_matches('\0')
        .to_string();
    let packed_len = u64::from_le_bytes(trailer[48..56].try_into().unwrap_or_default()) as usize;
    let start = (data.len() - TRAILER)
        .checked_sub(packed_len)
        .ok_or("Setup is damaged. Download TDT-Setup.exe again.")?;
    let exe = miniz_oxide::inflate::decompress_to_vec(&data[start..data.len() - TRAILER])
        .map_err(|_| "Setup is damaged. Download TDT-Setup.exe again.")?;
    if sha2::Sha256::digest(&exe).as_slice() != expected_hash {
        return Err("Setup failed its integrity check. Download TDT-Setup.exe again.".into());
    }
    let mut stub = data;
    stub.truncate(start);
    Ok(Payload {
        stub,
        app: Some(App { exe, version }),
    })
}

/// Waits for a running TDT to quit, then ends it. The in-app updater quits
/// by itself right after it starts setup.
fn close_tdt() -> Result<(), String> {
    let running = || unsafe {
        match OpenMutexW(
            SYNCHRONIZATION_ACCESS_RIGHTS(0x0010_0000),
            false,
            INSTANCE_MUTEX,
        ) {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                true
            }
            // A TDT that runs as administrator holds a mutex that this
            // process may not open.
            Err(error) => error.code() == E_ACCESSDENIED,
        }
    };
    let wait = |limit: Duration| {
        let start = Instant::now();
        while running() && start.elapsed() < limit {
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    wait(Duration::from_secs(5));
    if running() {
        log("Ending the running TDT");
        let _ = Command::new("taskkill.exe")
            .args(["/F", "/IM", "TDT.exe"])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        wait(Duration::from_secs(5));
    }
    if running() {
        return Err(
            "TDT is still running, possibly as administrator. Quit TDT from its tray icon, then run setup again."
                .into(),
        );
    }
    Ok(())
}

/// Writes a fresh file. Windows lets a locked exe be renamed but not
/// replaced, so a locked file moves aside first; the next setup removes it.
fn replace_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.exists() && fs::remove_file(path).is_err() {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let aside = path.with_file_name(format!("{name}.old-{}", std::process::id()));
        fs::rename(path, aside).map_err(|e| format!("Could not replace {name}: {e}"))?;
    }
    fs::write(path, bytes).map_err(|e| format!("Could not write {}: {e}", path.display()))
}

fn remove_stale_files(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.contains(".old") || STALE_FILES.contains(&name.as_str()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn register(dir: &Path, version: &str, exe_len: usize) -> Result<(), String> {
    let uninstaller = dir.join("uninstall.exe");
    let exe = dir.join("TDT.exe");
    let text = [
        ("DisplayName", "TDT - Talk Don't Type".to_string()),
        ("DisplayVersion", version.to_string()),
        ("Publisher", "Hi9841".to_string()),
        ("InstallLocation", dir.display().to_string()),
        ("DisplayIcon", exe.display().to_string()),
        ("URLInfoAbout", "https://github.com/Hi9841/tdt".to_string()),
        (
            "UninstallString",
            format!("\"{}\" /UNINSTALL", uninstaller.display()),
        ),
        (
            "QuietUninstallString",
            format!("\"{}\" /UNINSTALL /S", uninstaller.display()),
        ),
    ];
    let numbers = [
        ("NoModify", 1u32),
        ("NoRepair", 1),
        ("EstimatedSize", (exe_len / 1024) as u32),
    ];
    unsafe {
        let mut key = HKEY::default();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(UNINSTALL_KEY),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        );
        if status.is_err() {
            return Err(format!("Could not register TDT with Windows: {status:?}"));
        }
        for (name, value) in text {
            let wide: Vec<u8> = value
                .encode_utf16()
                .chain(Some(0))
                .flat_map(u16::to_le_bytes)
                .collect();
            let _ = RegSetValueExW(key, &HSTRING::from(name), 0, REG_SZ, Some(&wide));
        }
        for (name, value) in numbers {
            let _ = RegSetValueExW(
                key,
                &HSTRING::from(name),
                0,
                REG_DWORD,
                Some(&value.to_le_bytes()),
            );
        }
        let _ = RegCloseKey(key);
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
    Ok(())
}

fn write_shortcut(target: &Path, link_path: &Path) -> Result<(), String> {
    use windows::core::Interface;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    let fail = |e: windows::core::Error| format!("Could not create the Start menu shortcut: {e}");
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(fail)?;
        let target_hs = HSTRING::from(target.as_os_str());
        link.SetPath(&target_hs).map_err(fail)?;
        if let Some(dir) = target.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))
                .map_err(fail)?;
        }
        link.SetDescription(w!("TDT - Talk Don't Type"))
            .map_err(fail)?;
        link.SetIconLocation(&target_hs, 0).map_err(fail)?;
        let file: IPersistFile = link.cast().map_err(fail)?;
        file.Save(&HSTRING::from(link_path.as_os_str()), true)
            .map_err(fail)?;
    }
    Ok(())
}

fn install_dir() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(|base| PathBuf::from(base).join("TDT"))
        .ok_or_else(|| "LOCALAPPDATA is not set".to_string())
}

fn known_folder(id: &GUID) -> Result<PathBuf, String> {
    unsafe {
        let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None)
            .map_err(|e| format!("Could not find a Windows folder: {e}"))?;
        let text = path.to_string();
        windows::Win32::System::Com::CoTaskMemFree(Some(path.as_ptr() as *const _));
        text.map(PathBuf::from)
            .map_err(|_| "Could not read a Windows folder path".to_string())
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn log(line: &str) {
    let path = std::env::temp_dir().join("TDT-setup.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

fn message(text: &str, error: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION};
    let style = if error {
        MB_ICONERROR
    } else {
        MB_ICONINFORMATION
    };
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), w!("TDT Setup"), style);
    }
}
