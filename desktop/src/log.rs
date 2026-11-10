//! Lines for the log of the program that uses this library. TDT writes them
//! to `tdt.log`. The bench programs print them.

use std::sync::OnceLock;

static SINK: OnceLock<fn(&str)> = OnceLock::new();

/// Sends each later `note` to `sink`. Only the first call has an effect.
pub fn set_sink(sink: fn(&str)) {
    let _ = SINK.set(sink);
}

/// One line for the log. It must not hold text that the user dictated.
pub fn note(message: &str) {
    match SINK.get() {
        Some(sink) => sink(message),
        None => eprintln!("{message}"),
    }
}
