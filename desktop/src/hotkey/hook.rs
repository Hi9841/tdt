use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    PressStarted,
    HoldReleased,
    Toggled,
    Captured(HotkeyBinding),
    CaptureCancelled,
}

/// A press longer than this records until release instead of toggling.
pub const HOLD_THRESHOLD: Duration = Duration::from_millis(350);

/// A rebind request that is left unanswered stops listening for keys after
/// this long, so a forgotten capture cannot swallow the keyboard.
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub vk: u16,
}

impl HotkeyBinding {
    pub const DEFAULT: Self = Self {
        ctrl: true,
        alt: false,
        shift: false,
        win: false,
        vk: 0xBA,
    };

    pub fn parse(raw: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut win = false;
        let mut vk = None;
        for part in raw.split('+') {
            let token = part.trim();
            if token.is_empty() {
                continue;
            }
            match token.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" | "option" => alt = true,
                "shift" => shift = true,
                "win" | "windows" | "super" | "meta" => win = true,
                other => vk = parse_vk(other),
            }
        }
        let binding = Self {
            ctrl,
            alt,
            shift,
            win,
            vk: vk?,
        };
        binding.is_valid().then_some(binding)
    }

    pub fn display(self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.win {
            parts.push("Win".to_string());
        }
        parts.push(vk_label(self.vk));
        parts.join("+")
    }

    pub fn is_valid(self) -> bool {
        if self.vk == 0 || self.vk == 0x1B || is_modifier_vk(self.vk) {
            return false;
        }
        let has_mod = self.ctrl || self.alt || self.shift || self.win;
        let is_function = (0x70..=0x87).contains(&self.vk);
        has_mod || is_function
    }
}

impl Default for HotkeyBinding {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A short press starts recording when idle and stops an already-running toggle session.
pub fn tap_should_stop(recording_before_press: bool) -> bool {
    recording_before_press
}

/// True when a press was held long enough to mean "record until release".
pub fn is_hold(duration: Duration) -> bool {
    duration > HOLD_THRESHOLD
}

fn capture_timed_out(start: Instant, now: Instant) -> bool {
    now.saturating_duration_since(start) >= CAPTURE_TIMEOUT
}

static BINDING: Mutex<HotkeyBinding> = Mutex::new(HotkeyBinding::DEFAULT);
static CAPTURE: AtomicBool = AtomicBool::new(false);
static CAPTURE_START: Mutex<Option<Instant>> = Mutex::new(None);
static PRESS_START: Mutex<Option<Instant>> = Mutex::new(None);
static SENDER: Mutex<Option<Sender<HotkeyAction>>> = Mutex::new(None);

pub fn set_binding(binding: HotkeyBinding) {
    if binding.is_valid() {
        *BINDING.lock() = binding;
    }
}

pub fn current_binding() -> HotkeyBinding {
    *BINDING.lock()
}

pub fn begin_capture() {
    PRESS_START.lock().take();
    *CAPTURE_START.lock() = Some(Instant::now());
    CAPTURE.store(true, Ordering::SeqCst);
}

pub fn end_capture() {
    *CAPTURE_START.lock() = None;
    CAPTURE.store(false, Ordering::SeqCst);
}

/// Cancel an abandoned capture from the application poll loop. Keyboard hooks
/// only run when a key changes, so relying on the hook alone would leave the
/// settings UI in capture mode forever when the user walks away.
pub fn poll_capture_timeout() {
    if CAPTURE.load(Ordering::SeqCst) && capture_expired() {
        end_capture();
        send_action(HotkeyAction::CaptureCancelled);
    }
}

fn capture_expired() -> bool {
    CAPTURE_START
        .lock()
        .is_some_and(|start| capture_timed_out(start, Instant::now()))
}

fn send_action(action: HotkeyAction) {
    if let Some(ref sender) = *SENDER.lock() {
        let _ = sender.send(action);
    }
}

fn finish_press() {
    let Some(start) = PRESS_START.lock().take() else {
        return;
    };
    let action = if is_hold(start.elapsed()) {
        HotkeyAction::HoldReleased
    } else {
        HotkeyAction::Toggled
    };
    send_action(action);
}

pub struct HotkeyListener {
    _running: Arc<AtomicBool>,
}

impl HotkeyListener {
    #[cfg(target_os = "windows")]
    pub fn start() -> (Self, Receiver<HotkeyAction>) {
        use windows::Win32::Foundation::{HINSTANCE, HWND};
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, GetMessageW, SetWindowsHookExW, UnhookWindowsHookEx, MSG,
            WH_KEYBOARD_LL,
        };

