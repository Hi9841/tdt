//! Clipboard copy and Unicode text typing on macOS using CoreGraphics events.

use arboard::Clipboard;
use parking_lot::Mutex;
use std::ffi::c_void;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// UTF-16 code units per CoreGraphics keyboard event (macOS caps at 20).
const UNICODE_CHUNK_UNITS: usize = 16;
const PAUSE_BETWEEN_CHUNKS: Duration = Duration::from_millis(2);

#[derive(Clone, Copy, Debug, Default)]
pub struct DeliverySteps {
    pub copy: Duration,
    pub focus: Duration,
    pub typing: Duration,
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
    Copied,
    Pasted,
    PastedNotCopied(String),
    CopiedFallback(NotTyped),
}

#[derive(Debug, PartialEq)]
pub enum NotTyped {
    UserTyped,
    Elevated,
    NoFocus(&'static str),
    Partial(String),
}

impl NotTyped {
    pub fn notice(&self) -> &'static str {
        match self {
            Self::UserTyped => {
                "You typed while TDT was finishing. The final text is on the clipboard."
            }
            Self::Elevated => {
                "TDT needs Accessibility permission to type into other apps. \
                 Press Cmd+V to paste the text."
            }
            Self::NoFocus(_) => {
                "TDT could not go back to the app. Click in the app and press Cmd+V to paste the text."
            }
            Self::Partial(_) => {
                "Copied. Could not insert all text. Check the destination before pasting with Cmd+V."
            }
        }
    }
}

impl std::fmt::Display for NotTyped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserTyped => f.write_str("the user typed"),
            Self::Elevated => f.write_str("Accessibility permission is not granted"),
            Self::NoFocus(detail) => f.write_str(detail),
            Self::Partial(detail) => f.write_str(detail),
        }
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

    pub fn copy_timed(&self, text: &str, steps: &mut DeliverySteps) -> Result<(), String> {
        let started = Instant::now();
        let copied = self.copy_to_clipboard(text);
        steps.copy = started.elapsed();
        copied
    }

    pub fn try_type(
        &self,
        text: &str,
        target_pid: Option<isize>,
        user_typed: &dyn Fn() -> bool,
        steps: &mut DeliverySteps,
    ) -> Result<(), NotTyped> {
        let started = Instant::now();
        let pid = self.prepare_target(target_pid)?;
        steps.focus = started.elapsed();

        let started = Instant::now();
        let typed = type_text_macos(text, pid, user_typed);
        steps.typing = started.elapsed();
        if typed.is_ok() {
            steps.typed_at = Some(Instant::now());
        }
        typed
    }

    fn prepare_target(&self, target_pid: Option<isize>) -> Result<i32, NotTyped> {
        let Some(raw) = target_pid else {
            return Err(NotTyped::NoFocus("no target app"));
        };
        if !crate::target::accessibility_trusted() {
            return Err(NotTyped::Elevated);
        }
        let pid = raw as i32;
        if pid <= 0 {
            return Err(NotTyped::NoFocus("invalid target app"));
        }
        if frontmost_pid() != Some(pid) {
            activate_pid(pid);
            for _ in 0..8 {
                if frontmost_pid() == Some(pid) {
                    return Ok(pid);
                }
                thread::sleep(Duration::from_millis(15));
            }
            return Err(NotTyped::NoFocus("the app did not come to the front"));
        }
        Ok(pid)
    }
}

fn frontmost_pid() -> Option<i32> {
    use objc2_app_kit::NSWorkspace;
    let workspace = NSWorkspace::sharedWorkspace();
    workspace
        .frontmostApplication()
        .map(|app| app.processIdentifier())
}

fn activate_pid(pid: i32) {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
        #[allow(deprecated)]
        let _ = app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
    }
}

// CoreGraphics FFI for Unicode typing
type CGEventSourceRef = *mut c_void;
type CGEventRef = *mut c_void;
const K_CG_EVENT_SOURCE_STATE_HID: i32 = 1;
const K_CG_HID_EVENT_TAP: u32 = 0;
const K_CG_EVENT_SOURCE_USER_DATA: u32 = 42;
const K_VK_RETURN: u16 = 0x24;
const K_VK_TAB: u16 = 0x30;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceCreate(state_id: i32) -> CGEventSourceRef;
    fn CGEventCreateKeyboardEvent(
        source: CGEventSourceRef,
        virtual_key: u16,
        key_down: bool,
    ) -> CGEventRef;
    fn CGEventKeyboardSetUnicodeString(
        event: CGEventRef,
        string_length: usize,
        unicode_string: *const u16,
    );
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventPost(tap: u32, event: CGEventRef);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
}

