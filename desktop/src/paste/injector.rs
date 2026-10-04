use arboard::Clipboard;
use parking_lot::{Condvar, Mutex};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// What the final transcript should do to text already inserted at key-up.
#[derive(Debug, PartialEq, Eq)]
pub enum FinalEdit {
    /// Final text is empty. `erase` is set when a partial was inserted.
    NoSpeech { erase: Option<String> },
    /// Copy only. Auto-paste is off.
    CopyOnly,
    /// Nothing was inserted early. Copy and type the whole final text.
    TypeAll,
    /// The insertion already matches the final text. Refresh the clipboard only.
    Keep,
    /// Final text continues the insertion. Type this tail.
    Extend(String),
    /// The hypothesis changed. Delete `erase`, then type `text`.
    Replace { erase: String, text: String },
}

/// `inserted` is the exact string typed at key-up. `None` means the document was not changed.
pub fn final_edit(inserted: Option<&str>, final_text: &str, should_paste: bool) -> FinalEdit {
    let final_text = final_text.trim();
    if final_text.is_empty() {
        return FinalEdit::NoSpeech {
            erase: inserted.map(str::to_string),
        };
    }
    // Auto-paste off still corrects text that was already typed at key-up.
    if !should_paste && inserted.is_none() {
        return FinalEdit::CopyOnly;
    }
    let Some(inserted) = inserted else {
        return FinalEdit::TypeAll;
    };
    if let Some(rest) = final_text.strip_prefix(inserted) {
        return if rest.is_empty() {
            FinalEdit::Keep
        } else {
            FinalEdit::Extend(rest.to_string())
        };
    }
    FinalEdit::Replace {
        erase: inserted.to_string(),
        text: final_text.to_string(),
    }
}

/// Keys actually sent for `text`. Carriage returns are skipped, matching insertion.
pub fn inserted_key_count(text: &str) -> usize {
    text.chars().filter(|character| *character != '\r').count()
}

/// Modifier virtual keys to release before Unicode injection. Order is stable.
const MODIFIER_VKS: [u16; 11] = [
    0x10, 0x11, 0x12, 0x5B, 0x5C, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5,
];