        let (sender, receiver) = crossbeam_channel::unbounded();
        let running = Arc::new(AtomicBool::new(true));
        let running_thread = Arc::clone(&running);
        *SENDER.lock() = Some(sender);

        std::thread::spawn(move || unsafe {
            let hook = SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(hotkey_hook_proc),
                HINSTANCE::default(),
                0,
            );
            if let Ok(h) = hook {
                HOOK_PTR.store(h.0, Ordering::SeqCst);
                let mut msg = MSG::default();
                while running_thread.load(Ordering::Relaxed)
                    && GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool()
                {
                    let _ = DispatchMessageW(&msg);
                }
                let _ = UnhookWindowsHookEx(h);
                HOOK_PTR.store(std::ptr::null_mut(), Ordering::SeqCst);
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

#[cfg(target_os = "windows")]
static HOOK_PTR: std::sync::atomic::AtomicPtr<core::ffi::c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

#[cfg(target_os = "windows")]
fn key_down(vk: i32) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    unsafe { GetAsyncKeyState(vk) as u16 & 0x8000 != 0 }
}

#[cfg(target_os = "windows")]
fn modifiers_down() -> (bool, bool, bool, bool) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU,
        VK_RSHIFT, VK_RWIN, VK_SHIFT,
    };
    let ctrl = key_down(VK_CONTROL.0 as i32)
        || key_down(VK_LCONTROL.0 as i32)
        || key_down(VK_RCONTROL.0 as i32);
    let alt =
        key_down(VK_MENU.0 as i32) || key_down(VK_LMENU.0 as i32) || key_down(VK_RMENU.0 as i32);
    let shift =
        key_down(VK_SHIFT.0 as i32) || key_down(VK_LSHIFT.0 as i32) || key_down(VK_RSHIFT.0 as i32);
    let win = key_down(VK_LWIN.0 as i32) || key_down(VK_RWIN.0 as i32);
    (ctrl, alt, shift, win)
}

#[cfg(target_os = "windows")]
fn modifiers_match(binding: HotkeyBinding) -> bool {
    let (ctrl, alt, shift, win) = modifiers_down();
    ctrl == binding.ctrl && alt == binding.alt && shift == binding.shift && win == binding.win
}

fn is_required_modifier(vk: u16, binding: HotkeyBinding) -> bool {
    (binding.ctrl && matches!(vk, 0x11 | 0xA2 | 0xA3))
        || (binding.alt && matches!(vk, 0x12 | 0xA4 | 0xA5))
        || (binding.shift && matches!(vk, 0x10 | 0xA0 | 0xA1))
        || (binding.win && matches!(vk, 0x5B | 0x5C))
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn hotkey_hook_proc(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, WM_KEYDOWN, WM_KEYUP,
        WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    if code >= 0 {
        let kbd = *(lparam.0 as *const KBDLLHOOKSTRUCT);
        let msg = wparam.0 as u32;
        let vk = kbd.vkCode as u16;
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

        // Ignore synthetic keys: auto-paste types text with SendInput, and that
        // text must never start a recording or be mistaken for a new shortcut.
        if kbd.flags & LLKHF_INJECTED == LLKHF_INJECTED {
            return CallNextHookEx(
                HHOOK(HOOK_PTR.load(Ordering::Relaxed)),
                code,
                wparam,
                lparam,
            );
        }

        if CAPTURE.load(Ordering::SeqCst) {
            if capture_expired() {
                end_capture();
                send_action(HotkeyAction::CaptureCancelled);
            } else if is_down && vk == 0x1B {
                end_capture();
                send_action(HotkeyAction::CaptureCancelled);
                return LRESULT(1);
            } else if !is_modifier_vk(vk) && (is_down || is_up) {
                if is_down {
                    let (ctrl, alt, shift, win) = modifiers_down();
                    let captured = HotkeyBinding {
                        ctrl,
                        alt,
                        shift,
                        win,
                        vk,
                    };
                    if captured.is_valid() {
                        set_binding(captured);
                        end_capture();
                        send_action(HotkeyAction::Captured(captured));
                    }
                }
                return LRESULT(1);
            }
        } else {
            let binding = current_binding();
            if is_down && vk == binding.vk && modifiers_match(binding) {
                let mut start_guard = PRESS_START.lock();
                if start_guard.is_none() {
                    *start_guard = Some(Instant::now());
                    send_action(HotkeyAction::PressStarted);
                }
                return LRESULT(1);
            }
            if is_up && vk == binding.vk {
                if PRESS_START.lock().is_some() {
                    finish_press();
                    return LRESULT(1);
                }
            } else if is_up && is_required_modifier(vk, binding) {
                finish_press();
            }
        }
    }
    CallNextHookEx(
        HHOOK(HOOK_PTR.load(Ordering::Relaxed)),
        code,
        wparam,
        lparam,
    )
}

fn is_modifier_vk(vk: u16) -> bool {
    matches!(
        vk,
        0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0 | 0xA1 | 0xA2 | 0xA3 | 0xA4 | 0xA5
    )
}

fn parse_vk(token: &str) -> Option<u16> {
    let lower = token.trim().to_ascii_lowercase();
    match lower.as_str() {
        ";" | "semicolon" => Some(0xBA),
        "=" | "plus" | "equal" => Some(0xBB),
        "," | "comma" => Some(0xBC),
        "-" | "minus" => Some(0xBD),
        "." | "period" | "dot" => Some(0xBE),
        "/" | "slash" => Some(0xBF),
        "`" | "grave" => Some(0xC0),
        "[" => Some(0xDB),
        "\\" | "backslash" => Some(0xDC),
        "]" => Some(0xDD),
        "'" | "quote" => Some(0xDE),
        "space" => Some(0x20),
        "enter" | "return" => Some(0x0D),
        "tab" => Some(0x09),
        "backspace" => Some(0x08),
        other => {
            if let Some(num) = other.strip_prefix('f') {
                let n: u16 = num.parse().ok()?;
                if (1..=24).contains(&n) {
                    return Some(0x6F + n);
                }
            }
            if other.len() == 1 {
                let ch = other.chars().next()?.to_ascii_uppercase();
                if ch.is_ascii_alphanumeric() {
                    return Some(ch as u16);
                }
            }
            None
        }
    }
}

fn vk_label(vk: u16) -> String {
    match vk {
        0x08 => "Backspace".into(),
        0x09 => "Tab".into(),
        0x0D => "Enter".into(),
        0x20 => "Space".into(),
        0xBA => ";".into(),
        0xBB => "=".into(),
        0xBC => ",".into(),
        0xBD => "-".into(),
        0xBE => ".".into(),
        0xBF => "/".into(),
        0xC0 => "`".into(),
        0xDB => "[".into(),
        0xDC => "\\".into(),
        0xDD => "]".into(),
        0xDE => "'".into(),
        0x30..=0x39 | 0x41..=0x5A => char::from(vk as u8).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => format!("Vk{vk:02X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        capture_timed_out, is_hold, tap_should_stop, HotkeyBinding, CAPTURE_TIMEOUT, HOLD_THRESHOLD,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn first_tap_starts_and_second_tap_stops() {
        assert!(!tap_should_stop(false));
        assert!(tap_should_stop(true));
    }

    #[test]
    fn press_duration_picks_tap_or_hold() {
        assert!(!is_hold(HOLD_THRESHOLD - Duration::from_millis(1)));
        assert!(!is_hold(HOLD_THRESHOLD));
        assert!(is_hold(HOLD_THRESHOLD + Duration::from_millis(1)));
    }

    #[test]
    fn capture_expires_so_it_cannot_swallow_the_keyboard() {
        let start = Instant::now();
        assert!(!capture_timed_out(
            start,
            start + Duration::from_millis(500)
        ));
        assert!(capture_timed_out(start, start + CAPTURE_TIMEOUT));
        assert!(capture_timed_out(
            start,
            start + CAPTURE_TIMEOUT + Duration::from_millis(1)
        ));
    }

    #[test]
    fn parse_default_ctrl_semicolon() {
        let binding = HotkeyBinding::parse("Ctrl+;").unwrap();
        assert_eq!(binding, HotkeyBinding::DEFAULT);
        assert_eq!(binding.display(), "Ctrl+;");
    }

    #[test]
    fn parse_function_key_without_modifier() {
        let binding = HotkeyBinding::parse("F9").unwrap();
        assert!(!binding.ctrl);
        assert_eq!(binding.display(), "F9");
    }

    #[test]
    fn parse_ctrl_shift_space() {
        let binding = HotkeyBinding::parse("ctrl+shift+space").unwrap();
        assert_eq!(binding.display(), "Ctrl+Shift+Space");
    }

    #[test]
    fn reject_letter_without_modifier() {
        assert!(HotkeyBinding::parse("A").is_none());
        assert!(HotkeyBinding::parse("Esc").is_none());
    }
}
