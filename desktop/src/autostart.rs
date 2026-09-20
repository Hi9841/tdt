//! Per-user "start with Windows".
//!
//! Task Manager's Startup list is filled from the user Startup folder and the
//! HKCU Run key. The toggle writes a Startup-folder shortcut (working
//! directory set to the exe folder) and enables the matching StartupApproved
//! value so Windows does not keep a leftover Disabled state. If the shortcut
//! cannot be created, it falls back to the Run key. Never both, so logon
//! does not launch TDT twice.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, TRUE};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_BINARY, REG_OPTION_NON_VOLATILE,
    REG_SZ,
};
use windows::Win32::UI::Shell::{
    IShellLinkW, SHChangeNotify, ShellLink, SHCNE_CREATE, SHCNE_DELETE, SHCNE_UPDATEDIR,
    SHCNF_PATHW,
};

const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APPROVED_RUN: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run";
const APPROVED_FOLDER: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\StartupFolder";
const VALUE_NAME: &str = "TDT";
const SHORTCUT_NAME: &str = "TDT.lnk";
/// Windows 8+ enabled blob: 0x02, then 11 zero bytes.
const APPROVED_ENABLED: [u8; 12] = [0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// True when TDT will launch at sign-in and Task Manager does not have it Disabled.
pub fn is_enabled() -> bool {
    let shortcut = shortcut_path();
    if shortcut.exists() && !is_approved_disabled(APPROVED_FOLDER, SHORTCUT_NAME) {
        return true;
    }
    read_value_exists(RUN_SUBKEY, VALUE_NAME) && !is_approved_disabled(APPROVED_RUN, VALUE_NAME)
}

/// Keep the Startup shortcut aimed at this TDT.exe after an update or move.
pub fn refresh_if_enabled() {
    if !is_enabled() {
        return;
    }
    let Ok(exe) = current_tdt_exe() else {
        return;
    };
    if !is_tdt_exe_name(&exe) {
        return;
    }
    let _ = enable_startup();
}

fn is_tdt_exe_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("TDT.exe"))
}

/// Register or unregister TDT as a per-user startup app.
pub fn set_enabled(enable: bool) -> Result<(), String> {
    if enable {
        enable_startup()
    } else {
        disable_startup()
    }
}

fn enable_startup() -> Result<(), String> {
    let exe = current_tdt_exe()?;
    match write_shortcut(&exe, &shortcut_path()) {
        Ok(()) => {
            let _ = delete_value(RUN_SUBKEY, VALUE_NAME);
            let _ = delete_value(APPROVED_RUN, VALUE_NAME);
            write_approved(APPROVED_FOLDER, SHORTCUT_NAME)?;
            Ok(())
        }
        Err(shortcut_error) => match write_run_key(&exe) {
            Ok(()) => {
                write_approved(APPROVED_RUN, VALUE_NAME)?;
                Ok(())
            }
            Err(run_error) => Err(format!("{shortcut_error}. {run_error}")),
        },
    }
}

fn disable_startup() -> Result<(), String> {
    let mut errors = Vec::new();
    if let Err(error) = remove_shortcut(&shortcut_path()) {
        errors.push(error);
    }
    if let Err(error) = delete_value(APPROVED_FOLDER, SHORTCUT_NAME) {
        errors.push(error);
    }
    if let Err(error) = delete_value(RUN_SUBKEY, VALUE_NAME) {
        errors.push(error);
    }
    if let Err(error) = delete_value(APPROVED_RUN, VALUE_NAME) {
        errors.push(error);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join(". "))
    }
}

fn current_tdt_exe() -> Result<PathBuf, String> {
    let exe =
        std::env::current_exe().map_err(|error| format!("Could not locate TDT.exe: {error}"))?;
    Ok(win32_launch_path(&exe))
}

/// Explorer and the Run key reject `\\?\` verbatim paths from `canonicalize()`.
fn win32_launch_path(path: &Path) -> PathBuf {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let rendered = canonical.to_string_lossy();
    if let Some(rest) = rendered.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = rendered.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        canonical
    }
}

fn shortcut_path() -> PathBuf {
    startup_folder().join(SHORTCUT_NAME)
}

fn startup_folder() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Startup");
    }
    PathBuf::from(".")
}

fn write_shortcut(exe: &Path, path: &Path) -> Result<(), String> {
    let exe = exe.to_path_buf();
    let path = path.to_path_buf();
    std::thread::spawn(move || write_shortcut_sta(&exe, &path))
        .join()
        .unwrap_or_else(|_| Err("Could not create the startup shortcut.".into()))
}

