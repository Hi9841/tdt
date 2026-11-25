//! Global hotkey listener on macOS using a CoreGraphics `CGEventTap`.

use async_channel::{Receiver, Sender};
use parking_lot::Mutex;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    PressStarted(u64),
    HoldReleased(Instant),
    Toggled(Instant),
    Captured(HotkeyBinding),
    CaptureCancelled,
    Wake,
}

const HOLD_THRESHOLD: Duration = Duration::from_millis(350);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(6);

const VK_MOUSE4: u16 = 0x05;
const VK_MOUSE5: u16 = 0x06;

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
                "win" | "windows" | "super" | "meta" | "cmd" | "command" => win = true,
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
        self.key_labels().join("+")
    }

    pub fn key_labels(self) -> Vec<String> {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Option".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.win {
            parts.push("Cmd".to_string());
        }
        parts.push(vk_label(self.vk));
        parts
    }

    pub fn is_valid(self) -> bool {
        if self.vk == 0 || self.vk == 0x1B || crate::keys::is_modifier(self.vk) {
            return false;
        }
        let has_mod = self.ctrl || self.alt || self.shift || self.win;
        let is_function = (0x70..=0x87).contains(&self.vk);
        has_mod || is_function || self.is_mouse()
    }

    pub fn is_mouse(self) -> bool {
        matches!(self.vk, VK_MOUSE4 | VK_MOUSE5)
    }
}

impl Default for HotkeyBinding {
    fn default() -> Self {
        Self::DEFAULT
    }
}

static BINDING: Mutex<HotkeyBinding> = Mutex::new(HotkeyBinding::DEFAULT);
static CAPTURE: AtomicBool = AtomicBool::new(false);
static CAPTURE_START: Mutex<Option<Instant>> = Mutex::new(None);
static PRESS_START: Mutex<Option<Instant>> = Mutex::new(None);
static SENDER: Mutex<Option<Sender<HotkeyAction>>> = Mutex::new(None);
static SHORTCUT_DOWN: AtomicBool = AtomicBool::new(false);
static TYPED_EVENT: AtomicU64 = AtomicU64::new(0);
static PHYSICAL_DOWN: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];
static TAP_REF: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static RUN_LOOP_REF: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

pub fn set_binding(binding: HotkeyBinding) {
    if binding.is_valid() {
        *BINDING.lock() = binding;
        SHORTCUT_DOWN.store(false, Ordering::SeqCst);
    }
}

pub fn current_binding() -> HotkeyBinding {
    *BINDING.lock()
}

pub fn begin_capture() {
    if PRESS_START.lock().take().is_some() {
        send_action(HotkeyAction::HoldReleased(Instant::now()));
    }
    *CAPTURE_START.lock() = Some(Instant::now());
    CAPTURE.store(true, Ordering::SeqCst);
}

pub fn end_capture() {
    *CAPTURE_START.lock() = None;
    CAPTURE.store(false, Ordering::SeqCst);
}

pub fn poll_capture_timeout() {
    if CAPTURE.load(Ordering::SeqCst) && capture_expired() {
        end_capture();
        send_action(HotkeyAction::CaptureCancelled);
    }
}

fn capture_expired() -> bool {
    CAPTURE_START
        .lock()
        .is_some_and(|start| start.elapsed() >= CAPTURE_TIMEOUT)
}

pub fn wake_loop() {
    send_action(HotkeyAction::Wake);
}

pub fn poll_missed_release() {
    if PRESS_START.lock().is_none() {
        return;
    }
    let binding = current_binding();
    let (ctrl, alt, shift, win) = modifiers_down();
    let released = (binding.ctrl && !ctrl)
        || (binding.alt && !alt)
        || (binding.shift && !shift)
        || (binding.win && !win);
    if released {
        finish_press();
    }
}

fn send_action(action: HotkeyAction) {
    if let Some(ref sender) = *SENDER.lock() {
        let _ = sender.try_send(action);
    }
}

pub fn typed_generation() -> u64 {
    TYPED_EVENT.load(Ordering::SeqCst) >> 16
}

pub fn note_user_typed(vk: u16) {
    let _ = TYPED_EVENT.try_update(Ordering::SeqCst, Ordering::SeqCst, |event| {
        Some(((event & !0xffff).wrapping_add(1 << 16)) | u64::from(vk))
    });
}

pub fn user_typed_since(generation: u64) -> Option<u16> {
    let event = TYPED_EVENT.load(Ordering::SeqCst);
    (event >> 16 != generation).then_some(event as u16)
}

