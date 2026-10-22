//! Per-user "start with Windows".
//!
//! The toggle registers a Task Scheduler task that starts TDT when this user
//! signs in. Explorer starts Run-key and Startup-folder apps one at a time
//! and waits for each, so it can reach TDT minutes after sign-in; the task
//! does not wait in that queue.
//!
//! If the task cannot be registered, the toggle writes a Startup-folder
//! shortcut (working directory set to the exe folder), then the HKCU Run key,
//! and enables the matching StartupApproved value so Windows does not keep a
//! leftover Disabled state. Only one of the three exists at a time, so logon
//! does not launch TDT twice. TDT 0.4.2 and older used only the shortcut and
//! the Run key; the first start of a newer TDT moves them to the task.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::core::{Interface, BSTR, HSTRING, PCWSTR, PWSTR, VARIANT};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, E_FAIL, HANDLE, HLOCAL, TRUE,
};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_BINARY, REG_OPTION_NON_VOLATILE,
    REG_SZ,
};
use windows::Win32::System::TaskScheduler::{
    IExecAction, ITaskFolder, ITaskService, TaskScheduler, TASK_CREATE_OR_UPDATE,
    TASK_LOGON_INTERACTIVE_TOKEN,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
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
/// Task names are machine-wide, so each user gets their own task.
const TASK_PREFIX: &str = "TDT Startup ";
/// Windows 8+ enabled blob: 0x02, then 11 zero bytes.
const APPROVED_ENABLED: [u8; 12] = [0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// True when TDT will launch at sign-in and the user has not disabled it.
pub fn is_enabled() -> bool {
    match read_task() {
        Some(task) => task.enabled,
        None => legacy_enabled(),
    }
}

fn legacy_enabled() -> bool {
    let shortcut = shortcut_path();
    if !shortcut.as_os_str().is_empty()
        && shortcut.exists()
        && !is_approved_disabled(APPROVED_FOLDER, SHORTCUT_NAME)
    {
        return true;
    }
    read_value_exists(RUN_SUBKEY, VALUE_NAME) && !is_approved_disabled(APPROVED_RUN, VALUE_NAME)
}

/// Keep startup aimed at this TDT.exe after an update or move, and move an
/// older shortcut or Run value to the task.
pub fn refresh_if_enabled() {
    let Ok(exe) = current_tdt_exe() else {
        return;
    };
    if !is_tdt_exe_name(&exe) {
        return;
    }
    let needs_update = match read_task() {
        Some(task) => task.enabled && !same_path(&task.exe, &exe),
        // An older shortcut or Run value moves to the task. If the task still
        // cannot be registered, this rewrites the fallback, which is cheap.
        None => legacy_enabled(),
    };
    if needs_update {
        let _ = enable_startup();
    }
}

fn same_path(stored: &str, exe: &Path) -> bool {
    stored
        .trim_matches('"')
        .eq_ignore_ascii_case(&exe.to_string_lossy())
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
    match register_task(&exe) {
        Ok(()) => {
            // A leftover shortcut would only start a second TDT, which quits
            // at once because one already runs, so this is best effort.
            let _ = remove_legacy();
            Ok(())
        }
        Err(task_error) => {
            enable_legacy(&exe).map_err(|legacy_error| format!("{task_error}. {legacy_error}"))
        }
    }
}

fn disable_startup() -> Result<(), String> {
    let task = delete_task();
    let legacy = remove_legacy();
    match (task, legacy) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(task_error), Err(legacy_error)) => Err(format!("{task_error}. {legacy_error}")),
    }
}

fn enable_legacy(exe: &Path) -> Result<(), String> {
    let lnk = shortcut_path();
    // A claimed-successful write that left no file must fall back to the Run
    // key, not silently report enabled.
    let shortcut_result = if lnk.as_os_str().is_empty() {
        Err("Could not find the Startup folder.".into())
    } else {
        write_shortcut(exe, &lnk).and_then(|_| {
            if lnk.is_file() {
                Ok(())
            } else {
                Err("The Startup shortcut was not written.".into())
            }
        })
    };
    match shortcut_result {
        Ok(()) => {
            let _ = delete_value(RUN_SUBKEY, VALUE_NAME);
            let _ = delete_value(APPROVED_RUN, VALUE_NAME);
            if let Err(error) = write_approved(APPROVED_FOLDER, SHORTCUT_NAME) {
                if !lnk.as_os_str().is_empty() {
                    let _ = remove_shortcut(&lnk);
                }
                return Err(error);
            }
            Ok(())
        }
        Err(shortcut_error) => {
            // Never both: a stale .lnk beside the Run key launches TDT twice.
            if !lnk.as_os_str().is_empty() {
                let _ = remove_shortcut(&lnk);
            }
            match write_run_key(exe) {
                Ok(()) => {
                    write_approved(APPROVED_RUN, VALUE_NAME)?;
                    Ok(())
                }
                Err(run_error) => Err(format!("{shortcut_error}. {run_error}")),
            }
        }
    }
}

