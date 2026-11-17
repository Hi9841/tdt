//! The app that gets the dictation: its name for the log, and whether Windows
//! lets TDT type into it.

use crate::integrity;
use std::fmt;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, E_ACCESSDENIED, HWND};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowThreadProcessId};

pub struct Target {
    exe: String,
    class: String,
    level: windows::core::Result<u32>,
}

impl Target {
    pub fn of(raw: isize) -> Self {
        let hwnd = HWND(raw as _);
        let mut class = [0u16; 128];
        let mut pid = 0u32;
        // SAFETY: both calls write into the buffers given; a stale handle makes a call fail, not misbehave.
        let (class_len, thread) = unsafe {
            (
                GetClassNameW(hwnd, &mut class),
                GetWindowThreadProcessId(hwnd, Some(&mut pid)),
            )
        };
        let class = String::from_utf16_lossy(&class[..class_len.max(0) as usize]);
        if thread == 0 {
            return Self {
                exe: "?".into(),
                class,
                level: Err(windows::core::Error::from_win32()),
            };
        }
        // SAFETY: the process handle is closed after the two queries. `path` outlives the call that fills
        // it, and `len` holds its size.
        let (exe, level) = unsafe {
            match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(process) => {
                    let mut path = [0u16; 512];
                    let mut len = path.len() as u32;
                    let exe = QueryFullProcessImageNameW(
                        process,
                        PROCESS_NAME_WIN32,
                        PWSTR(path.as_mut_ptr()),
                        &mut len,
                    )
                    .map(|()| String::from_utf16_lossy(&path[..len as usize]))
                    .unwrap_or_default();
                    let level = integrity::of_process(process);
                    let _ = CloseHandle(process);
                    (exe, level)
                }
                Err(error) => (String::new(), Err(error)),
            }
        };
        // Only the file name: the full path can hold the user name.
        let exe = exe
            .rsplit('\\')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("?");
        Self {
            exe: exe.to_string(),
            class,
            level,
        }
    }

    /// True when Windows drops TDT's keys for this app. Windows can refuse to
    /// tell TDT the level of an app that runs as administrator, so a denied
    /// query counts as a higher level.
    pub fn blocks_input(&self) -> bool {
        let own = integrity::own().unwrap_or(integrity::MEDIUM);
        match &self.level {
            Ok(level) => *level > own,
            Err(error) => error.code() == E_ACCESSDENIED && own < integrity::HIGH,
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}, ", self.exe, self.class)?;
        match &self.level {
            Ok(level) => write!(f, "{})", integrity::name(*level)),
            Err(error) if error.code() == E_ACCESSDENIED => f.write_str("level denied)"),
            Err(_) => f.write_str("level unknown)"),
        }
    }
}