pub fn input_held(binding_vk: u16) -> Option<u16> {
    (1..=255u16).find(|&vk| {
        PHYSICAL_DOWN[usize::from(vk)].load(Ordering::SeqCst)
            && !crate::keys::is_modifier(vk)
            && vk != binding_vk
    })
}

fn start_press() {
    let mut start_guard = PRESS_START.lock();
    if start_guard.is_none() {
        *start_guard = Some(Instant::now());
        send_action(HotkeyAction::PressStarted(typed_generation()));
    }
}

fn finish_capture(captured: HotkeyBinding) {
    end_capture();
    if captured.is_valid() {
        set_binding(captured);
        send_action(HotkeyAction::Captured(captured));
    } else {
        send_action(HotkeyAction::CaptureCancelled);
    }
}

fn finish_press() {
    let Some(start) = PRESS_START.lock().take() else {
        return;
    };
    let action = if start.elapsed() > HOLD_THRESHOLD {
        HotkeyAction::HoldReleased(Instant::now())
    } else {
        HotkeyAction::Toggled(start)
    };
    send_action(action);
}

// CoreGraphics / CoreFoundation FFI
type CFMachPortRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFStringRef = *const c_void;
type CGEventTapProxy = *mut c_void;
type CGEventRef = *mut c_void;
type CGEventType = u32;
type CGEventFlags = u64;
type CGEventMask = u64;

const K_CG_SESSION_EVENT_TAP: u32 = 1;
const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
const K_CG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;

const K_CG_EVENT_KEY_DOWN: CGEventType = 10;
const K_CG_EVENT_KEY_UP: CGEventType = 11;
const K_CG_EVENT_FLAGS_CHANGED: CGEventType = 12;
const K_CG_EVENT_OTHER_MOUSE_DOWN: CGEventType = 25;
const K_CG_EVENT_OTHER_MOUSE_UP: CGEventType = 26;
const K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT: CGEventType = 0xFFFF_FFFE;
const K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT: CGEventType = 0xFFFF_FFFF;

const K_CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
const K_CG_KEYBOARD_EVENT_AUTOREPEAT: u32 = 8;
const K_CG_MOUSE_EVENT_BUTTON_NUMBER: u32 = 3;
const K_CG_EVENT_SOURCE_USER_DATA: u32 = 42;
const K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION: i32 = 0;

const K_CG_EVENT_FLAG_MASK_SHIFT: CGEventFlags = 1 << 17;
const K_CG_EVENT_FLAG_MASK_CONTROL: CGEventFlags = 1 << 18;
const K_CG_EVENT_FLAG_MASK_ALTERNATE: CGEventFlags = 1 << 19;
const K_CG_EVENT_FLAG_MASK_COMMAND: CGEventFlags = 1 << 20;

type CGEventTapCallBack = unsafe extern "C" fn(
    proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: CGEventMask,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventGetFlags(event: CGEventRef) -> CGEventFlags;
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    fn CGEventSourceFlagsState(state_id: i32) -> CGEventFlags;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopCommonModes: CFStringRef;
    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: CFMachPortRef,
        order: isize,
    ) -> CFRunLoopSourceRef;
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRun();
    fn CFRunLoopStop(rl: CFRunLoopRef);
    fn CFRelease(cf: *const c_void);
}

fn modifiers_from_flags(flags: CGEventFlags) -> (bool, bool, bool, bool) {
    (
        flags & K_CG_EVENT_FLAG_MASK_CONTROL != 0,
        flags & K_CG_EVENT_FLAG_MASK_ALTERNATE != 0,
        flags & K_CG_EVENT_FLAG_MASK_SHIFT != 0,
        flags & K_CG_EVENT_FLAG_MASK_COMMAND != 0,
    )
}

fn modifiers_down() -> (bool, bool, bool, bool) {
    // SAFETY: `CGEventSourceFlagsState` reads the combined session modifier bitmask.
    modifiers_from_flags(unsafe {
        CGEventSourceFlagsState(K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION)
    })
}

fn modifiers_match_flags(binding: HotkeyBinding, flags: CGEventFlags) -> bool {
    let (ctrl, alt, shift, win) = modifiers_from_flags(flags);
    ctrl == binding.ctrl && alt == binding.alt && shift == binding.shift && win == binding.win
}

