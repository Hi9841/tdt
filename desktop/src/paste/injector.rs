use arboard::Clipboard;
use parking_lot::Mutex;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Modifier virtual keys to release before Unicode injection. Order is stable.
const MODIFIER_VKS: [u16; 11] = [
    0x10, 0x11, 0x12, 0x5B, 0x5C, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5,
];

pub fn modifier_vks_to_release(down: impl Fn(u16) -> bool) -> Vec<u16> {
    MODIFIER_VKS.into_iter().filter(|vk| down(*vk)).collect()
}

/// Longest we will wait for a restored window to become able to accept text.
///
/// This replaces a fixed sleep that every dictation paid whether or not the
/// window was already ready, so it is the ceiling, not the common case. An
/// app that never reports input focus waits exactly as long as it used to.
const INPUT_READY_BUDGET: Duration = Duration::from_millis(60);
/// Granularity of the readiness poll. Small enough that a late-ready window is
/// still caught early, large enough not to spin.
const INPUT_READY_STEP: Duration = Duration::from_millis(4);

#[derive(Debug, PartialEq)]
pub enum DeliveryOutcome {
    NoSpeech,
    Copied,
    Pasted,
    CopiedFallback(NotTyped),
}

/// Why the text is on the clipboard but not in the app.
#[derive(Debug, PartialEq)]
pub enum NotTyped {
    /// A key or mouse button went down while TDT was finishing.
    UserTyped,
    /// The app runs as administrator. Windows drops TDT's keys and reports no error.
    Elevated,
    /// The app is gone, or Windows did not give it the focus.
    NoFocus(&'static str),
    /// Windows refused some of the key events.
    Partial(String),
}

impl NotTyped {
    /// What the pill tells the user.
    pub fn notice(&self) -> &'static str {
        match self {
            Self::UserTyped => {
                "You typed while TDT was finishing. The final text is on the clipboard."
            }
            Self::Elevated => {
                "This app runs as administrator, so Windows does not let TDT type into it. \
                 Press Ctrl+V to paste the text."
            }
            Self::NoFocus(_) => {
                "TDT could not go back to the app. Click in the app and press Ctrl+V to paste the text."
            }
            Self::Partial(_) => {
                "Copied. Could not insert all text. Check the destination before pasting with Ctrl+V."
            }
        }
    }
}

impl std::fmt::Display for NotTyped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserTyped => f.write_str("the user typed"),
            Self::Elevated => f.write_str("the app runs as administrator"),
            Self::NoFocus(detail) => f.write_str(detail),
            Self::Partial(detail) => f.write_str(detail),
        }
    }
}

/// True once the window's input focus is live, meaning synthesized keystrokes
/// will land in a real edit control rather than being dropped.
#[cfg(target_os = "windows")]
fn has_input_focus(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO,
    };
    // SAFETY: `info` has `cbSize` set and outlives the call. a stale handle makes a call fail, not misbehave.
    unsafe {
        let thread = GetWindowThreadProcessId(hwnd, None);
        if thread == 0 {
            return false;
        }
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        // hwndFocus is the window itself when it owns focus directly, so this
        // also covers apps with no child edit control.
        GetGUIThreadInfo(thread, &mut info).is_ok() && !info.hwndFocus.is_invalid()
    }
}

#[cfg(target_os = "windows")]
fn wait_for_input_focus(hwnd: windows::Win32::Foundation::HWND) -> Duration {
    let started = std::time::Instant::now();
    while started.elapsed() < INPUT_READY_BUDGET {
        if has_input_focus(hwnd) {
            break;
        }
        thread::sleep(INPUT_READY_STEP);
    }
    started.elapsed()
}

fn deliver_with(
    text: &str,
    should_paste: bool,
    copy: impl FnOnce(&str) -> Result<(), String>,
    insert: impl FnOnce(&str) -> Result<(), NotTyped>,
) -> Result<DeliveryOutcome, String> {
    if text.trim().is_empty() {
        return Ok(DeliveryOutcome::NoSpeech);
    }
    copy(text)?;
    if !should_paste {
        return Ok(DeliveryOutcome::Copied);
    }
    Ok(match insert(text) {
        Ok(()) => DeliveryOutcome::Pasted,
        Err(error) => DeliveryOutcome::CopiedFallback(error),
    })
}

