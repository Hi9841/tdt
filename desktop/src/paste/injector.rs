use arboard::Clipboard;
use parking_lot::Mutex;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// The longest wait for a restored window to accept text. An app that never
/// reports an input focus waits this long.
const INPUT_READY_BUDGET: Duration = Duration::from_millis(60);
/// Granularity of the readiness poll. Small enough that a late-ready window is
/// still caught early, large enough not to spin.
const INPUT_READY_STEP: Duration = Duration::from_millis(4);

/// Characters in one `SendInput` call. A short pause follows each call but
/// the last, so the app can read its input before more comes.
const TYPED_AT_ONCE: usize = 256;
const PAUSE_BETWEEN_CALLS: Duration = Duration::from_millis(1);

/// Where the delivery time went, for the log.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeliverySteps {
    pub copy: Duration,
    pub focus: Duration,
    pub typing: Duration,
    /// When the text was in the app. The wait of the user ends here.
    pub typed_at: Option<Instant>,
}

impl std::fmt::Display for DeliverySteps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "copy {}, focus {}, typing {}",
            self.copy.as_millis(),
            self.focus.as_millis(),
            self.typing.as_millis()
        )
    }
}

#[derive(Debug, PartialEq)]
pub enum DeliveryOutcome {
    NoSpeech,
    Copied,
    Pasted,
    /// The text is in the app, but the clipboard did not take it.
    PastedNotCopied(String),
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

fn wait_for_input_focus(hwnd: windows::Win32::Foundation::HWND) {
    let started = Instant::now();
    while started.elapsed() < INPUT_READY_BUDGET {
        if has_input_focus(hwnd) {
            break;
        }
        thread::sleep(INPUT_READY_STEP);
    }
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
                        // Another app can hold the clipboard for a moment.
                        *guard = None;
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

    /// Puts the final transcript on the clipboard, and types it into
    /// `target_hwnd` when auto-paste is on. `user_typed` keeps TDT from typing
    /// over keys that the user pressed while the transcript was finishing.
    pub fn deliver(
        &self,
        text: &str,
        should_paste: bool,
        target_hwnd: Option<isize>,
        user_typed: bool,
        steps: &mut DeliverySteps,
    ) -> Result<DeliveryOutcome, String> {
        if text.trim().is_empty() {
            return Ok(DeliveryOutcome::NoSpeech);
        }
        if user_typed {
            self.copy_to_clipboard(text.trim())?;
            return Ok(DeliveryOutcome::CopiedFallback(NotTyped::UserTyped));
        }
        if !should_paste {
            self.copy_timed(text, steps)?;
            return Ok(DeliveryOutcome::Copied);
        }
        // The app gets the text first. A clipboard manager can hold the copy for
        // a time, and the user does not wait for the clipboard.
        let typed = self.insert_text(text, target_hwnd, steps);
        let copied = self.copy_timed(text, steps);
        match (typed, copied) {
            (Ok(()), Ok(())) => Ok(DeliveryOutcome::Pasted),
            (Ok(()), Err(error)) => Ok(DeliveryOutcome::PastedNotCopied(error)),
            (Err(reason), Ok(())) => Ok(DeliveryOutcome::CopiedFallback(reason)),
            (Err(_), Err(error)) => Err(error),
        }
    }

    fn copy_timed(&self, text: &str, steps: &mut DeliverySteps) -> Result<(), String> {
        let started = Instant::now();
        let copied = self.copy_to_clipboard(text);
        steps.copy = started.elapsed();
        copied
    }

    fn insert_text(
        &self,
        text: &str,
        target_hwnd: Option<isize>,
        steps: &mut DeliverySteps,
    ) -> Result<(), NotTyped> {
        let started = Instant::now();
        let prepared = self.prepare_target(target_hwnd);
        steps.focus = started.elapsed();
        prepared?;
        let started = Instant::now();
        // Unicode key events, not Ctrl+V: some apps paste a synthetic Ctrl+V
        // as an image, even when the clipboard holds text.
        let typed = type_text_windows(text).map_err(NotTyped::Partial);
        steps.typing = started.elapsed();
        if typed.is_ok() {
            steps.typed_at = Some(Instant::now());
        }
        typed
    }

    /// Brings the window of the dictation back to the front.
    fn prepare_target(&self, target_hwnd: Option<isize>) -> Result<(), NotTyped> {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            BringWindowToTop, GetForegroundWindow, SetForegroundWindow,
        };
        let Some(raw) = target_hwnd else {
            return Err(NotTyped::NoFocus("no target window"));
        };
        // Windows would drop the keys and report success, so do not
        // claim a paste that cannot happen.
        if super::Target::of(raw).blocks_input() {
            return Err(NotTyped::Elevated);
        }
        let hwnd = HWND(raw as _);
        // SAFETY: foreground calls on the caller's target handle; a stale handle makes a call fail, not misbehave.
        let foreground_allowed = unsafe {
            // SetForegroundWindow is frequently denied to background
            // apps; the loop below is the real check.
            let allowed = SetForegroundWindow(hwnd).as_bool();
            let _ = BringWindowToTop(hwnd);
            allowed
        };
        for _ in 0..8 {
            // SAFETY: GetForegroundWindow takes no arguments.
            if unsafe { GetForegroundWindow() } == hwnd {
                // The `focus` time of the delivery line in the log holds this wait.
                wait_for_input_focus(hwnd);
                return Ok(());
            }
            thread::sleep(Duration::from_millis(15));
        }
        Err(NotTyped::NoFocus(if foreground_allowed {
            "the window did not come to the front"
        } else {
            "Windows refused the focus"
        }))
    }
}

fn release_physical_modifiers() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, KEYEVENTF_KEYUP};

    let down: Vec<u16> = crate::keys::MODIFIERS
        .into_iter()
        // SAFETY: GetAsyncKeyState takes no pointers.
        .filter(|vk| unsafe { GetAsyncKeyState(i32::from(*vk)) as u16 & 0x8000 != 0 })
        .collect();
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
                dwExtraInfo: crate::keys::OWN_INPUT,
            },
        },
    }
}

fn push_key(
    inputs: &mut Vec<windows::Win32::UI::Input::KeyboardAndMouse::INPUT>,
    virtual_key: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP};
    inputs.push(keyboard_input(virtual_key, 0, KEYBD_EVENT_FLAGS::default()));
    inputs.push(keyboard_input(virtual_key, 0, KEYEVENTF_KEYUP));
}

fn send_inputs(
    inputs: &[windows::Win32::UI::Input::KeyboardAndMouse::INPUT],
) -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::SendInput;
    // Each character is a key down and a key up.
    for (index, chunk) in inputs.chunks(2 * TYPED_AT_ONCE).enumerate() {
        if index > 0 {
            thread::sleep(PAUSE_BETWEEN_CALLS);
        }
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