unsafe extern "C" fn event_tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: CGEventRef,
    _user_info: *mut c_void,
) -> CGEventRef {
    if event_type == K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT
        || event_type == K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT
    {
        let tap = TAP_REF.load(Ordering::SeqCst);
        if !tap.is_null() {
            CGEventTapEnable(tap, true);
        }
        return event;
    }

    if SENDER.lock().is_none() {
        return event;
    }

    let user_data = CGEventGetIntegerValueField(event, K_CG_EVENT_SOURCE_USER_DATA);
    if user_data == crate::keys::OWN_INPUT as i64 {
        return event;
    }

    let flags = CGEventGetFlags(event);

    match event_type {
        K_CG_EVENT_FLAGS_CHANGED => {
            if PRESS_START.lock().is_some() {
                let binding = current_binding();
                let (ctrl, alt, shift, win) = modifiers_from_flags(flags);
                let released = (binding.ctrl && !ctrl)
                    || (binding.alt && !alt)
                    || (binding.shift && !shift)
                    || (binding.win && !win);
                if released {
                    finish_press();
                }
            }
            event
        }
        K_CG_EVENT_KEY_DOWN | K_CG_EVENT_KEY_UP => {
            let keycode = CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) as u16;
            let Some(vk) = mac_keycode_to_vk(keycode) else {
                return event;
            };
            let is_down = event_type == K_CG_EVENT_KEY_DOWN;
            let is_up = event_type == K_CG_EVENT_KEY_UP;
            let is_repeat = CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0;

            PHYSICAL_DOWN[usize::from(vk & 0xFF)].store(is_down, Ordering::SeqCst);

            if CAPTURE.load(Ordering::SeqCst) {
                if capture_expired() {
                    end_capture();
                    send_action(HotkeyAction::CaptureCancelled);
                } else if is_down && vk == 0x1B {
                    end_capture();
                    send_action(HotkeyAction::CaptureCancelled);
                    return std::ptr::null_mut();
                } else if !crate::keys::is_modifier(vk) && (is_down || is_up) {
                    if is_down {
                        let (ctrl, alt, shift, win) = modifiers_from_flags(flags);
                        finish_capture(HotkeyBinding {
                            ctrl,
                            alt,
                            shift,
                            win,
                            vk,
                        });
                    }
                    return std::ptr::null_mut();
                }
                return event;
            }

            let binding = current_binding();
            if is_down && vk == binding.vk {
                if modifiers_match_flags(binding, flags) {
                    if !is_repeat {
                        start_press();
                    }
                    SHORTCUT_DOWN.store(true, Ordering::SeqCst);
                    return std::ptr::null_mut();
                }
                if is_repeat && SHORTCUT_DOWN.load(Ordering::SeqCst) {
                    return std::ptr::null_mut();
                }
            }
            if is_down
                && !is_repeat
                && vk > 0x06
                && !crate::keys::is_modifier(vk)
                && vk != binding.vk
            {
                note_user_typed(vk);
            }
            if is_up && vk == binding.vk {
                let kept = SHORTCUT_DOWN.swap(false, Ordering::SeqCst);
                if PRESS_START.lock().is_some() {
                    finish_press();
                    return std::ptr::null_mut();
                }
                if kept {
                    return std::ptr::null_mut();
                }
            }
            event
        }
        K_CG_EVENT_OTHER_MOUSE_DOWN | K_CG_EVENT_OTHER_MOUSE_UP => {
            let button = CGEventGetIntegerValueField(event, K_CG_MOUSE_EVENT_BUTTON_NUMBER);
            let vk = match button {
                3 => Some(VK_MOUSE4),
                4 => Some(VK_MOUSE5),
                _ => None,
            };
            let Some(vk) = vk else {
                return event;
            };
            let is_down = event_type == K_CG_EVENT_OTHER_MOUSE_DOWN;
            if is_down {
                if CAPTURE.load(Ordering::SeqCst) {
                    if capture_expired() {
                        end_capture();
                        send_action(HotkeyAction::CaptureCancelled);
                        return event;
                    }
                    let (ctrl, alt, shift, win) = modifiers_from_flags(flags);
                    finish_capture(HotkeyBinding {
                        ctrl,
                        alt,
                        shift,
                        win,
                        vk,
                    });
                    return std::ptr::null_mut();
                }
                let binding = current_binding();
                if vk == binding.vk && modifiers_match_flags(binding, flags) {
                    start_press();
                    return std::ptr::null_mut();
                }
            } else if current_binding().vk == vk && PRESS_START.lock().is_some() {
                finish_press();
                return std::ptr::null_mut();
            }
            event
        }
        _ => event,
    }
}

