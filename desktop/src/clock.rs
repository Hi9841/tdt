//! The local time, for History and for the log.

/// `YYYY-MM-DD HH:MM:SS` in the local time zone.
#[cfg(windows)]
pub fn local_hms() -> String {
    // SAFETY: GetLocalTime takes no pointer and returns the time by value.
    let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute, time.wSecond
    )
}

/// `YYYY-MM-DD HH:MM:SS` in the local time zone.
#[cfg(not(windows))]
pub fn local_hms() -> String {
    // SAFETY: `time` and `localtime_r` operate on stack-owned POD structs.
    unsafe {
        let mut now: libc::time_t = 0;
        libc::time(&mut now);
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return "1970-01-01 00:00:00".to_string();
        }
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}