#[derive(Clone)]
pub struct PasteInjector {
    clipboard: Arc<Mutex<Option<Clipboard>>>,
}

impl PasteInjector {
    pub fn new() -> Self {
        let clipboard = Clipboard::new().ok();
        Self {
            clipboard: Arc::new(Mutex::new(clipboard)),
        }
    }

    pub fn copy_to_clipboard(&self, text: &str) -> Result<(), String> {
        // The transcript intentionally replaces the clipboard: copy-to-
        // clipboard is a documented feature, not a side effect, so the
        // previous contents are never restored.
        let mut last_err = String::new();
        for _ in 0..4 {
            let mut guard = self.clipboard.lock();
            if guard.is_none() {
                *guard = Clipboard::new().ok();
            }

            if let Some(ref mut cb) = *guard {
                match cb.set_text(text) {
                    Ok(_) => return Ok(()),
                    Err(e) => {
                        last_err = format!("Clipboard error: {e}");
                        *guard = None; // Reset clipboard handle and retry
                    }
                }
            } else {
                last_err = "Failed to open system clipboard".to_string();
            }
            drop(guard);
            thread::sleep(Duration::from_millis(25));
        }
        Err(last_err)
    }

    pub fn deliver(
        &self,
        text: &str,
        should_paste: bool,
        target_hwnd: Option<isize>,
    ) -> Result<DeliveryOutcome, String> {
        deliver_with(
            text,
            should_paste,
            |text| self.copy_to_clipboard(text),
            |text| self.insert_text(text, target_hwnd),
        )
    }

    /// Apply the final transcript: copy it, and type it when auto-paste is on.
    /// `user_typed` keeps us from typing over keys the user pressed while the
    /// transcript was finishing.
    pub fn apply_final(
        &self,
        final_text: &str,
        should_paste: bool,
        target_hwnd: Option<isize>,
        user_typed: bool,
    ) -> Result<DeliveryOutcome, String> {
        if final_text.trim().is_empty() {
            return Ok(DeliveryOutcome::NoSpeech);
        }
        if user_typed {
            self.copy_to_clipboard(final_text.trim())?;
            return Ok(DeliveryOutcome::CopiedFallback(NotTyped::UserTyped));
        }
        self.deliver(final_text, should_paste, target_hwnd)
    }

    fn insert_text(&self, text: &str, target_hwnd: Option<isize>) -> Result<(), NotTyped> {
        self.prepare_target(target_hwnd)?;
        self.type_prepared(text)
    }

    fn prepare_target(&self, target_hwnd: Option<isize>) -> Result<(), NotTyped> {
        if target_hwnd.is_none_or(|handle| handle == 0) {
            return Err(NotTyped::NoFocus("no target window"));
        }

        // Bring the original target back to the foreground.
        #[cfg(target_os = "windows")]
        if let Some(raw) = target_hwnd {
            if raw != 0 {
                use windows::Win32::Foundation::HWND;
                use windows::Win32::UI::WindowsAndMessaging::{
                    BringWindowToTop, GetForegroundWindow, SetForegroundWindow,
                };
                // Windows would drop the keys and report success, so do not
                // claim a paste that cannot happen.
                if super::Target::of(raw).blocks_input() {
                    return Err(NotTyped::Elevated);
                }
                // SAFETY: foreground calls on the caller's target handle; a stale handle makes a call fail, not misbehave.
                unsafe {
                    let hwnd = HWND(raw as _);
                    // SetForegroundWindow is frequently denied to background
                    // apps; the retry loop below is the real check. A denial
                    // here just means focus arrives via the loop or not at all.
                    let foreground_allowed = SetForegroundWindow(hwnd).as_bool();
                    let _ = BringWindowToTop(hwnd);

                    let mut focused = false;
                    for _ in 0..8 {
                        if GetForegroundWindow() == hwnd {
                            focused = true;
                            break;
                        }
                        thread::sleep(Duration::from_millis(15));
                    }
                    if !focused {
                        return Err(NotTyped::NoFocus(if foreground_allowed {
                            "the window did not come to the front"
                        } else {
                            "Windows refused the focus"
                        }));
                    }

                    // Wait for the window to be able to accept text, rather
                    // than sleeping a fixed 60 ms on every dictation. A window
                    // that is already live costs nothing; one that never
                    // becomes live still costs no more than it used to.
                    let waited = wait_for_input_focus(hwnd);
                    if waited > Duration::ZERO {
                        println!("delivery: input focus live after {} ms", waited.as_millis());
                    }
                }
            }
        }

        // Non-Windows targets keep the fixed delay: the readiness probe is a
        // Win32 concept and this path is not shipped.
        #[cfg(not(target_os = "windows"))]
        thread::sleep(INPUT_READY_BUDGET);

        Ok(())
    }

