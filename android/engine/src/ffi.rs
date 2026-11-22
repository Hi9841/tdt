//! The C ABI of this library (`include/tdt_speech.h`). Every function turns
//! an error and a panic into the calling thread's error text, which
//! `tdt_last_error` returns. No panic crosses the ABI.

use crate::engine::Engine;
use crate::take::Take;
use std::cell::RefCell;
use std::ffi::{c_char, c_int, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::Arc;

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_error(message: &str) {
    let text = CString::new(message.replace('\0', " ")).unwrap_or_default();
    LAST_ERROR.with(|error| *error.borrow_mut() = text);
}

/// Runs `body`, turns an `Err` or a panic into the thread's error, and
/// returns `failed` in that case.
fn guard<T>(failed: T, body: impl FnOnce() -> Result<T, String>) -> T {
    set_error("");
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(value)) => value,
        Ok(Err(message)) => {
            set_error(&message);
            failed
        }
        Err(_) => {
            set_error("The speech engine failed. Try again.");
            failed
        }
    }
}

/// The text behind `pointer`.
///
/// # Safety
/// `pointer` is NULL or a NUL-terminated string that lives through the call.
unsafe fn text_arg<'a>(pointer: *const c_char, name: &str) -> Result<&'a str, String> {
    if pointer.is_null() {
        return Err(format!("{name} is NULL"));
    }
    // SAFETY: the caller gives a NUL-terminated string.
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map_err(|_| format!("{name} is not UTF-8"))
}

/// The engine behind `pointer`.
///
/// # Safety
/// `pointer` is NULL or came from `tdt_engine_new`.
unsafe fn engine_arg<'a>(pointer: *const Arc<Engine>) -> Result<&'a Arc<Engine>, String> {
    // SAFETY: an engine is never freed, so the pointer stays valid.
    unsafe { pointer.as_ref() }.ok_or_else(|| "engine is NULL".to_string())
}

/// The take behind `pointer`.
///
/// # Safety
/// `pointer` is NULL or a take that `tdt_take_finish` and `tdt_take_cancel`
/// did not get yet.
unsafe fn take_arg<'a>(pointer: *const Take) -> Result<&'a Take, String> {
    // SAFETY: the caller keeps the take alive through the call.
    unsafe { pointer.as_ref() }.ok_or_else(|| "take is NULL".to_string())
}

/// An engine for the model in `model_dir`. Nothing loads yet.
///
/// # Safety
/// `model_dir` and `precision` are NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn tdt_engine_new(
    model_dir: *const c_char,
    precision: *const c_char,
    end_quiet_ms: c_int,
) -> *mut Arc<Engine> {
    guard(std::ptr::null_mut(), || {
        // SAFETY: the caller gives NUL-terminated strings.
        let (dir, precision) = unsafe {
            (
                text_arg(model_dir, "model_dir")?,
                text_arg(precision, "precision")?,
            )
        };
        let precision = trit::Precision::parse(precision)?;
        let end_quiet_ms = usize::try_from(end_quiet_ms)
            .map_err(|_| format!("end_quiet_ms is {end_quiet_ms}, not 0 or more"))?;
        // 16 samples a millisecond at 16 kHz.
        let engine = Arc::new(Engine::new(Path::new(dir), precision, end_quiet_ms * 16));
        Ok(Box::into_raw(Box::new(engine)))
    })
}

/// # Safety
/// `engine` came from `tdt_engine_new`.
#[no_mangle]
pub unsafe extern "C" fn tdt_engine_load(engine: *const Arc<Engine>) -> c_int {
    guard(-1, || {
        // SAFETY: the caller gives an engine of `tdt_engine_new`.
        unsafe { engine_arg(engine) }?.load().map(|_| 0)
    })
}

/// # Safety
/// `engine` came from `tdt_engine_new`.
#[no_mangle]
pub unsafe extern "C" fn tdt_engine_unload(engine: *const Arc<Engine>) {
    guard((), || {
        // SAFETY: the caller gives an engine of `tdt_engine_new`.
        unsafe { engine_arg(engine) }?.unload();
        Ok(())
    });
}

