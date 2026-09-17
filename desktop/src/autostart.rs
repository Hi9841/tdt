//! Per-user "start with Windows" support backed by the HKCU Run key.
//!
//! The Run key itself is the source of truth, so there is no extra config
//! field to keep in sync: removing the value disables autostart.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};

const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "TDT";

/// True when the current user's Run key holds a TDT entry.
pub fn is_enabled() -> bool {
    read_enabled(RUN_SUBKEY)
}

fn read_enabled(subkey: &str) -> bool {
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
            PCWSTR::from_raw(wide(VALUE_NAME).as_ptr()),
            None,
            None,
            None,
            Some(&mut data_len),
        ) == ERROR_SUCCESS;
        let _ = RegCloseKey(key);
        exists
    }
}

/// Register or unregister TDT under the current user's Run key. The value
/// points at the running executable, so it stays correct after updates.
pub fn set_enabled(enable: bool) -> Result<(), String> {
    set_entry(RUN_SUBKEY, enable)
}

fn set_entry(subkey: &str, enable: bool) -> Result<(), String> {
    if !enable {
        return disable(subkey);
    }
    let exe =
        std::env::current_exe().map_err(|error| format!("Could not locate TDT.exe: {error}"))?;
    let data = startup_command(&exe);
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

fn disable(subkey: &str) -> Result<(), String> {
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
        let deleted = RegDeleteValueW(key, PCWSTR::from_raw(wide(VALUE_NAME).as_ptr()));
        let _ = RegCloseKey(key);
        if deleted == ERROR_SUCCESS || deleted == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!(
                "Could not delete the Run registry value (error {deleted:?})"
            ))
        }
    }
}

fn startup_command(exe: &std::path::Path) -> Vec<u8> {
    std::iter::once(b'"' as u16)
        .chain(exe.as_os_str().encode_wide())
        .chain([b'"' as u16, 0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// Null-terminated UTF-16 for Win32 wide-string APIs.
fn wide(value: &str) -> Vec<u16> {
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
        assert!(!read_enabled(&key.0));
        set_entry(&key.0, false).unwrap();
        set_entry(&key.0, true).unwrap();
        assert!(read_enabled(&key.0));
        set_entry(&key.0, true).unwrap();
        set_entry(&key.0, false).unwrap();
        assert!(!read_enabled(&key.0));
        set_entry(&key.0, false).unwrap();
    }
}