pub fn modifier_vks_to_release(down: impl Fn(u16) -> bool) -> Vec<u16> {
    MODIFIER_VKS.into_iter().filter(|vk| down(*vk)).collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum EarlyClaim {
    /// No early insert ran. Type the final text normally.
    None,
    /// An early insert was attempted and did not land in the document.
    NotInserted,
    /// This exact string is already in the document.
    Inserted(String),
}

enum PastePhase {
    Idle,
    Running,
    Done { text: String, inserted: bool },
    Closed,
}

/// One early insert per utterance, then the final pass waits for it.
pub struct PasteGate {
    phase: Mutex<PastePhase>,
    cv: Condvar,
}

impl PasteGate {
    pub fn new() -> Self {
        Self {
            phase: Mutex::new(PastePhase::Idle),
            cv: Condvar::new(),
        }
    }

    /// False when an early insert is already in flight or the final pass has claimed the slot.
    pub fn begin(&self) -> bool {
        let mut phase = self.phase.lock();
        if !matches!(*phase, PastePhase::Idle) {
            return false;
        }
        *phase = PastePhase::Running;
        true
    }

    pub fn finish(&self, text: String, inserted: bool) {
        let mut phase = self.phase.lock();
        *phase = PastePhase::Done { text, inserted };
        self.cv.notify_all();
    }

    pub fn claim_final(&self) -> EarlyClaim {
        let mut phase = self.phase.lock();
        loop {
            match &*phase {
                PastePhase::Idle => {
                    *phase = PastePhase::Closed;
                    return EarlyClaim::None;
                }
                PastePhase::Done { text, inserted } => {
                    let claim = if *inserted {
                        EarlyClaim::Inserted(text.clone())
                    } else {
                        EarlyClaim::NotInserted
                    };
                    *phase = PastePhase::Closed;
                    return claim;
                }
                PastePhase::Closed => return EarlyClaim::None,
                PastePhase::Running => self.cv.wait(&mut phase),
            }
        }
    }
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
    CopiedFallback(String),
}

/// True once the window's input focus is live, meaning synthesized keystrokes
/// will land in a real edit control rather than being dropped.
#[cfg(target_os = "windows")]
fn has_input_focus(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO,
    };
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
    insert: impl FnOnce(&str) -> Result<(), String>,
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
                        last_err = format!("Clipboard error: {}", e);
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

    /// Apply the final transcript after an optional key-up insert.
    pub fn apply_final(
        &self,
        final_text: &str,
        should_paste: bool,
        target_hwnd: Option<isize>,
        claim: EarlyClaim,
        user_typed: bool,
    ) -> Result<DeliveryOutcome, String> {
        let inserted = match claim {
            EarlyClaim::Inserted(text) => Some(text),
            EarlyClaim::None | EarlyClaim::NotInserted => None,
        };
        let edit = final_edit(inserted.as_deref(), final_text, should_paste);
        if user_typed {
            return match edit {
                FinalEdit::NoSpeech { .. } => Ok(DeliveryOutcome::NoSpeech),
                _ => {
                    self.copy_to_clipboard(final_text.trim())?;
                    Ok(DeliveryOutcome::CopiedFallback(
                        "You typed while TDT was finishing. The final text is on the clipboard."
                            .to_string(),
                    ))
                }
            };
        }
        match edit {
            FinalEdit::NoSpeech { erase } => {
                if let Some(erase) = erase {
                    let _ = self.replace_typed(&erase, "", target_hwnd);
                }
                Ok(DeliveryOutcome::NoSpeech)
            }
            FinalEdit::CopyOnly => self.deliver(final_text, false, target_hwnd),
            FinalEdit::TypeAll => self.deliver(final_text, true, target_hwnd),
            FinalEdit::Keep => {
                self.copy_to_clipboard(final_text.trim())?;
                Ok(DeliveryOutcome::Pasted)
            }
            FinalEdit::Extend(suffix) => {
                self.copy_to_clipboard(final_text.trim())?;
                match self.insert_text(&suffix, target_hwnd) {
                    Ok(()) => Ok(DeliveryOutcome::Pasted),
                    Err(error) => Ok(DeliveryOutcome::CopiedFallback(error)),
                }
            }
            FinalEdit::Replace { erase, text } => {
                self.copy_to_clipboard(&text)?;
                match self.replace_typed(&erase, &text, target_hwnd) {
                    Ok(()) => Ok(DeliveryOutcome::Pasted),
                    Err(error) => Ok(DeliveryOutcome::CopiedFallback(error)),
                }
            }
        }
    }

    fn insert_text(&self, text: &str, target_hwnd: Option<isize>) -> Result<(), String> {
        self.prepare_target(target_hwnd)?;
        self.type_prepared(text)
    }

    fn replace_typed(
        &self,
        previous: &str,
        next: &str,
        target_hwnd: Option<isize>,
    ) -> Result<(), String> {
        self.prepare_target(target_hwnd)?;
        #[cfg(target_os = "windows")]
        {
            backspace_windows(inserted_key_count(previous))?;
            if !next.is_empty() {
                type_text_windows(next)?;
            }
            Ok(())
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (previous, next);
            Ok(())
        }
    }

    fn prepare_target(&self, target_hwnd: Option<isize>) -> Result<(), String> {
        if target_hwnd.is_none_or(|handle| handle == 0) {
            return Err("The destination app is no longer available.".into());
        }

        // Bring the original target back to the foreground.
        #[cfg(target_os = "windows")]
        if let Some(raw) = target_hwnd {
            if raw != 0 {
                use windows::Win32::Foundation::HWND;
                use windows::Win32::UI::WindowsAndMessaging::{
                    BringWindowToTop, GetForegroundWindow, SetForegroundWindow,
                };
                unsafe {
                    let hwnd = HWND(raw as _);
                    let _ = SetForegroundWindow(hwnd);
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
                        return Err(
                            "Could not restore the target application focus; text remains on the clipboard"
                                .to_string(),
                        );
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

    fn type_prepared(&self, text: &str) -> Result<(), String> {
        // Inject Unicode text directly. A synthetic Ctrl+V is interpreted as an image
        // paste by some apps, even when CF_UNICODETEXT is present.
        #[cfg(target_os = "windows")]
        {
            type_text_windows(text)?;
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
fn backspace_windows(count: usize) -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_BACK;

    release_physical_modifiers();
    let mut inputs = Vec::with_capacity(count * 2);
    for _ in 0..count {
        push_key(&mut inputs, VK_BACK);
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

#[cfg(test)]
mod revision_tests {
    use super::{
        final_edit, inserted_key_count, modifier_vks_to_release, EarlyClaim, FinalEdit, PasteGate,
    };
    use std::sync::Arc;

    #[test]
    fn identical_text_is_left_in_place() {
        assert_eq!(final_edit(Some("hello"), "hello", true), FinalEdit::Keep);
    }

    #[test]
    fn a_longer_final_types_only_the_tail() {
        assert_eq!(
            final_edit(Some("hello"), "hello world", true),
            FinalEdit::Extend(" world".to_string())
        );
    }

    #[test]
    fn a_rewritten_final_replaces_the_insertion() {
        assert_eq!(
            final_edit(Some("hello word"), "hello world", true),
            FinalEdit::Replace {
                erase: "hello word".to_string(),
                text: "hello world".to_string(),
            }
        );
    }

    #[test]
    fn no_early_insert_types_the_whole_final() {
        assert_eq!(final_edit(None, "hello", true), FinalEdit::TypeAll);
        assert_eq!(final_edit(None, "hello", false), FinalEdit::CopyOnly);
    }

    #[test]
    fn empty_final_erases_an_insertion_and_ignores_a_miss() {
        assert_eq!(
            final_edit(Some("hello"), "  ", true),
            FinalEdit::NoSpeech {
                erase: Some("hello".to_string()),
            }
        );
        assert_eq!(
            final_edit(None, "", true),
            FinalEdit::NoSpeech { erase: None }
        );
    }

    #[test]
    fn held_ctrl_is_released_before_typing() {
        let mut down = [false; 256];
        assert!(modifier_vks_to_release(|vk| down[vk as usize]).is_empty());
        down[0x11] = true;
        down[0xA2] = true;
        assert_eq!(
            modifier_vks_to_release(|vk| down[vk as usize]),
            vec![0x11, 0xA2]
        );
    }

    #[test]
    fn key_count_skips_carriage_returns_only() {
        assert_eq!(inserted_key_count("a\r\nb"), 3);
        assert_eq!(inserted_key_count(""), 0);
    }

    #[test]
    fn claim_before_an_early_insert_types_everything() {
        let gate = PasteGate::new();
        assert_eq!(gate.claim_final(), EarlyClaim::None);
        assert!(!gate.begin());
    }

    #[test]
    fn a_failed_early_insert_still_types_the_final() {
        let gate = PasteGate::new();
        assert!(gate.begin());
        assert!(!gate.begin());
        gate.finish("hi".into(), false);
        assert_eq!(gate.claim_final(), EarlyClaim::NotInserted);
    }

    #[test]
    fn claim_waits_until_the_early_insert_finishes() {
        let gate = Arc::new(PasteGate::new());
        assert!(gate.begin());
        let waiting = Arc::clone(&gate);
        let worker = std::thread::spawn(move || waiting.claim_final());
        gate.finish("hello".into(), true);
        assert_eq!(
            worker.join().expect("claim"),
            EarlyClaim::Inserted("hello".into())
        );
    }
}

#[cfg(test)]
mod delivery_tests {
    use super::{deliver_with, DeliveryOutcome};

    #[test]
    fn silence_does_not_touch_clipboard_or_insert() {
        assert_eq!(
            deliver_with(" \n", true, |_| panic!("copy"), |_| panic!("insert")),
            Ok(DeliveryOutcome::NoSpeech)
        );
    }

    #[test]
    fn clipboard_failure_never_attempts_insertion() {
        assert_eq!(
            deliver_with(
                "words",
                true,
                |_| Err("clipboard busy".into()),
                |_| panic!("insert")
            ),
            Err("clipboard busy".into())
        );
    }

    #[test]
    fn insertion_failure_preserves_successful_copy() {
        assert_eq!(
            deliver_with("words", true, |_| Ok(()), |_| Err("focus lost".into())),
            Ok(DeliveryOutcome::CopiedFallback("focus lost".into()))
        );
    }

    #[test]
    fn copy_only_never_inserts_and_successful_insertion_is_pasted() {
        assert_eq!(
            deliver_with("words", false, |_| Ok(()), |_| panic!("insert")),
            Ok(DeliveryOutcome::Copied)
        );
        assert_eq!(
            deliver_with("words", true, |_| Ok(()), |_| Ok(())),
            Ok(DeliveryOutcome::Pasted)
        );
    }
}
