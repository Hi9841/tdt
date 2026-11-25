#[cfg(windows)]
pub mod hook;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
pub mod hook;
pub use hook::{
    begin_capture, current_binding, end_capture, input_held, note_user_typed, poll_capture_timeout,
    poll_missed_release, set_binding, typed_generation, user_typed_since, wake_loop, HotkeyAction,
    HotkeyBinding, HotkeyListener,
};
