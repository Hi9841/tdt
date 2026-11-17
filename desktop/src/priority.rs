//! Thread priority for the path from the key to the typed text.

/// For a thread between the key and the typed text. These threads sleep
/// nearly all the time, so the priority costs other programs nothing, and a
/// CPU load at normal priority no longer stands in front of each step.
pub fn raise_thread() {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
    };
    // SAFETY: the pseudo handle names the calling thread; the call takes no
    // pointer. A failed call only leaves the thread at normal priority.
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL) };
}
