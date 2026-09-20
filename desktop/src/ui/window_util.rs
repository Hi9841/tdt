#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, WPARAM};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
    DWMWA_NCRENDERING_POLICY, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, SetWindowRgn, HDC, HMONITOR, HRGN,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::HiDpi::GetDpiForWindow;
#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetFocus};
use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetClientRect, GetForegroundWindow, GetWindowLongW, GetWindowRect,
    GetWindowThreadProcessId, IsWindow, PostMessageW, SetForegroundWindow, SetWindowLongW,
    SetWindowPos, ShowWindow, SystemParametersInfoW, GWL_EXSTYLE, GWL_STYLE, HTCAPTION,
    HWND_TOPMOST, SPI_GETCLIENTAREAANIMATION, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOCOPYBITS,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WM_NCLBUTTONDOWN, WS_CAPTION, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_THICKFRAME,
};

pub const BUBBLE_WIDTH: f32 = 400.0;
pub const BUBBLE_HEIGHT: f32 = 42.0;
pub const PANEL_WIDTH: i32 = 400;
pub const PANEL_HEIGHT: i32 = 540;
pub const PANEL_MIN_HEIGHT: i32 = 480;
pub const PANEL_MAX_HEIGHT: i32 = 620;
pub const PANEL_OPEN_MS: u64 = 240;
pub const PANEL_CLOSE_MS: u64 = 180;
/// Inset from the monitor work area when choosing a panel size. 48 keeps the
/// existing 480-620 height range on typical displays while still fitting a
/// 400px-wide panel (400 + 48). These values are GPUI logical pixels and are
/// converted to device pixels before Win32 sizing calls.
const PANEL_WORK_MARGIN: i32 = 48;
const PANEL_EDGE_PAD: i32 = 20;

fn overlay_expanded() -> &'static std::sync::atomic::AtomicBool {
    static FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &FLAG
}

fn overlay_animating() -> &'static std::sync::atomic::AtomicBool {
    static FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &FLAG
}

fn overlay_hidden() -> &'static std::sync::atomic::AtomicBool {
    static FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &FLAG
}

fn remembered_hwnd() -> &'static parking_lot::Mutex<Option<isize>> {
    static HWND: parking_lot::Mutex<Option<isize>> = parking_lot::Mutex::new(None);
    &HWND
}

fn previous_foreground() -> &'static parking_lot::Mutex<Option<isize>> {
    static HWND: parking_lot::Mutex<Option<isize>> = parking_lot::Mutex::new(None);
    &HWND
}

/// Compute the overlay extended style. Always keeps topmost + toolwindow.
/// Expanded panels drop WS_EX_NOACTIVATE so they can take keyboard focus.
pub fn overlay_extended_style(current_ex: i32, expanded: bool) -> i32 {
    let topmost_tool = (WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0) as i32;
    let noactivate = WS_EX_NOACTIVATE.0 as i32;
    if expanded {
        (current_ex | topmost_tool) & !noactivate
    } else {
        current_ex | topmost_tool | noactivate
    }
}

/// Restore the captured app only while this overlay still holds foreground.
pub fn should_restore_previous_foreground(
    overlay: isize,
    current_fg: isize,
    previous: Option<isize>,
) -> bool {
    let Some(prev) = previous else {
        return false;
    };
    if prev == 0 || overlay == 0 || prev == overlay {
        return false;
    }
    current_fg == overlay
}

/// After the open morph, activate only if the user did not switch apps.
pub fn should_activate_overlay_after_open(
    overlay: isize,
    current_fg: isize,
    previous: Option<isize>,
) -> bool {
    if overlay == 0 {
        return false;
    }
    current_fg == 0 || current_fg == overlay || previous == Some(current_fg)
}

/// External dictation target: live foreground if it is not the overlay,
/// otherwise a still-valid captured previous hwnd. Never returns the overlay.
pub fn dictation_target_from_handles(
    overlay: Option<isize>,
    current_fg: isize,
    previous: Option<isize>,
    previous_is_live: bool,
) -> Option<isize> {
    if current_fg != 0 && overlay != Some(current_fg) {
        return Some(current_fg);
    }
    match previous {
        Some(prev) if prev != 0 && overlay != Some(prev) && previous_is_live => Some(prev),
        _ => None,
    }
}