/// # Safety
/// `engine` came from `tdt_engine_new`; `terms` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn tdt_engine_set_dictionary(
    engine: *const Arc<Engine>,
    terms: *const c_char,
) -> c_int {
    guard(-1, || {
        // SAFETY: the caller gives an engine of `tdt_engine_new` and a string.
        let (engine, terms) = unsafe { (engine_arg(engine)?, text_arg(terms, "terms")?) };
        let terms = terms
            .lines()
            .map(str::trim)
            .filter(|term| !term.is_empty())
            .map(str::to_string)
            .collect();
        engine.set_dictionary(terms);
        Ok(0)
    })
}

/// # Safety
/// `engine` came from `tdt_engine_new`.
#[no_mangle]
pub unsafe extern "C" fn tdt_take_start(engine: *const Arc<Engine>) -> *mut Take {
    guard(std::ptr::null_mut(), || {
        // SAFETY: the caller gives an engine of `tdt_engine_new`.
        let engine = unsafe { engine_arg(engine) }?;
        Ok(Box::into_raw(Box::new(Take::start(engine))))
    })
}

/// # Safety
/// `take` is a take in work; `samples` holds `count` values.
#[no_mangle]
pub unsafe extern "C" fn tdt_take_push(
    take: *const Take,
    samples: *const i16,
    count: usize,
) -> c_int {
    guard(0, || {
        // SAFETY: the caller gives a take in work.
        let take = unsafe { take_arg(take) }?;
        if samples.is_null() {
            return Err("samples is NULL".into());
        }
        // SAFETY: the caller gives `count` values at `samples`.
        let pcm = unsafe { std::slice::from_raw_parts(samples, count) };
        Ok(c_int::from(take.push(pcm)))
    })
}

/// # Safety
/// `take` is a take in work.
#[no_mangle]
pub unsafe extern "C" fn tdt_take_stop(take: *const Take) -> c_int {
    guard(0, || {
        // SAFETY: the caller gives a take in work.
        Ok(c_int::from(unsafe { take_arg(take) }?.ask_stop()))
    })
}

/// # Safety
/// `take` is a take in work. The call frees it.
#[no_mangle]
pub unsafe extern "C" fn tdt_take_finish(take: *mut Take) -> *mut c_char {
    guard(std::ptr::null_mut(), || {
        if take.is_null() {
            return Err("take is NULL".into());
        }
        // SAFETY: the pointer came from `tdt_take_start`, and the caller
        // does not use it again.
        let take = unsafe { Box::from_raw(take) };
        let heard = take.finish()?;
        let text = CString::new(heard.text.replace('\0', " ")).unwrap_or_default();
        Ok(text.into_raw())
    })
}

/// # Safety
/// `take` is NULL or a take in work. The call frees it.
#[no_mangle]
pub unsafe extern "C" fn tdt_take_cancel(take: *mut Take) {
    guard((), || {
        if !take.is_null() {
            // SAFETY: the pointer came from `tdt_take_start`, and the caller
            // does not use it again. The take's thread ends by itself.
            drop(unsafe { Box::from_raw(take) });
        }
        Ok(())
    });
}

/// # Safety
/// `text` is NULL or came from `tdt_take_finish`.
#[no_mangle]
pub unsafe extern "C" fn tdt_text_free(text: *mut c_char) {
    if !text.is_null() {
        // SAFETY: the pointer came from `CString::into_raw`.
        drop(unsafe { CString::from_raw(text) });
    }
}

/// The error of the last failed call on this thread, or "".
#[no_mangle]
pub extern "C" fn tdt_last_error() -> *const c_char {
    LAST_ERROR.with(|error| error.borrow().as_ptr())
}

/// The Trit version built into this library, for example "0.11.0".
#[no_mangle]
pub extern "C" fn tdt_trit_version() -> *const c_char {
    static VERSION: std::sync::OnceLock<CString> = std::sync::OnceLock::new();
    VERSION
        .get_or_init(|| CString::new(trit::VERSION).unwrap_or_default())
        .as_ptr()
}
