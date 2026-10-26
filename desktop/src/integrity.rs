//! Windows integrity levels. Windows drops the keys that a process sends to a
//! window of a process at a higher level (UIPI), and it reports no error.

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub const MEDIUM: u32 = 0x2000;
pub const HIGH: u32 = 0x3000;

/// The integrity level of TDT.
pub fn own() -> Option<u32> {
    // SAFETY: the pseudo handle of the current process is always valid.
    unsafe { of_process(GetCurrentProcess()).ok() }
}

/// The integrity level of `process`. Windows can deny the query for a
/// process at a higher level than the caller.
///
/// # Safety
/// `process` must be a live process handle with query access.
pub unsafe fn of_process(process: HANDLE) -> windows::core::Result<u32> {
    // SAFETY: `token` is closed after one query. `buf` is 8-byte aligned and sized for the label and its
    // SID; a short buffer fails the call before the read. The SID pointer stays valid while `buf` lives.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token)?;
        let mut buf = [0u64; 16];
        let mut len = 0u32;
        let read = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buf.as_mut_ptr().cast()),
            std::mem::size_of_val(&buf) as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        read?;
        let sid = (*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL)).Label.Sid;
        Ok(*GetSidSubAuthority(
            sid,
            u32::from(*GetSidSubAuthorityCount(sid)) - 1,
        ))
    }
}

pub fn name(level: u32) -> &'static str {
    match level {
        ..MEDIUM => "low",
        MEDIUM..HIGH => "medium",
        _ => "high",
    }
}