/// Apply topmost/toolwindow and NOACTIVATE according to the live expanded flag.
/// Main startup should call this instead of OR-ing WS_EX_NOACTIVATE blindly.
/// SetWindowPos always uses SWP_NOACTIVATE so a style refresh cannot steal focus.
#[cfg(target_os = "windows")]
pub fn apply_overlay_window_style(hwnd: HWND) {
    if hwnd.is_invalid() {
        return;
    }
    let expanded = overlay_expanded().load(std::sync::atomic::Ordering::SeqCst);
    unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let next = overlay_extended_style(ex_style, expanded);
        if next != ex_style {
            let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, next);
        }
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOCOPYBITS | SWP_NOACTIVATE,
        );
        apply_overlay_dwm(hwnd);
    }
}

#[cfg(target_os = "windows")]
unsafe fn apply_overlay_dwm(hwnd: HWND) {
    // GPUI owns the alpha-capable client surface. A second system backdrop or
    // an extended DWM frame can hide GPUI's painted content, so keep DWM
    // ownership limited to the border and corner treatment.
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_BORDER_COLOR,
        &DWMWA_COLOR_NONE as *const _ as _,
        std::mem::size_of_val(&DWMWA_COLOR_NONE) as u32,
    );
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_NCRENDERING_POLICY,
        &DWMNCRP_DISABLED as *const _ as _,
        std::mem::size_of_val(&DWMNCRP_DISABLED) as u32,
    );
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &DWMWCP_DONOTROUND as *const _ as _,
        std::mem::size_of_val(&DWMWCP_DONOTROUND) as u32,
    );
    apply_overlay_region(hwnd);
}

#[cfg(target_os = "windows")]
unsafe fn apply_overlay_region(hwnd: HWND) {
    // A GDI round region is not antialiased and produces a second, jagged
    // curve over GPUI's GPU-painted corner. Clear it and let the transparent
    // client surface provide the single visible rounded shape.
    let _ = SetWindowRgn(hwnd, HRGN::default(), true);
}