fn remove_legacy() -> Result<(), String> {
    let mut errors = Vec::new();
    let shortcut = shortcut_path();
    if !shortcut.as_os_str().is_empty() {
        if let Err(error) = remove_shortcut(&shortcut) {
            errors.push(error);
        }
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

struct StartupTask {
    enabled: bool,
    exe: String,
}

fn task_name() -> Result<String, String> {
    current_user_sid()
        .map(|sid| format!("{TASK_PREFIX}{sid}"))
        .ok_or_else(|| "Could not read the Windows user for the startup task.".into())
}

/// Runs `work` against the Task Scheduler root folder on a fresh COM thread.
fn with_task_folder<T: Send + 'static>(
    work: impl FnOnce(&ITaskFolder) -> windows::core::Result<T> + Send + 'static,
) -> windows::core::Result<T> {
    std::thread::spawn(move || unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let result = (|| {
            let service: ITaskService =
                CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
            let none = VARIANT::default();
            service.Connect(&none, &none, &none, &none)?;
            work(&service.GetFolder(&BSTR::from("\\"))?)
        })();
        CoUninitialize();
        result
    })
    .join()
    .unwrap_or_else(|_| Err(E_FAIL.into()))
}

/// None when this user has no startup task.
fn read_task() -> Option<StartupTask> {
    let name = task_name().ok()?;
    with_task_folder(move |folder| unsafe {
        let task = folder.GetTask(&BSTR::from(name))?;
        let action: IExecAction = task.Definition()?.Actions()?.get_Item(1)?.cast()?;
        let mut exe = BSTR::new();
        action.Path(&mut exe)?;
        Ok(StartupTask {
            enabled: task.Enabled()?.as_bool(),
            exe: exe.to_string(),
        })
    })
    .ok()
}

fn register_task(exe: &Path) -> Result<(), String> {
    let name = task_name()?;
    let sid = current_user_sid().unwrap_or_default();
    let workdir = exe.parent().unwrap_or(exe);
    // InteractiveToken runs TDT in the user's desktop session without a
    // stored password. Priority 4 keeps normal CPU and I/O priority; the
    // default 7 runs below normal. The time limit PT0S means no limit.
    let xml = format!(
        r#"<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Starts TDT when you sign in.</Description></RegistrationInfo>
  <Triggers><LogonTrigger><UserId>{sid}</UserId></LogonTrigger></Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{sid}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{}</Command>
      <WorkingDirectory>{}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>"#,
        xml_text(&exe.to_string_lossy()),
        xml_text(&workdir.to_string_lossy()),
    );
    with_task_folder(move |folder| unsafe {
        let none = VARIANT::default();
        folder
            .RegisterTask(
                &BSTR::from(name),
                &BSTR::from(xml),
                TASK_CREATE_OR_UPDATE.0,
                &none,
                &none,
                TASK_LOGON_INTERACTIVE_TOKEN,
                &none,
            )
            .map(|_| ())
    })
    .map_err(|error| format!("Could not register the startup task: {error}"))
}

fn delete_task() -> Result<(), String> {
    let name = task_name()?;
    with_task_folder(move |folder| unsafe {
        match folder.DeleteTask(&BSTR::from(name), 0) {
            Err(error) if error.code() == ERROR_FILE_NOT_FOUND.to_hresult() => Ok(()),
            other => other,
        }
    })
    .map_err(|error| format!("Could not remove the startup task: {error}"))
}

fn xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The user's SID, such as S-1-5-21-...-1001. The task trigger and principal
/// need it, and it makes the task name unique on a shared computer.
fn current_user_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut buffer = [0u64; 32];
        let mut len = 0u32;
        let read = GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            std::mem::size_of_val(&buffer) as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        read.ok()?;
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text).ok()?;
        let sid = text.to_string().ok();
        let _ = LocalFree(HLOCAL(text.0.cast()));
        sid
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
    let folder = startup_folder();
    if folder.as_os_str().is_empty() {
        return PathBuf::new();
    }
    folder.join(SHORTCUT_NAME)
}

fn startup_folder() -> PathBuf {
    // SHGetKnownFolderPath is the canonical answer and follows profile
    // redirection; APPDATA is only a fallback. No "." fallback — it would
    // drop the .lnk beside the exe where Explorer never reads it.
    if let Some(folder) = known_startup_folder() {
        return folder;
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Startup");
    }
    PathBuf::new()
}

fn known_startup_folder() -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Startup, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let rendered = SHGetKnownFolderPath(&FOLDERID_Startup, KF_FLAG_DEFAULT, None).ok()?;
        let path = rendered.to_string().ok();
        CoTaskMemFree(Some(rendered.as_ptr() as *const _));
        path.filter(|value| !value.is_empty()).map(PathBuf::from)
    }
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
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if init.is_err() {
        return Err("Could not initialize COM for the startup shortcut.".into());
    }
    let result: Result<(), String> = (|| {
        unsafe {
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
        Ok(())
    })();
    unsafe { CoUninitialize() };
    result?;
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
        queried == ERROR_SUCCESS && data_len >= 12 && data[0] != 0x02
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
