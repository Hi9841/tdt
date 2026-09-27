pub mod hook;
pub use hook::{
    begin_capture, current_binding, end_capture, input_appeared, input_mask, is_modifier_vk,
    poll_capture_timeout, set_binding, tap_should_stop, HotkeyAction, HotkeyBinding,
    HotkeyListener,
};