    fn type_prepared(&self, text: &str) -> Result<(), NotTyped> {
        // Inject Unicode text directly. A synthetic Ctrl+V is interpreted as an image
        // paste by some apps, even when CF_UNICODETEXT is present.
        #[cfg(target_os = "windows")]
        {
            type_text_windows(text).map_err(NotTyped::Partial)?;
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = text;
        }

        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn release_physical_modifiers() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, KEYEVENTF_KEYUP};

    // SAFETY: GetAsyncKeyState takes no pointers.
    let down = modifier_vks_to_release(|vk| unsafe {
        GetAsyncKeyState(i32::from(vk)) as u16 & 0x8000 != 0
    });
    if down.is_empty() {
        return;
    }
    let inputs: Vec<_> = down
        .into_iter()
        .map(|vk| {
            keyboard_input(
                windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(vk),
                0,
                KEYEVENTF_KEYUP,
            )
        })
        .collect();
    let _ = send_inputs(&inputs);
}

#[cfg(target_os = "windows")]
fn type_text_windows(text: &str) -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_RETURN, VK_TAB,
    };

    release_physical_modifiers();
    let mut inputs = Vec::with_capacity(text.encode_utf16().count() * 2);
    for character in text.chars() {
        match character {
            '\n' => push_key(&mut inputs, VK_RETURN),
            '\t' => push_key(&mut inputs, VK_TAB),
            '\r' => continue,
            '\0' => return Err("Cannot type text containing a null character".to_string()),
            _ => {
                let mut encoded = [0_u16; 2];
                for &unit in character.encode_utf16(&mut encoded).iter() {
                    inputs.push(keyboard_input(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE));
                    inputs.push(keyboard_input(
                        VIRTUAL_KEY(0),
                        unit,
                        KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    ));
                }
            }
        }
    }
    send_inputs(&inputs)
}

#[cfg(target_os = "windows")]
fn keyboard_input(
    virtual_key: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    scan_code: u16,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS,
) -> windows::Win32::UI::Input::KeyboardAndMouse::INPUT {
    use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT};
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: scan_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

#[cfg(target_os = "windows")]
fn push_key(
    inputs: &mut Vec<windows::Win32::UI::Input::KeyboardAndMouse::INPUT>,
    virtual_key: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP};
    inputs.push(keyboard_input(virtual_key, 0, KEYBD_EVENT_FLAGS::default()));
    inputs.push(keyboard_input(virtual_key, 0, KEYEVENTF_KEYUP));
}

#[cfg(target_os = "windows")]
fn send_inputs(
    inputs: &[windows::Win32::UI::Input::KeyboardAndMouse::INPUT],
) -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::SendInput;
    for chunk in inputs.chunks(512) {
        // SAFETY: `chunk` is a valid slice of INPUT, and the size argument is the size of one INPUT.
        let sent = unsafe { SendInput(chunk, std::mem::size_of_val(&chunk[0]) as i32) };
        if sent != chunk.len() as u32 {
            return Err(format!(
                "Windows accepted {sent} of {} text input events: {}",
                chunk.len(),
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}
