use arboard::Clipboard;
use parking_lot::Mutex;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

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

    pub fn auto_paste(&self, text: &str, target_hwnd: Option<isize>) -> Result<(), String> {
        // Keep the transcription available on the clipboard as a fallback.
        self.copy_to_clipboard(text)?;

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
                }
            }
        }

        // Give Windows time to restore focus before sending text input.
        thread::sleep(Duration::from_millis(60));

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