fn write_shortcut_sta(exe: &Path, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create the Startup folder: {error}"))?;
    }
    let workdir = exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| format!("Could not create a startup shortcut: {error}"))?;
        let exe_hs = HSTRING::from(exe.to_string_lossy().as_ref());
        link.SetPath(&exe_hs)
            .map_err(|error| format!("Could not set the startup target: {error}"))?;
        link.SetWorkingDirectory(&HSTRING::from(workdir.to_string_lossy().as_ref()))
            .map_err(|error| format!("Could not set the startup folder: {error}"))?;
        link.SetDescription(&HSTRING::from("TDT - Talk Don't Type"))
            .map_err(|error| format!("Could not set the startup description: {error}"))?;
        link.SetIconLocation(&exe_hs, 0)
            .map_err(|error| format!("Could not set the startup icon: {error}"))?;
        let persist: IPersistFile = link
            .cast()
            .map_err(|error| format!("Could not save the startup shortcut: {error}"))?;
        persist
            .Save(&HSTRING::from(path.to_string_lossy().as_ref()), TRUE)
            .map_err(|error| format!("Could not write the Startup shortcut: {error}"))?;
    }
    notify_path(path, SHCNE_CREATE);
    Ok(())
}

fn remove_shortcut(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            notify_path(path, SHCNE_DELETE);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not remove the Startup shortcut: {error}")),
    }
}

fn notify_path(path: &Path, event: windows::Win32::UI::Shell::SHCNE_ID) {
    let wide = wide(&path.to_string_lossy());
    unsafe {
        SHChangeNotify(event, SHCNF_PATHW, Some(wide.as_ptr() as *const _), None);
        if let Some(parent) = path.parent() {
            let folder = wide_owned(&parent.to_string_lossy());
            SHChangeNotify(
                SHCNE_UPDATEDIR,
                SHCNF_PATHW,
                Some(folder.as_ptr() as *const _),
                None,
            );
        }
    }
}

fn write_run_key(exe: &Path) -> Result<(), String> {
    let data = startup_command(exe);
    unsafe {
        let mut key = HKEY::default();
        let opened = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(wide(RUN_SUBKEY).as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        );
        if opened != ERROR_SUCCESS {
            return Err(format!(
                "Could not open the Run registry key (error {opened:?})"
            ));
        }
        let written = RegSetValueExW(
            key,
            PCWSTR::from_raw(wide(VALUE_NAME).as_ptr()),
            0,
            REG_SZ,
            Some(&data),
        );
        let _ = RegCloseKey(key);
        if written != ERROR_SUCCESS {
            return Err(format!(
                "Could not write the Run registry value (error {written:?})"
            ));
        }
    }
    Ok(())
}

fn write_approved(subkey: &str, value: &str) -> Result<(), String> {
    unsafe {
        let mut key = HKEY::default();
        let opened = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(wide(subkey).as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        );
        if opened != ERROR_SUCCESS {
            return Err(format!(
                "Could not open startup approval settings (error {opened:?})"
            ));
        }
        let written = RegSetValueExW(
            key,
            PCWSTR::from_raw(wide(value).as_ptr()),
            0,
            REG_BINARY,
            Some(&APPROVED_ENABLED),
        );
        let _ = RegCloseKey(key);
        if written != ERROR_SUCCESS {
            return Err(format!(
                "Could not enable TDT in Task Manager startup (error {written:?})"
            ));
        }
    }
    Ok(())
}

fn is_approved_disabled(subkey: &str, value: &str) -> bool {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(wide(subkey).as_ptr()),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        ) != ERROR_SUCCESS
        {
            return false;
        }
        let mut data = [0u8; 16];
        let mut data_len = data.len() as u32;
        let queried = RegQueryValueExW(
            key,
            PCWSTR::from_raw(wide(value).as_ptr()),
            None,
            None,
            Some(data.as_mut_ptr()),
            Some(&mut data_len),
        );
        let _ = RegCloseKey(key);
        queried == ERROR_SUCCESS && data_len > 0 && data[0] != 0x02
    }
}

fn read_value_exists(subkey: &str, value: &str) -> bool {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(wide(subkey).as_ptr()),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        ) != ERROR_SUCCESS
        {
            return false;
        }
        let mut data_len = 0u32;
        let exists = RegQueryValueExW(
            key,
            PCWSTR::from_raw(wide(value).as_ptr()),
            None,
            None,
            None,
            Some(&mut data_len),
        ) == ERROR_SUCCESS;
        let _ = RegCloseKey(key);
        exists
    }
}

