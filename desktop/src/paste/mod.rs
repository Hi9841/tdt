#[cfg(windows)]
pub mod injector;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
pub mod injector;
pub use injector::{DeliveryOutcome, DeliverySteps, NotTyped, PasteInjector};