fn check_ready(pid: i32, user_typed: &dyn Fn() -> bool, sent_any: bool) -> Result<(), NotTyped> {
    if user_typed() {
        return Err(if sent_any {
            NotTyped::Partial("stopped typing because a key was pressed".to_string())
        } else {
            NotTyped::UserTyped
        });
    }
    if frontmost_pid() != Some(pid) {
        return Err(if sent_any {
            NotTyped::Partial("stopped typing because the app lost focus".to_string())
        } else {
            NotTyped::NoFocus("the app lost focus before typing")
        });
    }
    Ok(())
}

fn post_key(
    source: CGEventSourceRef,
    virtual_key: u16,
    unicode: Option<&[u16]>,
) -> Result<(), NotTyped> {
    // SAFETY: CoreGraphics event creation, tagging, posting, and release.
    unsafe {
        for key_down in [true, false] {
            let ev = CGEventCreateKeyboardEvent(source, virtual_key, key_down);
            if ev.is_null() {
                return Err(NotTyped::Partial(
                    "macOS could not create a keyboard event".to_string(),
                ));
            }
            CGEventSetFlags(ev, 0);
            CGEventSetIntegerValueField(
                ev,
                K_CG_EVENT_SOURCE_USER_DATA,
                crate::keys::OWN_INPUT as i64,
            );
            if let Some(units) = unicode {
                CGEventKeyboardSetUnicodeString(ev, units.len(), units.as_ptr());
            }
            CGEventPost(K_CG_HID_EVENT_TAP, ev);
            CFRelease(ev);
        }
    }
    Ok(())
}

fn type_text_macos(text: &str, pid: i32, user_typed: &dyn Fn() -> bool) -> Result<(), NotTyped> {
    if text.contains('\0') {
        return Err(NotTyped::Partial(
            "Cannot type text containing a null character".to_string(),
        ));
    }
    check_ready(pid, user_typed, false)?;

    // SAFETY: `CGEventSourceCreate` returns a CF object released before returning.
    let source = unsafe { CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_HID) };
    let mut utf16_buf = Vec::with_capacity(UNICODE_CHUNK_UNITS);
    let mut sent_any = false;

    let result = (|| -> Result<(), NotTyped> {
        let flush = |buf: &mut Vec<u16>, sent_any: &mut bool| -> Result<(), NotTyped> {
            if buf.is_empty() {
                return Ok(());
            }
            if *sent_any {
                thread::sleep(PAUSE_BETWEEN_CHUNKS);
            }
            check_ready(pid, user_typed, *sent_any)?;
            post_key(source, 0, Some(buf))?;
            buf.clear();
            *sent_any = true;
            Ok(())
        };

        for ch in text.chars() {
            match ch {
                '\r' => continue,
                '\n' => {
                    flush(&mut utf16_buf, &mut sent_any)?;
                    if sent_any {
                        thread::sleep(PAUSE_BETWEEN_CHUNKS);
                    }
                    check_ready(pid, user_typed, sent_any)?;
                    post_key(source, K_VK_RETURN, None)?;
                    sent_any = true;
                }
                '\t' => {
                    flush(&mut utf16_buf, &mut sent_any)?;
                    if sent_any {
                        thread::sleep(PAUSE_BETWEEN_CHUNKS);
                    }
                    check_ready(pid, user_typed, sent_any)?;
                    post_key(source, K_VK_TAB, None)?;
                    sent_any = true;
                }
                _ => {
                    let mut encoded = [0u16; 2];
                    let units = ch.encode_utf16(&mut encoded);
                    if utf16_buf.len() + units.len() > UNICODE_CHUNK_UNITS {
                        flush(&mut utf16_buf, &mut sent_any)?;
                    }
                    utf16_buf.extend_from_slice(units);
                }
            }
        }
        flush(&mut utf16_buf, &mut sent_any)?;
        Ok(())
    })();

    if !source.is_null() {
        // SAFETY: balances `CGEventSourceCreate`.
        unsafe { CFRelease(source) };
    }
    result
}
