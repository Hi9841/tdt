use arboard::Clipboard;
use parking_lot::Mutex;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

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

/// How long the readiness wait actually cost, and how many times it asked.
#[derive(Debug, PartialEq, Clone, Copy)]
struct ReadinessWait {
    polls: usize,
    waited: Duration,
}

/// Poll `probe` until it reports ready or `budget` is spent, then give up and
/// inject anyway.
///
/// The clock and the sleep are injected so the two properties that matter are
/// unit-testable without a foreground window: a window that is already ready
/// costs nothing, and a window that never becomes ready never costs more than
/// the budget it replaced.
fn wait_for_input_ready_with(
    probe: &mut dyn FnMut() -> bool,
    elapsed: &mut dyn FnMut() -> Duration,
    sleep: &mut dyn FnMut(Duration),
    budget: Duration,
    step: Duration,
) -> ReadinessWait {
    let mut polls = 0;
    let mut waited = Duration::ZERO;
    loop {
        polls += 1;
        if probe() {
            return ReadinessWait { polls, waited };
        }
        let spent = elapsed();
        if spent >= budget {
            return ReadinessWait { polls, waited };
        }
        let nap = step.min(budget - spent);
        sleep(nap);
        waited += nap;
    }
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
fn wait_for_input_focus(hwnd: windows::Win32::Foundation::HWND) -> ReadinessWait {
    let started = std::time::Instant::now();
    let mut elapsed = || started.elapsed();
    let mut sleep = |nap: Duration| thread::sleep(nap);
    wait_for_input_ready_with(
        &mut || has_input_focus(hwnd),
        &mut elapsed,
        &mut sleep,
        INPUT_READY_BUDGET,
        INPUT_READY_STEP,
    )
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

    fn insert_text(&self, text: &str, target_hwnd: Option<isize>) -> Result<(), String> {
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
                    let wait = wait_for_input_focus(hwnd);
                    if wait.waited > Duration::ZERO {
                        println!(
                            "delivery: input focus live after {} ms across {} polls",
                            wait.waited.as_millis(),
                            wait.polls
                        );
                    }
                }
            }
        }

        // Non-Windows targets keep the fixed delay: the readiness probe is a
        // Win32 concept and this path is not shipped.
        #[cfg(not(target_os = "windows"))]
        thread::sleep(INPUT_READY_BUDGET);

        // Inject Unicode text directly. A synthetic Ctrl+V is interpreted as an image
        // paste by some apps (including Codex), even when CF_UNICODETEXT is present.
        #[cfg(target_os = "windows")]
        {
            Self::type_text_windows(text)?;
        }

        #[cfg(not(target_os = "windows"))]
        {
            use enigo::{Direction, Enigo, Key, Keyboard, Settings};
            if let Ok(mut enigo) = Enigo::new(&Settings::default()) {
                let _ = enigo.key(Key::Control, Direction::Press);
                thread::sleep(Duration::from_millis(15));
                let _ = enigo.key(Key::Unicode('v'), Direction::Click);
                thread::sleep(Duration::from_millis(15));
                let _ = enigo.key(Key::Control, Direction::Release);
            }
        }

        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn type_text_windows(text: &str) -> Result<(), String> {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
            KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_RETURN, VK_TAB,
        };

        fn keyboard_input(
            virtual_key: VIRTUAL_KEY,
            scan_code: u16,
            flags: KEYBD_EVENT_FLAGS,
        ) -> INPUT {
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

        fn push_key(inputs: &mut Vec<INPUT>, virtual_key: VIRTUAL_KEY) {
            inputs.push(keyboard_input(virtual_key, 0, KEYBD_EVENT_FLAGS::default()));
            inputs.push(keyboard_input(virtual_key, 0, KEYEVENTF_KEYUP));
        }

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

        for chunk in inputs.chunks(512) {
            let sent = unsafe { SendInput(chunk, std::mem::size_of::<INPUT>() as i32) };
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

#[cfg(test)]
mod input_readiness_tests {
    use super::{
        wait_for_input_ready_with, Duration, INPUT_READY_BUDGET, INPUT_READY_STEP,
        ReadinessWait,
    };
    use std::cell::Cell;

    /// Drives the wait with a fake clock so both timing contracts are exact.
    fn run(ready_after: Option<usize>) -> (ReadinessWait, usize) {
        let mut polls = 0usize;
        let mut naps = 0usize;
        let clock = Cell::new(Duration::ZERO);
        let mut probe = || {
            polls += 1;
            ready_after.is_some_and(|n| polls > n)
        };
        let mut elapsed = || clock.get();
        let mut sleep = |nap: Duration| {
            naps += 1;
            clock.set(clock.get() + nap);
        };
        let result = wait_for_input_ready_with(
            &mut probe,
            &mut elapsed,
            &mut sleep,
            INPUT_READY_BUDGET,
            INPUT_READY_STEP,
        );
        (result, naps)
    }

    #[test]
    fn already_ready_window_costs_no_wait_at_all() {
        let (result, naps) = run(Some(0));
        assert_eq!(result.waited, Duration::ZERO);
        assert_eq!(naps, 0, "a live window must not be slept on");
        assert_eq!(result.polls, 1);
    }

    #[test]
    fn window_that_becomes_ready_early_stops_waiting_early() {
        let (result, _) = run(Some(2));
        assert_eq!(result.waited, INPUT_READY_STEP * 2u32);
        assert!(result.waited < INPUT_READY_BUDGET);
    }

    #[test]
    fn window_that_never_becomes_ready_never_exceeds_the_old_fixed_sleep() {
        let (result, naps) = run(None);
        assert_eq!(
            result.waited, INPUT_READY_BUDGET,
            "worst case must match the 60 ms it replaced, not exceed it"
        );
        assert_eq!(result.polls, naps + 1);
    }

    #[test]
    fn wait_never_overshoots_a_smaller_budget() {
        let clock = Cell::new(Duration::ZERO);
        let mut probe = || false;
        let mut elapsed = || clock.get();
        let mut sleep = |nap: Duration| clock.set(clock.get() + nap);
        let result = wait_for_input_ready_with(
            &mut probe,
            &mut elapsed,
            &mut sleep,
            Duration::from_millis(10),
            Duration::from_millis(4),
        );
        assert_eq!(result.waited, Duration::from_millis(10));
    }

    #[test]
    fn step_larger_than_budget_cannot_oversleep() {
        let clock = Cell::new(Duration::ZERO);
        let mut probe = || false;
        let mut elapsed = || clock.get();
        let mut sleep = |nap: Duration| clock.set(clock.get() + nap);
        let result = wait_for_input_ready_with(
            &mut probe,
            &mut elapsed,
            &mut sleep,
            Duration::from_millis(5),
            Duration::from_millis(500),
        );
        assert_eq!(result.waited, Duration::from_millis(5));
    }
}