pub struct HotkeyListener {
    running: Arc<AtomicBool>,
}

impl HotkeyListener {
    pub fn start() -> Result<(Self, Receiver<HotkeyAction>), String> {
        let (sender, receiver) = async_channel::unbounded();
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        let running = Arc::new(AtomicBool::new(true));
        {
            let mut guard = SENDER.lock();
            if guard.is_some() {
                return Err("Hotkey listener is already running.".to_string());
            }
            *guard = Some(sender);
        }

        std::thread::spawn(move || {
            crate::priority::raise_thread();
            let mask: CGEventMask = (1 << K_CG_EVENT_KEY_DOWN)
                | (1 << K_CG_EVENT_KEY_UP)
                | (1 << K_CG_EVENT_FLAGS_CHANGED)
                | (1 << K_CG_EVENT_OTHER_MOUSE_DOWN)
                | (1 << K_CG_EVENT_OTHER_MOUSE_UP);
            // SAFETY: CoreGraphics/CoreFoundation event tap lifecycle on this dedicated thread.
            unsafe {
                let tap = CGEventTapCreate(
                    K_CG_SESSION_EVENT_TAP,
                    K_CG_HEAD_INSERT_EVENT_TAP,
                    K_CG_EVENT_TAP_OPTION_DEFAULT,
                    mask,
                    event_tap_callback,
                    std::ptr::null_mut(),
                );
                if tap.is_null() {
                    let _ = ready_tx.send(Err(
                        "TDT needs Accessibility permission for the global shortcut. Enable TDT in System Settings > Privacy & Security > Accessibility, then restart TDT."
                            .to_string(),
                    ));
                    return;
                }
                let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
                if source.is_null() {
                    CFRelease(tap);
                    let _ = ready_tx.send(Err(
                        "Could not create the macOS event loop source for the shortcut."
                            .to_string(),
                    ));
                    return;
                }
                let run_loop = CFRunLoopGetCurrent();
                TAP_REF.store(tap, Ordering::SeqCst);
                RUN_LOOP_REF.store(run_loop, Ordering::SeqCst);
                CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
                CGEventTapEnable(tap, true);
                let _ = ready_tx.send(Ok(()));
                CFRunLoopRun();
                TAP_REF.store(std::ptr::null_mut(), Ordering::SeqCst);
                RUN_LOOP_REF.store(std::ptr::null_mut(), Ordering::SeqCst);
                CFRelease(source);
                CFRelease(tap);
            }
        });

        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok((Self { running }, receiver)),
            failed => {
                drop(Self { running });
                *SENDER.lock() = None;
                Err(match failed {
                    Ok(Err(error)) => error,
                    _ => "Could not start the macOS shortcut listener.".to_string(),
                })
            }
        }
    }
}

impl Drop for HotkeyListener {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        let rl = RUN_LOOP_REF.load(Ordering::SeqCst);
        if !rl.is_null() {
            // SAFETY: `CFRunLoopStop` is thread-safe for a live run loop.
            unsafe { CFRunLoopStop(rl) };
        }
        *SENDER.lock() = None;
    }
}