/// HWND of the app that should receive inserted text. Never the TDT overlay.
pub fn dictation_target_window() -> Option<isize> {
    #[cfg(target_os = "windows")]
    {
        let overlay = find_app_hwnd();
        let overlay_raw = overlay.map(|h| h.0 as isize);
        let fg = unsafe { GetForegroundWindow() };
        let fg_raw = if fg.is_invalid() { 0 } else { fg.0 as isize };
        let previous = *previous_foreground().lock();
        let previous_is_live = previous.is_some_and(hwnd_still_valid);
        dictation_target_from_handles(overlay_raw, fg_raw, previous, previous_is_live)
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[cfg(target_os = "windows")]
fn hwnd_still_valid(raw: isize) -> bool {
    if raw == 0 {
        return false;
    }
    unsafe { IsWindow(HWND(raw as _)).as_bool() }
}

#[cfg(target_os = "windows")]
fn capture_previous_foreground(overlay: HWND) {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg == overlay {
            return;
        }
        *previous_foreground().lock() = Some(fg.0 as isize);
    }
}

#[cfg(target_os = "windows")]
fn set_foreground_hwnd(hwnd: HWND) {
    if hwnd.is_invalid() {
        return;
    }
    unsafe {
        if !IsWindow(hwnd).as_bool() {
            return;
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

#[cfg(target_os = "windows")]
fn activate_overlay_window(hwnd: HWND) {
    if hwnd.is_invalid() {
        return;
    }
    set_foreground_hwnd(hwnd);
    unsafe {
        let _ = SetFocus(hwnd);
    }
}

#[cfg(target_os = "windows")]
fn restore_previous_foreground(overlay: HWND) {
    let previous = *previous_foreground().lock();
    let current = unsafe { GetForegroundWindow() }.0 as isize;
    if !should_restore_previous_foreground(overlay.0 as isize, current, previous) {
        previous_foreground().lock().take();
        return;
    }
    let Some(raw) = previous_foreground().lock().take() else {
        return;
    };
    if !hwnd_still_valid(raw) {
        return;
    }
    set_foreground_hwnd(HWND(raw as _));
}

#[cfg(target_os = "windows")]
fn finish_window_mode(hwnd: HWND, is_expanded: bool) {
    if !is_expanded {
        bubble_anchor().lock().take();
        overlay_expanded().store(false, std::sync::atomic::Ordering::SeqCst);
        apply_overlay_window_style(hwnd);
        restore_previous_foreground(hwnd);
    } else {
        apply_overlay_window_style(hwnd);
        let current = unsafe { GetForegroundWindow() }.0 as isize;
        let previous = *previous_foreground().lock();
        if should_activate_overlay_after_open(hwnd.0 as isize, current, previous) {
            activate_overlay_window(hwnd);
        }
    }
    overlay_animating().store(false, std::sync::atomic::Ordering::SeqCst);
}

pub fn is_overlay_hidden() -> bool {
    overlay_hidden().load(std::sync::atomic::Ordering::SeqCst)
}

pub fn set_overlay_hidden(hidden: bool) {
    overlay_hidden().store(hidden, std::sync::atomic::Ordering::SeqCst);
    #[cfg(target_os = "windows")]
    {
        let Some(hwnd) = find_app_hwnd() else {
            return;
        };
        unsafe {
            let _ = ShowWindow(hwnd, if hidden { SW_HIDE } else { SW_SHOWNOACTIVATE });
            if !hidden {
                apply_overlay_dwm(hwnd);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn window_scale(hwnd: HWND) -> f32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        1.0
    } else {
        dpi as f32 / 96.0
    }
}

fn logical_to_device(value: f32, scale: f32) -> i32 {
    (value * scale).round().max(1.0) as i32
}

/// Window-chrome insets GPUI reserves on the borderless overlay: the delta
/// between the outer window rect and its client rect. Sizing the window with
/// the raw bubble constants leaves the client area smaller than the UI
/// expects, clipping the pill's right and bottom edges.
#[cfg(target_os = "windows")]
fn chrome_inset(hwnd: HWND) -> (i32, i32) {
    unsafe {
        let mut outer = RECT::default();
        let mut client = RECT::default();
        if GetWindowRect(hwnd, &mut outer).is_ok() && GetClientRect(hwnd, &mut client).is_ok() {
            let w = (outer.right - outer.left) - (client.right - client.left);
            let h = (outer.bottom - outer.top) - (client.bottom - client.top);
            return (w.max(0), h.max(0));
        }
    }
    (0, 0)
}

/// Outer window size that yields the intended logical bubble client area.
#[cfg(target_os = "windows")]
fn bubble_outer_size(hwnd: HWND) -> (i32, i32) {
    let (dw, dh) = chrome_inset(hwnd);
    let scale = window_scale(hwnd);
    (
        logical_to_device(BUBBLE_WIDTH, scale) + dw,
        logical_to_device(BUBBLE_HEIGHT, scale) + dh,
    )
}

/// Make the overlay a true popup, strip resize handles, and snap the collapsed
/// surface to exactly 400x42 logical pixels without an overlapped-window frame.
pub fn lock_overlay_chrome() {
    #[cfg(target_os = "windows")]
    {
        if is_overlay_hidden() {
            return;
        }
        let Some(hwnd) = find_app_hwnd() else {
            return;
        };
        unsafe {
            let style = GetWindowLongW(hwnd, GWL_STYLE);
            let blocked =
                (WS_CAPTION.0 | WS_THICKFRAME.0 | WS_MAXIMIZEBOX.0 | WS_MINIMIZEBOX.0) as i32;
            let popup_style = (style & !blocked) | WS_POPUP.0 as i32;
            if popup_style != style {
                let _ = SetWindowLongW(hwnd, GWL_STYLE, popup_style);
                let _ = SetWindowPos(
                    hwnd,
                    HWND::default(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE
                        | SWP_NOZORDER
                        | SWP_NOCOPYBITS
                        | SWP_FRAMECHANGED
                        | SWP_NOMOVE
                        | SWP_NOSIZE,
                );
                apply_overlay_dwm(hwnd);
            }
            if overlay_expanded().load(std::sync::atomic::Ordering::SeqCst)
                || overlay_animating().load(std::sync::atomic::Ordering::SeqCst)
            {
                return;
            }
            let mut rect = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rect);
            let (want_w, want_h) = bubble_outer_size(hwnd);
            let w = rect.right - rect.left;
            let h = rect.bottom - rect.top;
            if w != want_w || h != want_h {
                let bottom = rect.bottom;
                let _ = SetWindowPos(
                    hwnd,
                    HWND::default(),
                    rect.left,
                    bottom - want_h,
                    want_w,
                    want_h,
                    SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOCOPYBITS,
                );
                apply_overlay_region(hwnd);
            }
        }
    }
}

fn bubble_anchor() -> &'static parking_lot::Mutex<Option<(i32, i32, i32)>> {
    static POS: parking_lot::Mutex<Option<(i32, i32, i32)>> = parking_lot::Mutex::new(None);
    &POS
}

fn save_bubble_rect(left: i32, top: i32, width: i32) {
    let mut saved = bubble_anchor().lock();
    if saved.is_none() {
        *saved = Some((left, top, width.max(1)));
    }
}

/// Size the expanded panel. Width stays 400 when the work area allows it.
/// Height stays in 480-620 when space allows, and both shrink to fit a
/// small monitor rather than overflowing.
#[cfg(test)]
fn panel_size_for_work(work_w: i32, work_h: i32) -> (i32, i32) {
    panel_size_for_work_at_scale(work_w, work_h, 1.0)
}

fn panel_size_for_work_at_scale(work_w: i32, work_h: i32, scale: f32) -> (i32, i32) {
    let width = logical_to_device(PANEL_WIDTH as f32, scale);
    let height = logical_to_device(PANEL_HEIGHT as f32, scale);
    let min_height = logical_to_device(PANEL_MIN_HEIGHT as f32, scale);
    let max_height = logical_to_device(PANEL_MAX_HEIGHT as f32, scale);
    let margin = logical_to_device(PANEL_WORK_MARGIN as f32, scale);
    (
        clamp_panel_extent(work_w, width, width, width, margin),
        clamp_panel_extent(work_h, height, min_height, max_height, margin),
    )
}

fn clamp_panel_extent(
    work: i32,
    preferred: i32,
    min_preferred: i32,
    max_preferred: i32,
    margin: i32,
) -> i32 {
    if work <= 0 {
        return preferred;
    }
    let available = (work - margin).max(1);
    if available < min_preferred {
        available
    } else {
        available.min(max_preferred).max(min_preferred)
    }
}

/// Clamp the expanded panel to a monitor work area. Origins may be negative
/// or beyond the primary width when the overlay lives on a second display.
#[cfg(test)]
fn panel_origin(bubble: (i32, i32), work: (i32, i32, i32, i32), panel: (i32, i32)) -> (i32, i32) {
    panel_origin_at_scale(bubble, work, panel, 1.0)
}

fn panel_origin_at_scale(
    bubble: (i32, i32),
    work: (i32, i32, i32, i32),
    panel: (i32, i32),
    scale: f32,
) -> (i32, i32) {
    let pad = logical_to_device(PANEL_EDGE_PAD as f32, scale);
    let (bubble_left, bubble_top) = bubble;
    let (work_left, work_top, work_right, work_bottom) = work;
    let (panel_w, panel_h) = panel;
    let bubble_bottom = bubble_top + logical_to_device(BUBBLE_HEIGHT, scale);
    let min_left = work_left + pad;
    let max_left = (work_right - panel_w - pad).max(min_left);
    let min_top = work_top + pad;
    let max_top = (work_bottom - panel_h - pad).max(min_top);
    let left = bubble_left.clamp(min_left, max_left);
    let top = (bubble_bottom - panel_h).clamp(min_top, max_top);
    (left, top)
}

fn cubic_bezier_point(t: f32, a: f32, b: f32) -> f32 {
    let u = 1.0 - t;
    3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
}

fn cubic_bezier_deriv(t: f32, a: f32, b: f32) -> f32 {
    let u = 1.0 - t;
    3.0 * u * u * a + 6.0 * u * t * (b - a) + 3.0 * t * t * (1.0 - b)
}

/// iOS-like drawer curve: cubic-bezier(0.32, 0.72, 0, 1)
pub fn ease_drawer(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let x1 = 0.32;
    let y1 = 0.72;
    let x2 = 0.0;
    let y2 = 1.0;
    let mut u = t;
    for _ in 0..8 {
        let x = cubic_bezier_point(u, x1, x2);
        let dx = cubic_bezier_deriv(u, x1, x2);
        if dx.abs() < 1e-6 {
            break;
        }
        u = (u - (x - t) / dx).clamp(0.0, 1.0);
    }
    cubic_bezier_point(u, y1, y2)
}

pub fn lerp_i32(a: i32, b: i32, t: f32) -> i32 {
    (a as f32 + (b - a) as f32 * t).round() as i32
}

/// Keep the bottom edge where it is while height changes. `to_y` is ignored so
/// a clamped panel origin cannot drag the pill off the taskbar.
pub fn lerp_pinned_bottom(from_y: i32, from_h: i32, _to_y: i32, to_h: i32, t: f32) -> (i32, i32) {
    let bottom = from_y + from_h;
    let h = lerp_i32(from_h, to_h, t).max(1);
    (bottom - h, h)
}

fn anim_generation() -> &'static std::sync::atomic::AtomicU64 {
    static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    &GEN
}

#[cfg(target_os = "windows")]
pub fn client_animations_enabled() -> bool {
    let mut enabled = BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&mut enabled as *mut BOOL).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    enabled.as_bool()
}

#[cfg(not(target_os = "windows"))]
pub fn client_animations_enabled() -> bool {
    false
}

#[cfg(target_os = "windows")]
pub fn find_app_hwnd() -> Option<HWND> {
    if let Some(hwnd) = stored_overlay_hwnd() {
        return Some(hwnd);
    }

    struct Context {
        pid: u32,
        found: Option<HWND>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut Context);
        let mut pid = 0;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != ctx.pid {
            return BOOL(1);
        }
        if is_gpui_overlay_window(hwnd) {
            ctx.found = Some(hwnd);
            return BOOL(0);
        }
        BOOL(1)
    }

    let mut ctx = Context {
        pid: std::process::id(),
        found: None,
    };

    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut _ as isize));
    }

    if let Some(hwnd) = ctx.found {
        *remembered_hwnd().lock() = Some(hwnd.0 as isize);
        return Some(hwnd);
    }
    None
}

