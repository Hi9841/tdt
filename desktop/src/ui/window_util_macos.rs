//! Overlay window management on macOS (floating pill, flyout panel, Spaces,
//! screen positioning, click-through outside the pill, and focus restoration).

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::{ClassType, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy, NSColor, NSEvent,
    NSFloatingWindowLevel, NSRunningApplication, NSScreen, NSWindow, NSWindowCollectionBehavior,
    NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

pub const PANEL_WIDTH: i32 = 400;
pub const PANEL_HEIGHT: i32 = 540;
pub const PANEL_MIN_HEIGHT: i32 = 480;
pub const PANEL_MAX_HEIGHT: i32 = 620;

/// `NSWindowStyleMaskNonactivatingPanel` (1 << 7) combined with `Borderless` (0).
/// Removes macOS's 1px top bezel hairline while keeping the panel non-activating.
fn overlay_style_mask() -> NSWindowStyleMask {
    NSWindowStyleMask::from_bits_retain(1 << 7)
}

static OVERLAY_EXPANDED: AtomicBool = AtomicBool::new(false);
static PREVIOUS_APP_PID: Mutex<Option<i32>> = Mutex::new(None);
static LAST_EXTERNAL_PID: AtomicI32 = AtomicI32::new(0);
static PILL_HIT_RECT: Mutex<Option<(f32, f32, f32, f32)>> = Mutex::new(None);
static CHROME_LOCKED: AtomicBool = AtomicBool::new(false);

/// Placeholder handle type on macOS so `main.rs` shares the same startup flow.
#[derive(Clone, Copy)]
pub struct AppWindow;

/// Configures the macOS app as a menu bar accessory (no Dock icon).
pub fn configure_accessory_app() {
    on_main_thread(|mtm| {
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    });
}

fn on_main_thread(f: impl FnOnce(MainThreadMarker) + Send + 'static) {
    if let Some(mtm) = MainThreadMarker::new() {
        f(mtm);
    } else {
        DispatchQueue::main().exec_async(move || {
            // SAFETY: `DispatchQueue::main()` executes on the main thread.
            let mtm = unsafe { MainThreadMarker::new_unchecked() };
            f(mtm);
        });
    }
}

fn overlay_window(mtm: MainThreadMarker) -> Option<Retained<NSWindow>> {
    let app = NSApplication::sharedApplication(mtm);
    for window in app.windows().iter() {
        let class_name = objc2::runtime::AnyObject::class(&window)
            .name()
            .to_string_lossy();
        if class_name.contains("StatusBar") || class_name.contains("Menu") {
            continue;
        }
        let size = window.frame().size;
        if size.width >= 200.0 && size.height >= 200.0 {
            return Some(window);
        }
    }
    None
}

/// Limit input on the collapsed overlay to `rect` (logical top-left coordinates
/// inside the 400x540 GPUI canvas).
pub fn set_pill_hit_rect(rect: (f32, f32, f32, f32)) {
    {
        let mut current = PILL_HIT_RECT.lock();
        if *current == Some(rect) {
            return;
        }
        *current = Some(rect);
    }
    on_main_thread(update_mouse_passthrough);
}

/// Updates `NSWindow::setIgnoresMouseEvents` so clicks pass through the
/// transparent Metal canvas above the collapsed pill to apps behind it, while
/// the pill itself and the expanded panel remain clickable.
fn update_mouse_passthrough(mtm: MainThreadMarker) {
    let Some(window) = overlay_window(mtm) else {
        return;
    };
    if OVERLAY_EXPANDED.load(Ordering::SeqCst) {
        if window.ignoresMouseEvents() {
            window.setIgnoresMouseEvents(false);
        }
        return;
    }
    let hit_rect = *PILL_HIT_RECT.lock();
    let ignore = match hit_rect {
        None => true,
        Some((rx, ry, rw, rh)) => {
            let mouse = NSEvent::mouseLocation();
            let frame = window.frame();
            // Convert GPUI top-left logical coordinates to Cocoa bottom-left screen coordinates.
            let pad = 6.0;
            let min_x = frame.origin.x + f64::from(rx) - pad;
            let max_x = frame.origin.x + f64::from(rx + rw) + pad;
            let top_y = frame.origin.y + frame.size.height - f64::from(ry) + pad;
            let min_y = frame.origin.y + frame.size.height - f64::from(ry + rh) - pad;
            !(mouse.x >= min_x && mouse.x <= max_x && mouse.y >= min_y && mouse.y <= top_y)
        }
    };
    if window.ignoresMouseEvents() != ignore {
        window.setIgnoresMouseEvents(ignore);
    }
}

/// Opens a file with its default macOS application.
pub fn open_file(path: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("/usr/bin/open")
        .arg(path)
        .spawn()
        .map(drop)
        .map_err(|error| format!("macOS could not open {}: {error}", path.display()))
}

pub fn find_app_hwnd() -> Option<AppWindow> {
    let found = if let Some(mtm) = MainThreadMarker::new() {
        overlay_window(mtm).is_some()
    } else {
        let result = AtomicBool::new(false);
        DispatchQueue::main().exec_sync(|| {
            // SAFETY: `DispatchQueue::main()` executes on the main thread.
            let mtm = unsafe { MainThreadMarker::new_unchecked() };
            result.store(overlay_window(mtm).is_some(), Ordering::SeqCst);
        });
        result.load(Ordering::SeqCst)
    };
    found.then_some(AppWindow)
}

fn configure_overlay_window(window: &NSWindow) {
    let behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary
        | NSWindowCollectionBehavior::Stationary;
    let clear = NSColor::clearColor();
    let target_mask = overlay_style_mask();
    if window.styleMask() != target_mask {
        window.setStyleMask(target_mask);
    }
    window.setLevel(NSFloatingWindowLevel);
    window.setCollectionBehavior(behavior);
    window.setOpaque(false);
    window.setBackgroundColor(Some(&clear));
    window.setHasShadow(false);
    window.invalidateShadow();
}

pub fn apply_overlay_window_style(_window: AppWindow) {
    on_main_thread(|mtm| {
        let Some(window) = overlay_window(mtm) else {
            return;
        };
        configure_overlay_window(&window);
        update_mouse_passthrough(mtm);
    });
}

pub fn keep_pill_from_activating(_window: AppWindow) {}

pub fn show_overlay_window(_window: AppWindow) {
    on_main_thread(|mtm| {
        if let Some(window) = overlay_window(mtm) {
            window.orderFrontRegardless();
        }
    });
}

pub fn ensure_overlay_topmost() {
    if let Some(pid) = frontmost_external_pid() {
        LAST_EXTERNAL_PID.store(pid, Ordering::Relaxed);
    }
    on_main_thread(update_mouse_passthrough);
}

pub fn lock_overlay_chrome() {
    on_main_thread(|mtm| {
        let Some(window) = overlay_window(mtm) else {
            return;
        };
        let target_mask = overlay_style_mask();
        if !CHROME_LOCKED.load(Ordering::Relaxed)
            || window.styleMask() != target_mask
            || window.hasShadow()
            || window.isOpaque()
        {
            configure_overlay_window(&window);
            CHROME_LOCKED.store(true, Ordering::Relaxed);
        }
    });
}

pub fn follow_current_virtual_desktop() {}

pub fn position_bubble_on_preferred_monitor(_window: AppWindow) {
    on_main_thread(|mtm| {
        let Some(window) = overlay_window(mtm) else {
            return;
        };
        let Some(screen) = NSScreen::mainScreen(mtm) else {
            return;
        };
        let visible = screen.visibleFrame();
        let w = (PANEL_WIDTH as f64).min((visible.size.width - 48.0).max(200.0));
        let h = (PANEL_HEIGHT as f64)
            .min((visible.size.height - 48.0).max(PANEL_MIN_HEIGHT as f64))
            .min(PANEL_MAX_HEIGHT as f64);
        let x = visible.origin.x + ((visible.size.width - w) / 2.0).max(0.0);
        let y = visible.origin.y + 40.0;
        let frame = NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
        window.setFrame_display(frame, true);
        update_mouse_passthrough(mtm);
    });
}

pub fn move_pill_to_screen_of(_target: Option<isize>) {}

pub fn set_window_mode(is_expanded: bool) {
    OVERLAY_EXPANDED.store(is_expanded, Ordering::SeqCst);
    if is_expanded {
        if let Some(pid) = frontmost_external_pid() {
            LAST_EXTERNAL_PID.store(pid, Ordering::Relaxed);
            *PREVIOUS_APP_PID.lock() = Some(pid);
        }
        on_main_thread(|mtm| {
            let app = NSApplication::sharedApplication(mtm);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            if let Some(window) = overlay_window(mtm) {
                window.setIgnoresMouseEvents(false);
                window.makeKeyAndOrderFront(None);
            }
        });
    } else {
        on_main_thread(update_mouse_passthrough);
        if let Some(pid) = PREVIOUS_APP_PID.lock().take() {
            let own_pid = std::process::id() as i32;
            if frontmost_pid() == Some(own_pid) {
                if let Some(prev) =
                    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                {
                    #[allow(deprecated)]
                    let _ = prev.activateWithOptions(
                        NSApplicationActivationOptions::ActivateIgnoringOtherApps,
                    );
                }
            }
        }
    }
}

pub fn dictation_target_window() -> Option<isize> {
    if let Some(pid) = frontmost_external_pid() {
        LAST_EXTERNAL_PID.store(pid, Ordering::Relaxed);
        return Some(pid as isize);
    }
    if let Some(pid) = *PREVIOUS_APP_PID.lock() {
        return Some(pid as isize);
    }
    let last = LAST_EXTERNAL_PID.load(Ordering::Relaxed);
    (last > 0).then_some(last as isize)
}

pub fn client_animations_enabled() -> bool {
    let workspace = NSWorkspace::sharedWorkspace();
    !workspace.accessibilityDisplayShouldReduceMotion()
}

fn frontmost_pid() -> Option<i32> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier())
}

fn frontmost_external_pid() -> Option<i32> {
    let own = std::process::id() as i32;
    frontmost_pid().filter(|&pid| pid > 0 && pid != own)
}

const _: fn() = || {
    let _ = <NSWindow as ClassType>::class;
};
