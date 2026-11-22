//! Lines for `adb logcat -s TDT:I`: times and counts, never the user's words.

/// Writes one line to the Android log, and to stderr on other systems.
pub fn note(message: &str) {
    #[cfg(target_os = "android")]
    {
        extern "C" {
            fn __android_log_write(
                priority: i32,
                tag: *const std::ffi::c_char,
                text: *const std::ffi::c_char,
            ) -> i32;
        }
        /// ANDROID_LOG_INFO.
        const INFO: i32 = 4;
        if let Ok(text) = std::ffi::CString::new(message) {
            // SAFETY: both strings are NUL-terminated and live through the call.
            unsafe { __android_log_write(INFO, c"TDT".as_ptr(), text.as_ptr()) };
        }
    }
    #[cfg(not(target_os = "android"))]
    eprintln!("tdt: {message}");
}