#[cfg(target_os = "windows")]
fn is_gpui_overlay_window(hwnd: HWND) -> bool {
    let mut class_name = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, &mut class_name) };
    len > 0 && String::from_utf16_lossy(&class_name[..len as usize]) == "Zed::Window"
}

#[cfg(target_os = "windows")]
fn stored_overlay_hwnd() -> Option<HWND> {
    let stored = *remembered_hwnd().lock();
    stored.and_then(|raw| {
        let hwnd = HWND(raw as _);
        unsafe {
            if IsWindow(hwnd).as_bool() && is_gpui_overlay_window(hwnd) {
                Some(hwnd)
            } else {
                None
            }
        }
    })
}

pub fn start_window_drag() {
    #[cfg(target_os = "windows")]
    {
        // Release GPUI's client-area capture on the UI thread, then ask Windows
        // to treat the same press as a title-bar drag. This keeps the whole
        // borderless surface draggable without adding a visible handle.
        if let Some(hwnd) = find_app_hwnd() {
            unsafe {
                let _ = ReleaseCapture();
                let _ = PostMessageW(
                    hwnd,
                    WM_NCLBUTTONDOWN,
                    WPARAM(HTCAPTION as usize),
                    LPARAM(0),
                );
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn monitor_work_area(hwnd: HWND) -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor.is_invalid() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let work = info.rcWork;
        Some((work.left, work.top, work.right, work.bottom))
    }
}

#[cfg(target_os = "windows")]
fn preferred_startup_work_area() -> Option<(i32, i32, i32, i32)> {
    struct Found {
        primary: Option<(i32, i32, i32, i32)>,
        secondary: Option<(i32, i32, i32, i32)>,
    }

    unsafe extern "system" fn enum_mon(
        monitor: HMONITOR,
        _hdc: HDC,
        _lprc: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let found = &mut *(lparam.0 as *mut Found);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let work = info.rcWork;
            let area = (work.left, work.top, work.right, work.bottom);
            const MONITORINFOF_PRIMARY: u32 = 1;
            if info.dwFlags & MONITORINFOF_PRIMARY != 0 {
                found.primary = Some(area);
            } else if found.secondary.is_none() {
                found.secondary = Some(area);
            }
        }
        BOOL(1)
    }

    let mut found = Found {
        primary: None,
        secondary: None,
    };
    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(enum_mon),
            LPARAM(&mut found as *mut Found as isize),
        );
    }
    found.secondary.or(found.primary)
}

