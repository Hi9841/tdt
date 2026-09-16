use crossbeam_channel::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    PressStarted,
    HoldReleased(Duration),
    Toggled,
}

/// A short press starts recording when idle and stops an already-running toggle session.
pub fn tap_should_stop(recording_before_press: bool) -> bool {
    recording_before_press
}

pub struct HotkeyListener {
    _running: Arc<AtomicBool>,
}

impl HotkeyListener {
    #[cfg(target_os = "windows")]
    pub fn start() -> (Self, Receiver<HotkeyAction>) {
        use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            GetAsyncKeyState, VK_CONTROL, VK_LCONTROL, VK_OEM_1, VK_RCONTROL,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
            HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
            WM_SYSKEYUP,
        };

        let (sender, receiver) = crossbeam_channel::unbounded();
        let running = Arc::new(AtomicBool::new(true));
        let running_thread = Arc::clone(&running);

        std::thread::spawn(move || {
            use parking_lot::Mutex;
            static SENDER: Mutex<Option<Sender<HotkeyAction>>> = Mutex::new(None);
            static PRESS_START: Mutex<Option<Instant>> = Mutex::new(None);
            static mut HOOK: Option<HHOOK> = None;

            *SENDER.lock() = Some(sender);

            unsafe extern "system" fn hook_proc(
                code: i32,
                wparam: WPARAM,
                lparam: LPARAM,
            ) -> LRESULT {
                if code >= 0 {
                    let kbd = *(lparam.0 as *const KBDLLHOOKSTRUCT);
                    let msg = wparam.0 as u32;

                    let is_semicolon = kbd.vkCode == VK_OEM_1.0 as u32;
                    let is_ctrl = kbd.vkCode == VK_CONTROL.0 as u32
                        || kbd.vkCode == VK_LCONTROL.0 as u32
                        || kbd.vkCode == VK_RCONTROL.0 as u32;

                    let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
                    let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

                    if is_semicolon && is_down {
                        let ctrl_down = (GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000)
                            != 0
                            || (GetAsyncKeyState(VK_LCONTROL.0 as i32) as u16 & 0x8000) != 0
                            || (GetAsyncKeyState(VK_RCONTROL.0 as i32) as u16 & 0x8000) != 0;

                        if ctrl_down {
                            let mut start_guard = PRESS_START.lock();
                            if start_guard.is_none() {
                                *start_guard = Some(Instant::now());
                                if let Some(ref s) = *SENDER.lock() {
                                    let _ = s.send(HotkeyAction::PressStarted);
                                }
                            }
                            // SWALLOW hotkey so it does not leak into the terminal as an ESC sequence
                            return LRESULT(1);
                        }
                    } else if is_semicolon && is_up {
                        // Release when semicolon is released
                        let start_opt = PRESS_START.lock().take();
                        if let Some(start) = start_opt {
                            let duration = start.elapsed();
                            let action = if duration > Duration::from_millis(350) {
                                HotkeyAction::HoldReleased(duration)
                            } else {
                                HotkeyAction::Toggled
                            };
                            if let Some(ref s) = *SENDER.lock() {
                                let _ = s.send(action);
                            }
                            // SWALLOW keyup for semicolon
                            return LRESULT(1);
                        }
                    } else if is_ctrl && is_up {
                        // If Ctrl is released before semicolon, complete the action without swallowing Ctrl
                        let start_opt = PRESS_START.lock().take();
                        if let Some(start) = start_opt {
                            let duration = start.elapsed();
                            let action = if duration > Duration::from_millis(350) {
                                HotkeyAction::HoldReleased(duration)
                            } else {
                                HotkeyAction::Toggled
                            };
                            if let Some(ref s) = *SENDER.lock() {
                                let _ = s.send(action);
                            }
                        }
                    }
                }
                CallNextHookEx(HOOK.unwrap_or_default(), code, wparam, lparam)
            }

            unsafe {
                let hook =
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), HINSTANCE::default(), 0);

                if let Ok(h) = hook {
                    HOOK = Some(h);
                    let mut msg = MSG::default();
                    while running_thread.load(Ordering::Relaxed)
                        && GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool()
                    {
                        let _ = DispatchMessageW(&msg);
                    }
                    let _ = UnhookWindowsHookEx(h);
                }
            }
        });

        (Self { _running: running }, receiver)
    }

    #[cfg(not(target_os = "windows"))]
    pub fn start() -> (Self, Receiver<HotkeyAction>) {
        let (_sender, receiver) = crossbeam_channel::unbounded();
        let running = Arc::new(AtomicBool::new(true));
        (Self { _running: running }, receiver)
    }
}

#[cfg(test)]
mod tests {
    use super::tap_should_stop;

    #[test]
    fn first_tap_starts_and_second_tap_stops() {
        assert!(!tap_should_stop(false));
        assert!(tap_should_stop(true));
    }
}
