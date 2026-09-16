pub mod hook;
pub use hook::{
    begin_capture, end_capture, poll_capture_timeout, set_binding, tap_should_stop, HotkeyAction,
    HotkeyBinding, HotkeyListener,
};