pub fn set_window_mode(is_expanded: bool) {
    #[cfg(target_os = "windows")]
    {
        overlay_animating().store(true, std::sync::atomic::Ordering::SeqCst);
        if is_expanded {
            overlay_expanded().store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(hwnd) = find_app_hwnd() {
                capture_previous_foreground(hwnd);
                let mut rect = RECT::default();
                unsafe {
                    let _ = GetWindowRect(hwnd, &mut rect);
                }
                save_bubble_rect(rect.left, rect.top, (rect.right - rect.left).max(1));
                apply_overlay_window_style(hwnd);
                activate_overlay_window(hwnd);
            }
        }

        let gen = anim_generation().fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;

        std::thread::spawn(move || {
            let Some(hwnd) = find_app_hwnd() else {
                overlay_animating().store(false, std::sync::atomic::Ordering::SeqCst);
                if !is_expanded {
                    overlay_expanded().store(false, std::sync::atomic::Ordering::SeqCst);
                }
                return;
            };
            unsafe {
                let mut rect = RECT::default();
                let _ = GetWindowRect(hwnd, &mut rect);
                let work = monitor_work_area(hwnd).unwrap_or((
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.bottom,
                ));

                let from_x = rect.left;
                let from_y = rect.top;
                let from_w = (rect.right - rect.left).max(1);
                let from_h = rect.bottom - rect.top;
                let bottom = from_y + from_h;
                let (bubble_w, bubble_h) = bubble_outer_size(hwnd);
                let (inset_w, inset_h) = chrome_inset(hwnd);
                let work_w = work.2 - work.0;
                let work_h = work.3 - work.1;
                let scale = window_scale(hwnd);
                let edge_pad = logical_to_device(PANEL_EDGE_PAD as f32, scale);

                let (to_x, to_y, to_w, to_h) = if is_expanded {
                    save_bubble_rect(rect.left, rect.top, bubble_w);
                    let (panel_w, panel_h) = panel_size_for_work_at_scale(work_w, work_h, scale);
                    let max_h = (bottom - work.1 - edge_pad).max(bubble_h);
                    let to_h = (panel_h + inset_h).min(max_h).max(1);
                    let max_w = work_w.max(1);
                    let to_w = (panel_w + inset_w).min(max_w).max(1);
                    let (clamped_x, _) = panel_origin_at_scale(
                        (from_x, from_y),
                        work,
                        (
                            to_w.saturating_sub(inset_w).max(1),
                            to_h.saturating_sub(inset_h).max(1),
                        ),
                        scale,
                    );
                    (clamped_x, bottom - to_h, to_w, to_h)
                } else {
                    (from_x, bottom - bubble_h, bubble_w, bubble_h)
                };

                let apply = |x, y, w, h| {
                    let _ = SetWindowPos(
                        hwnd,
                        HWND::default(),
                        x,
                        y,
                        w,
                        h,
                        SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOCOPYBITS,
                    );
                    apply_overlay_region(hwnd);
                };

                if !client_animations_enabled()
                    || (from_x == to_x && from_y == to_y && from_w == to_w && from_h == to_h)
                {
                    apply(to_x, to_y, to_w, to_h);
                    finish_window_mode(hwnd, is_expanded);
                    return;
                }

                let duration = std::time::Duration::from_millis(if is_expanded {
                    PANEL_OPEN_MS
                } else {
                    PANEL_CLOSE_MS
                });
                let started = std::time::Instant::now();
                loop {
                    if anim_generation().load(std::sync::atomic::Ordering::SeqCst) != gen {
                        return;
                    }
                    let t = (started.elapsed().as_secs_f32() / duration.as_secs_f32()).min(1.0);
                    let e = ease_drawer(t);
                    let (y, h) = lerp_pinned_bottom(from_y, from_h, to_y, to_h, e);
                    apply(
                        lerp_i32(from_x, to_x, e),
                        y,
                        lerp_i32(from_w, to_w, e).max(1),
                        h,
                    );
                    if t >= 1.0 {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                apply(to_x, to_y, to_w, to_h);
                finish_window_mode(hwnd, is_expanded);
            }
        });
    }
}

pub fn follow_current_virtual_desktop() {
    #[cfg(target_os = "windows")]
    {
        if is_overlay_hidden() {
            return;
        }
        use std::time::{Duration, Instant};
        static LAST: parking_lot::Mutex<Option<Instant>> = parking_lot::Mutex::new(None);
        {
            let mut last = LAST.lock();
            if last.is_some_and(|t| t.elapsed() < Duration::from_millis(400)) {
                return;
            }
            *last = Some(Instant::now());
        }

        let Some(hwnd) = find_app_hwnd() else {
            return;
        };
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let Ok(vdm) = CoCreateInstance::<_, IVirtualDesktopManager>(
                &VirtualDesktopManager,
                None,
                CLSCTX_ALL,
            ) else {
                return;
            };
            if vdm
                .IsWindowOnCurrentVirtualDesktop(hwnd)
                .ok()
                .is_some_and(|v| v.as_bool())
            {
                return;
            }
            let fg = GetForegroundWindow();
            if fg.is_invalid() || fg == hwnd {
                return;
            }
            if let Ok(desktop_id) = vdm.GetWindowDesktopId(fg) {
                let _ = vdm.MoveWindowToDesktop(hwnd, &desktop_id);
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub fn position_bubble_on_preferred_monitor(hwnd: HWND) {
    if let Some((left, top, right, bottom)) = preferred_startup_work_area() {
        let (w, h) = bubble_outer_size(hwnd);
        let scale = window_scale(hwnd);
        let x = left + ((right - left - w) / 2).max(0);
        let y =
            (bottom - h - logical_to_device(70.0, scale)).max(top + logical_to_device(20.0, scale));
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                x,
                y,
                w,
                h,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            apply_overlay_dwm(hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_grows_up_from_the_bubble() {
        let bubble_left = 700;
        let bubble_top = 600;
        let (left, top) = panel_origin(
            (bubble_left, bubble_top),
            (0, 0, 1920, 1080),
            (PANEL_WIDTH, PANEL_HEIGHT),
        );
        assert_eq!(left, bubble_left);
        assert_eq!(top, bubble_top + BUBBLE_HEIGHT as i32 - PANEL_HEIGHT);
    }

    #[test]
    fn panel_origin_stays_on_primary_work_area() {
        let (left, top) =
            panel_origin((-40, 2000), (0, 0, 1920, 1080), (PANEL_WIDTH, PANEL_HEIGHT));
        assert!(left >= 20);
        assert!(top >= 20);
        assert!(left + PANEL_WIDTH <= 1900);
        assert!(top + PANEL_HEIGHT <= 1060);
    }

    #[test]
    fn panel_size_fits_small_and_large_work_areas() {
        assert_eq!(panel_size_for_work(1920, 1080).0, BUBBLE_WIDTH as i32);
        assert_eq!(
            panel_size_for_work(1920, 1080),
            (PANEL_WIDTH, PANEL_MAX_HEIGHT)
        );
        assert_eq!(
            panel_size_for_work(400 + PANEL_WORK_MARGIN, 480 + PANEL_WORK_MARGIN),
            (PANEL_WIDTH, PANEL_MIN_HEIGHT)
        );
        assert_eq!(
            panel_size_for_work(540 + PANEL_WORK_MARGIN, 540 + PANEL_WORK_MARGIN),
            (PANEL_WIDTH, PANEL_HEIGHT)
        );
        assert_eq!(
            panel_size_for_work(300, 400),
            (300 - PANEL_WORK_MARGIN, 400 - PANEL_WORK_MARGIN)
        );
        assert_eq!(panel_size_for_work(0, 0), (PANEL_WIDTH, PANEL_HEIGHT));
    }

    #[test]
    fn ease_drawer_starts_fast_and_ends_at_one() {
        assert!((ease_drawer(0.0) - 0.0).abs() < 1e-5);
        assert!((ease_drawer(1.0) - 1.0).abs() < 1e-5);
        assert!(ease_drawer(0.2) > 0.2);
    }

    #[test]
    fn overlay_hidden_flag_round_trips() {
        assert!(!is_overlay_hidden());
        set_overlay_hidden(true);
        assert!(is_overlay_hidden());
        set_overlay_hidden(false);
        assert!(!is_overlay_hidden());
    }

    #[test]
    fn lerp_i32_hits_endpoints() {
        assert_eq!(lerp_i32(10, 20, 0.0), 10);
        assert_eq!(lerp_i32(10, 20, 1.0), 20);
        assert_eq!(lerp_i32(10, 20, 0.5), 15);
    }

    #[test]
    fn pinned_bottom_stays_put_when_growing_up() {
        let from_y = 918;
        let from_h = BUBBLE_HEIGHT as i32;
        let to_h = PANEL_HEIGHT;
        let to_y = from_y + from_h - to_h;
        let bottom = from_y + from_h;
        assert_eq!(to_y + to_h, bottom);
        let (y, h) = lerp_pinned_bottom(from_y, from_h, to_y, to_h, 0.5);
        assert_eq!(y + h, bottom);
    }

    #[test]
    fn panel_stays_on_second_monitor() {
        let bubble_left = 2500;
        let bubble_top = 900;
        let (left, top) = panel_origin(
            (bubble_left, bubble_top),
            (1920, 0, 3840, 1080),
            (PANEL_WIDTH, PANEL_HEIGHT),
        );
        assert_eq!(left, 2500);
        assert!(left >= 1920);
        assert!(left + PANEL_WIDTH <= 3840);
        assert!(top >= 0);
        assert!(top + PANEL_HEIGHT <= 1080);
        assert!(top < bubble_top);
    }

    #[test]
    fn panel_origin_fits_clamped_size_on_tiny_work_area() {
        let work = (0, 0, 300, 400);
        let size = panel_size_for_work(300, 400);
        let (left, top) = panel_origin((20, 350), work, size);
        assert!(left >= PANEL_EDGE_PAD);
        assert!(top >= PANEL_EDGE_PAD);
        assert!(left + size.0 <= 300 - PANEL_EDGE_PAD);
        assert!(top + size.1 <= 400 - PANEL_EDGE_PAD);
    }

    #[test]
    fn overlay_extended_style_drops_noactivate_only_when_expanded() {
        let noactivate = WS_EX_NOACTIVATE.0 as i32;
        let topmost = WS_EX_TOPMOST.0 as i32;
        let tool = WS_EX_TOOLWINDOW.0 as i32;
        let layered = 0x0008_0000;
        let collapsed = overlay_extended_style(layered, false);
        assert_eq!(collapsed & noactivate, noactivate);
        assert_eq!(collapsed & topmost, topmost);
        assert_eq!(collapsed & tool, tool);
        assert_eq!(collapsed & layered, layered);
        let expanded = overlay_extended_style(collapsed, true);
        assert_eq!(expanded & noactivate, 0);
        assert_eq!(expanded & topmost, topmost);
        assert_eq!(expanded & tool, tool);
        assert_eq!(expanded & layered, layered);
    }

    #[test]
    fn restore_previous_foreground_only_while_overlay_still_focused() {
        let overlay = 11;
        let editor = 22;
        let other = 33;
        assert!(should_restore_previous_foreground(
            overlay,
            overlay,
            Some(editor)
        ));
        assert!(!should_restore_previous_foreground(
            overlay,
            other,
            Some(editor)
        ));
        assert!(!should_restore_previous_foreground(
            overlay,
            overlay,
            Some(overlay)
        ));
        assert!(!should_restore_previous_foreground(overlay, overlay, None));
        assert!(!should_restore_previous_foreground(
            overlay,
            overlay,
            Some(0)
        ));
    }

    #[test]
    fn open_animation_end_does_not_steal_a_new_foreground_app() {
        let overlay = 11;
        let editor = 22;
        let other = 33;
        assert!(should_activate_overlay_after_open(
            overlay,
            overlay,
            Some(editor)
        ));
        assert!(should_activate_overlay_after_open(
            overlay,
            editor,
            Some(editor)
        ));
        assert!(should_activate_overlay_after_open(overlay, 0, Some(editor)));
        assert!(!should_activate_overlay_after_open(
            overlay,
            other,
            Some(editor)
        ));
    }

    #[test]
    fn dictation_target_never_returns_overlay_hwnd() {
        let overlay = 11;
        let editor = 22;
        let other = 33;
        assert_eq!(
            dictation_target_from_handles(Some(overlay), editor, Some(editor), true),
            Some(editor)
        );
        assert_eq!(
            dictation_target_from_handles(Some(overlay), overlay, Some(editor), true),
            Some(editor)
        );
        assert_eq!(
            dictation_target_from_handles(Some(overlay), other, Some(editor), true),
            Some(other)
        );
        assert_eq!(
            dictation_target_from_handles(Some(overlay), overlay, Some(editor), false),
            None
        );
        assert_eq!(
            dictation_target_from_handles(Some(overlay), overlay, Some(overlay), true),
            None
        );
        assert_eq!(
            dictation_target_from_handles(Some(overlay), overlay, None, false),
            None
        );
    }
}