/// Maps macOS Carbon virtual key codes (`CGKeyCode`) to TDT's canonical VK codes.
fn mac_keycode_to_vk(code: u16) -> Option<u16> {
    Some(match code {
        // Letters (ANSI)
        0x00 => b'A' as u16,
        0x0B => b'B' as u16,
        0x08 => b'C' as u16,
        0x02 => b'D' as u16,
        0x0E => b'E' as u16,
        0x03 => b'F' as u16,
        0x05 => b'G' as u16,
        0x04 => b'H' as u16,
        0x22 => b'I' as u16,
        0x26 => b'J' as u16,
        0x28 => b'K' as u16,
        0x25 => b'L' as u16,
        0x2E => b'M' as u16,
        0x2D => b'N' as u16,
        0x1F => b'O' as u16,
        0x23 => b'P' as u16,
        0x0C => b'Q' as u16,
        0x0F => b'R' as u16,
        0x01 => b'S' as u16,
        0x11 => b'T' as u16,
        0x20 => b'U' as u16,
        0x09 => b'V' as u16,
        0x0D => b'W' as u16,
        0x07 => b'X' as u16,
        0x10 => b'Y' as u16,
        0x06 => b'Z' as u16,
        // Digits
        0x1D => b'0' as u16,
        0x12 => b'1' as u16,
        0x13 => b'2' as u16,
        0x14 => b'3' as u16,
        0x15 => b'4' as u16,
        0x17 => b'5' as u16,
        0x16 => b'6' as u16,
        0x1A => b'7' as u16,
        0x1C => b'8' as u16,
        0x19 => b'9' as u16,
        // Control & punctuation
        0x33 => 0x08,        // Backspace
        0x30 => 0x09,        // Tab
        0x24 | 0x4C => 0x0D, // Return / Enter
        0x35 => 0x1B,        // Escape
        0x31 => 0x20,        // Space
        0x29 => 0xBA,        // ;
        0x18 => 0xBB,        // =
        0x2B => 0xBC,        // ,
        0x1B => 0xBD,        // -
        0x2F => 0xBE,        // .
        0x2C => 0xBF,        // /
        0x32 => 0xC0,        // `
        0x21 => 0xDB,        // [
        0x2A => 0xDC,        // \
        0x1E => 0xDD,        // ]
        0x27 => 0xDE,        // '
        // Function keys
        0x7A => 0x70, // F1
        0x78 => 0x71, // F2
        0x63 => 0x72, // F3
        0x76 => 0x73, // F4
        0x60 => 0x74, // F5
        0x61 => 0x75, // F6
        0x62 => 0x76, // F7
        0x64 => 0x77, // F8
        0x65 => 0x78, // F9
        0x6D => 0x79, // F10
        0x67 => 0x7A, // F11
        0x6F => 0x7B, // F12
        0x69 => 0x7C, // F13
        0x6B => 0x7D, // F14
        0x71 => 0x7E, // F15
        0x6A => 0x7F, // F16
        0x40 => 0x80, // F17
        0x4F => 0x81, // F18
        0x50 => 0x82, // F19
        0x5A => 0x83, // F20
        _ => return None,
    })
}

const KEY_NAMES: &[(u16, &str, &[&str])] = &[
    (VK_MOUSE4, "Mouse 4", &["mouse4", "xbutton1"]),
    (VK_MOUSE5, "Mouse 5", &["mouse5", "xbutton2"]),
    (0x08, "Backspace", &["backspace"]),
    (0x09, "Tab", &["tab"]),
    (0x0D, "Enter", &["enter", "return"]),
    (0x20, "Space", &["space"]),
    (0xBA, ";", &[";", "semicolon"]),
    (0xBB, "=", &["=", "plus", "equal"]),
    (0xBC, ",", &[",", "comma"]),
    (0xBD, "-", &["-", "minus"]),
    (0xBE, ".", &[".", "period", "dot"]),
    (0xBF, "/", &["/", "slash"]),
    (0xC0, "`", &["`", "grave"]),
    (0xDB, "[", &["["]),
    (0xDC, "\\", &["\\", "backslash"]),
    (0xDD, "]", &["]"]),
    (0xDE, "'", &["'", "quote"]),
];

fn parse_vk(token: &str) -> Option<u16> {
    let lower = token.trim().to_ascii_lowercase().replace(' ', "");
    if let Some((vk, ..)) = KEY_NAMES
        .iter()
        .find(|(_, _, names)| names.contains(&lower.as_str()))
    {
        return Some(*vk);
    }
    if let Some(num) = lower.strip_prefix('f') {
        let n: u16 = num.parse().ok()?;
        if (1..=24).contains(&n) {
            return Some(0x6F + n);
        }
    }
    let mut chars = lower.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) if ch.is_ascii_alphanumeric() => Some(ch.to_ascii_uppercase() as u16),
        _ => None,
    }
}

fn vk_label(vk: u16) -> String {
    if let Some((_, label, _)) = KEY_NAMES.iter().find(|(key, ..)| *key == vk) {
        return (*label).to_string();
    }
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from(vk as u8).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => format!("Vk{vk:02X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display_macos_modifiers() {
        let binding = HotkeyBinding::parse("Cmd+Option+;").expect("valid macOS shortcut");
        assert!(binding.win);
        assert!(binding.alt);
        assert_eq!(binding.vk, 0xBA);
        assert_eq!(binding.display(), "Option+Cmd+;");
    }

    #[test]
    fn default_shortcut_roundtrips_with_mac_keycode() {
        assert_eq!(mac_keycode_to_vk(0x29), Some(HotkeyBinding::DEFAULT.vk));
        assert_eq!(HotkeyBinding::parse("Ctrl+;"), Some(HotkeyBinding::DEFAULT));
    }
}