fn delete_value(subkey: &str, value: &str) -> Result<(), String> {
    unsafe {
        let mut key = HKEY::default();
        let opened = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(wide(subkey).as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut key,
        );
        if opened == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        if opened != ERROR_SUCCESS {
            return Err(format!(
                "Could not open startup settings (error {opened:?})"
            ));
        }
        let deleted = RegDeleteValueW(key, PCWSTR::from_raw(wide(value).as_ptr()));
        let _ = RegCloseKey(key);
        if deleted == ERROR_SUCCESS || deleted == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!(
                "Could not delete the startup value (error {deleted:?})"
            ))
        }
    }
}

fn startup_command(exe: &Path) -> Vec<u8> {
    std::iter::once(b'"' as u16)
        .chain(exe.as_os_str().encode_wide())
        .chain([b'"' as u16, 0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn wide(value: &str) -> Vec<u16> {
    wide_owned(value)
}

fn wide_owned(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_quotes_unicode_path_and_includes_terminator() {
        let data = startup_command(std::path::Path::new(r"C:\My Apps\日本語\TDT.exe"));
        let actual: Vec<u16> = data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(actual, wide("\"C:\\My Apps\\日本語\\TDT.exe\""));
    }

    #[test]
    fn win32_launch_path_strips_extended_prefix() {
        assert_eq!(
            win32_launch_path(Path::new(r"\\?\C:\Apps\TDT.exe")),
            PathBuf::from(r"C:\Apps\TDT.exe")
        );
        assert_eq!(
            win32_launch_path(Path::new(r"\\?\UNC\server\share\TDT.exe")),
            PathBuf::from(r"\\server\share\TDT.exe")
        );
    }

    #[test]
    fn shortcut_lives_in_user_startup_folder() {
        let path = shortcut_path();
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("TDT.lnk")
        );
        let rendered = path.to_string_lossy();
        assert!(
            rendered.contains("Start Menu") && rendered.contains("Startup"),
            "expected a user Startup folder path, got {rendered}"
        );
    }

    #[test]
    fn refresh_skips_test_binaries_and_keeps_installed_name() {
        assert!(is_tdt_exe_name(Path::new(r"C:\Users\hi\AppData\Local\TDT\TDT.exe")));
        assert!(is_tdt_exe_name(Path::new(r"C:\Apps\tdt.exe")));
        assert!(!is_tdt_exe_name(Path::new(
            r"C:\repo\desktop\target\debug\deps\TDT-71689b29e1f673d8.exe"
        )));
    }

    #[test]
    fn refresh_if_enabled_is_noop_when_disabled() {
        let shortcut = shortcut_path();
        let had_shortcut = shortcut.exists();
        if !had_shortcut && !read_value_exists(RUN_SUBKEY, VALUE_NAME) {
            refresh_if_enabled();
            assert!(!shortcut.exists());
            assert!(!read_value_exists(RUN_SUBKEY, VALUE_NAME));
        }
    }

    #[test]
    fn isolated_shortcut_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "tdt-startup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SHORTCUT_NAME);
        let exe = std::env::current_exe().unwrap();
        write_shortcut(&exe, &path).unwrap();
        assert!(
            path.is_file(),
            "shortcut should exist at {}",
            path.display()
        );
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            bytes.len() > 16 && bytes[0] == 0x4c,
            "shortcut should be a Windows .lnk file"
        );
        remove_shortcut(&path).unwrap();
        assert!(!path.exists());
        remove_shortcut(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn isolated_registry_round_trip() {
        struct TestKey(String);
        impl Drop for TestKey {
            fn drop(&mut self) {
                unsafe {
                    let _ = windows::Win32::System::Registry::RegDeleteKeyW(
                        HKEY_CURRENT_USER,
                        PCWSTR(wide(&self.0).as_ptr()),
                    );
                }
            }
        }
        let key = TestKey(format!(
            "Software\\TDT-Startup-Test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(!read_value_exists(&key.0, VALUE_NAME));
        write_approved(&key.0, VALUE_NAME).unwrap();
        assert!(read_value_exists(&key.0, VALUE_NAME));
        assert!(!is_approved_disabled(&key.0, VALUE_NAME));
        delete_value(&key.0, VALUE_NAME).unwrap();
        assert!(!read_value_exists(&key.0, VALUE_NAME));
        delete_value(&key.0, VALUE_NAME).unwrap();
    }
}
