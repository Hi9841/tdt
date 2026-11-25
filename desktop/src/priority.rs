//! Thread priority for the path from the key to the typed text.

/// For a thread between the key and the typed text. These threads sleep
/// nearly all the time, so the priority costs other programs nothing, and a
/// CPU load at normal priority no longer stands in front of each step.
#[cfg(windows)]
pub fn raise_thread() {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
    };
    // SAFETY: the pseudo handle names the calling thread; the call takes no
    // pointer. A failed call only leaves the thread at normal priority.
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL) };
}

#[cfg(target_os = "macos")]
pub fn raise_thread() {
    const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
    extern "C" {
        fn pthread_set_qos_class_self_np(
            qos_class: u32,
            relative_priority: libc::c_int,
        ) -> libc::c_int;
    }
    // SAFETY: `pthread_set_qos_class_self_np` only adjusts the calling thread's QoS class.
    let _ = unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn raise_thread() {}
