#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMNCRP_DISABLED, DWMWA_BORDER_COLOR,
    DWMWA_CLOAKED, DWMWA_COLOR_NONE, DWMWA_NCRENDERING_POLICY, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_DONOTROUND,
};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CreateRectRgn, EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow,
    SetWindowRgn, HDC, HMONITOR, HRGN, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    MONITOR_DEFAULTTONULL,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::HiDpi::GetDpiForWindow;
#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, EnumWindows, GetClassNameW, GetClientRect, GetForegroundWindow,
    GetSystemMetrics, GetWindow, GetWindowLongPtrW, GetWindowLongW, GetWindowRect,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, IsZoomed, LoadImageW, SendMessageW,
    SetForegroundWindow, SetWindowLongPtrW, SetWindowLongW, SetWindowPos, ShowWindow,
    SystemParametersInfoW, GWLP_WNDPROC, GWL_EXSTYLE, GWL_STYLE, GW_HWNDPREV, HWND_BOTTOM,
    HWND_TOPMOST, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTSIZE, LR_SHARED, MA_NOACTIVATE,
    SM_CXICON, SM_CXSMICON, SPI_GETCLIENTAREAANIMATION, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOCOPYBITS, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WM_MOUSEACTIVATE, WM_SETICON, WNDPROC, WS_CAPTION,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP,
    WS_THICKFRAME,
};

/// The overlay window always has the panel's size. The pill is drawn at its
/// bottom center, and the window region limits input to the pill while the
/// panel is closed, so the rest never blocks clicks. The window never resizes
/// on screen, so Windows never shows a stretched frame.
pub const PANEL_WIDTH: i32 = 400;
pub const PANEL_HEIGHT: i32 = 540;
pub const PANEL_MIN_HEIGHT: i32 = 480;
pub const PANEL_MAX_HEIGHT: i32 = 620;
/// Inset from the monitor work area when choosing a panel size. 48 keeps the
/// existing 480-620 height range on typical displays while still fitting a
/// 400px-wide panel (400 + 48). These values are GPUI logical pixels and are
/// converted to device pixels before Win32 sizing calls.
const PANEL_WORK_MARGIN: i32 = 48;

fn overlay_expanded() -> &'static std::sync::atomic::AtomicBool {
    static FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &FLAG
}

/// Logical (x, y, w, h) inside the canvas that takes input. `None` is the whole canvas.
fn pill_hit_rect() -> &'static parking_lot::Mutex<Option<(f32, f32, f32, f32)>> {
    static RECT: parking_lot::Mutex<Option<(f32, f32, f32, f32)>> = parking_lot::Mutex::new(None);
    &RECT
}

/// Limit input on the collapsed overlay to `rect`. Only a change reaches Win32.
pub fn set_pill_hit_rect(rect: (f32, f32, f32, f32)) {
    {
        let mut current = pill_hit_rect().lock();
        if *current == Some(rect) {
            return;
        }
        *current = Some(rect);
    }
    #[cfg(target_os = "windows")]
    if let Some(hwnd) = find_app_hwnd() {
        unsafe { apply_overlay_region(hwnd) };
    }
}

fn remembered_hwnd() -> &'static parking_lot::Mutex<Option<isize>> {
    static HWND: parking_lot::Mutex<Option<isize>> = parking_lot::Mutex::new(None);
    &HWND
}

fn previous_foreground() -> &'static parking_lot::Mutex<Option<isize>> {
    static HWND: parking_lot::Mutex<Option<isize>> = parking_lot::Mutex::new(None);
    &HWND
}

/// Opens a file with its default program, for example a text file in the user's editor.
pub fn open_file(path: &std::path::Path) -> Result<(), String> {
    // ShellExecute runs a message loop while it starts the handler. On the UI
    // thread that loop would run GPUI tasks inside the current update and
    // panic with "RefCell already borrowed", so it runs on its own thread.
    let path = path.to_path_buf();
    std::thread::spawn(move || shell_open(&path))
        .join()
        .unwrap_or_else(|_| Err("Windows could not open the file".into()))
}

fn shell_open(path: &std::path::Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    // SAFETY: ShellExecute wants COM on its thread; this thread is ours alone.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    // SAFETY: both strings are null-terminated and live until the call returns.
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if com {
        // SAFETY: balances the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
    // ShellExecute reports success with a value above 32.
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("Windows could not open {}", path.display()))
    }
}

/// Shell popups the pill must not cover. The taskbar is absent on purpose:
/// a topmost taskbar was painting over the pill and leaving it there.
pub fn overlay_should_yield_to(class_name: &str) -> bool {
    matches!(
        class_name,
        "#32768"
            | "Windows.UI.Core.CoreWindow"
            | "XamlExplorerHostIslandWindow"
            | "MultitaskingViewFrame"
            | "ForegroundStaging"
            | "NotifyIconOverflowWindow"
            | "TopLevelWindowForOverflowXamlIsland"
            | "Shell_InputSwitchTopLevelWindow"
    )
}

/// Raise when topmost style was dropped, or when an app or the taskbar is
/// above the pill. `covered_by_app_or_taskbar` is false when nothing is above
/// or the only windows above are shell popups from `overlay_should_yield_to`.
pub fn overlay_should_raise(style_is_current: bool, covered_by_app_or_taskbar: bool) -> bool {
    !style_is_current || covered_by_app_or_taskbar
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

/// External dictation target. An unidentified overlay types nowhere, because
/// the live foreground may be the TDT window itself. A known overlay is never
/// returned. Otherwise this is the live foreground, or a still-valid previous
/// hwnd captured when settings opened.
pub fn dictation_target_from_handles(
    overlay: Option<isize>,
    current_fg: isize,
    previous: Option<isize>,
    previous_is_live: bool,
) -> Option<isize> {
    let overlay = overlay.filter(|hwnd| *hwnd != 0)?;
    if current_fg != 0 && current_fg != overlay {
        return Some(current_fg);
    }
    match previous {
        Some(prev) if prev != 0 && prev != overlay && previous_is_live => Some(prev),
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
        set_window_icon(hwnd);
    }
}

/// Put TDT's icon on the window so the taskbar, Alt-Tab, Task Manager and
/// Start Menu search stop showing the blank default Zed::Window icon. The
/// image is resource ordinal 1 — the same "1 ICON ..." the .lnk and tray use.
#[cfg(target_os = "windows")]
pub fn set_window_icon(hwnd: HWND) {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    if hwnd.is_invalid() {
        return;
    }
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            return;
        };
        // MAKEINTRESOURCE(1): the ordinal declared by assets/tdt.rc.
        const TDT_ICON: windows::core::PCWSTR = windows::core::PCWSTR(1usize as _);
        let small = GetSystemMetrics(SM_CXSMICON).max(16);
        let big = GetSystemMetrics(SM_CXICON).max(16);
        if let Ok(icon) = LoadImageW(
            instance,
            TDT_ICON,
            IMAGE_ICON,
            big,
            big,
            LR_DEFAULTSIZE | LR_SHARED,
        ) {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                WPARAM(ICON_BIG as _),
                LPARAM(icon.0 as isize),
            );
        }
        if let Ok(icon) = LoadImageW(instance, TDT_ICON, IMAGE_ICON, small, small, LR_SHARED) {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                WPARAM(ICON_SMALL as _),
                LPARAM(icon.0 as isize),
            );
        }
    }
}

/// Put the pill back at the top of the topmost band. The startup pass runs
/// once and can miss the HWND; the taskbar can also step above a window that
/// already has WS_EX_TOPMOST. Call this from the existing overlay poll.
#[cfg(target_os = "windows")]
pub fn ensure_overlay_topmost() {
    let Some(hwnd) = find_app_hwnd() else {
        return;
    };
    let ex = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) };
    if fullscreen_app_in_front(hwnd) {
        // A window over a fullscreen game drops it out of fullscreen, and a
        // raise on every poll pulls the game in and out of focus. Step behind
        // it once; the next poll after the game leaves the front raises again.
        if ex & WS_EX_TOPMOST.0 as i32 != 0 {
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    HWND_BOTTOM,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOCOPYBITS,
                );
            }
        }
        return;
    }
    let expanded = overlay_expanded().load(std::sync::atomic::Ordering::SeqCst);
    let style_is_current = overlay_extended_style(ex, expanded) == ex;
    let covered = covered_by_app_or_taskbar(hwnd);
    if !overlay_should_raise(style_is_current, covered) {
        return;
    }
    if !style_is_current {
        apply_overlay_window_style(hwnd);
        return;
    }
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOCOPYBITS,
        );
    }
}

/// True when another app fills the overlay's whole monitor, as a game or a
/// fullscreen video does. A maximized window fills only the work area, and
/// the desktop and shell views are not apps.
#[cfg(target_os = "windows")]
fn fullscreen_app_in_front(overlay: HWND) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg == overlay || IsZoomed(fg).as_bool() {
            return false;
        }
        let class_name = window_class_name(fg);
        if matches!(class_name.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd")
            || overlay_should_yield_to(&class_name)
        {
            return false;
        }
        let monitor = MonitorFromWindow(fg, MONITOR_DEFAULTTONULL);
        if monitor.is_invalid() || monitor != MonitorFromWindow(overlay, MONITOR_DEFAULTTONEAREST) {
            return false;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let mut rect = RECT::default();
        if !GetMonitorInfoW(monitor, &mut info).as_bool() || GetWindowRect(fg, &mut rect).is_err() {
            return false;
        }
        let screen = info.rcMonitor;
        rect.left <= screen.left
            && rect.top <= screen.top
            && rect.right >= screen.right
            && rect.bottom >= screen.bottom
    }
}

/// GPUI's window procedure, saved by `keep_pill_from_activating`.
#[cfg(target_os = "windows")]
static GPUI_WNDPROC: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// GPUI answers WM_MOUSEACTIVATE with MA_ACTIVATE, so a click on the pill
/// would take focus from the app the user dictates into, or knock a game
/// out of fullscreen. Only the open panel may take focus.
#[cfg(target_os = "windows")]
pub fn keep_pill_from_activating(hwnd: HWND) {
    use std::sync::atomic::Ordering;
    if hwnd.is_invalid() || GPUI_WNDPROC.load(Ordering::SeqCst) != 0 {
        return;
    }
    unsafe {
        let gpui = GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
        if gpui == 0 {
            return;
        }
        // Saved before the swap, so the first message through the new
        // procedure already finds GPUI's.
        GPUI_WNDPROC.store(gpui, Ordering::SeqCst);
        SetWindowLongPtrW(hwnd, GWLP_WNDPROC, overlay_wndproc as *const () as isize);
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn overlay_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use std::sync::atomic::Ordering;
    if msg == WM_MOUSEACTIVATE && !overlay_expanded().load(Ordering::SeqCst) {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    // SAFETY: the value is the non-null WNDPROC read from this window.
    let gpui: WNDPROC = std::mem::transmute(GPUI_WNDPROC.load(Ordering::SeqCst));
    CallWindowProcW(gpui, hwnd, msg, wparam, lparam)
}

#[cfg(target_os = "windows")]
fn window_class_name(hwnd: HWND) -> String {
    let mut class_name = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut class_name) };
    if len <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&class_name[..len as usize])
    }
}

#[cfg(target_os = "windows")]
fn window_is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
    }
}

#[cfg(target_os = "windows")]
fn covered_by_app_or_taskbar(hwnd: HWND) -> bool {
    let mut above = unsafe { GetWindow(hwnd, GW_HWNDPREV) }.ok();
    const MAX_WINDOW_WALK: usize = 128;
    for _ in 0..MAX_WINDOW_WALK {
        let Some(current) = above else {
            return false;
        };
        let visible = unsafe { IsWindowVisible(current).as_bool() };
        if current != hwnd && visible && !window_is_cloaked(current) {
            let class_name = window_class_name(current);
            if !overlay_should_yield_to(&class_name) {
                return true;
            }
        }
        above = unsafe { GetWindow(current, GW_HWNDPREV) }.ok();
    }
    false
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
    // curve over GPUI's GPU-painted corner. So the region is a plain
    // rectangle around the pill; the transparent surface draws the round shape.
    // No region (the whole window) while the panel is open.
    let region = pill_region(hwnd).unwrap_or_default();
    // On success the system owns the region.
    let _ = SetWindowRgn(hwnd, region, true);
}

#[cfg(target_os = "windows")]
unsafe fn pill_region(hwnd: HWND) -> Option<HRGN> {
    let collapsed = !overlay_expanded().load(std::sync::atomic::Ordering::SeqCst);
    let (x, y, w, h) = (*pill_hit_rect().lock()).filter(|_| collapsed)?;
    let mut outer = RECT::default();
    let mut origin = POINT::default();
    GetWindowRect(hwnd, &mut outer).ok()?;
    ClientToScreen(hwnd, &mut origin).as_bool().then_some(())?;
    let scale = window_scale(hwnd);
    let (dx, dy) = (origin.x - outer.left, origin.y - outer.top);
    let region = CreateRectRgn(
        dx + (x * scale).floor() as i32,
        dy + (y * scale).floor() as i32,
        dx + ((x + w) * scale).ceil() as i32,
        dy + ((y + h) * scale).ceil() as i32,
    );
    (!region.is_invalid()).then_some(region)
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
fn apply_window_mode(hwnd: HWND, is_expanded: bool) {
    if !is_expanded {
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
    // Input follows the new mode: the whole panel, or only the pill.
    unsafe { apply_overlay_region(hwnd) };
}

/// GPUI's popup can finish creation without WS_VISIBLE when the first frame
/// was delayed. Show it without activating.
#[cfg(target_os = "windows")]
pub fn show_overlay_window(hwnd: HWND) {
    if hwnd.is_invalid() {
        return;
    }
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
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

/// Outer window size for the panel on the work area `work`.
#[cfg(target_os = "windows")]
fn canvas_outer_size(hwnd: HWND, work: (i32, i32, i32, i32)) -> (i32, i32) {
    let (dw, dh) = chrome_inset(hwnd);
    let (w, h) = panel_size_for_work_at_scale(work.2 - work.0, work.3 - work.1, window_scale(hwnd));
    (w + dw, h + dh)
}

/// Make the overlay a true popup, strip resize handles, and keep it at the
/// panel's size for its monitor without an overlapped-window frame.
pub fn lock_overlay_chrome() {
    #[cfg(target_os = "windows")]
    {
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
            let Some(work) = monitor_work_area(hwnd) else {
                return;
            };
            let mut rect = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rect);
            let (want_w, want_h) = canvas_outer_size(hwnd, work);
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

#[cfg(target_os = "windows")]
pub fn client_animations_enabled() -> bool {
    // Cached; the Win32 call is cheap but this runs on every render and the
    // value only changes via the system settings dialog.
    static CACHE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
    static LAST_CHECK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }
    let now = now_ms();
    let last = LAST_CHECK.load(std::sync::atomic::Ordering::Relaxed);
    if now.saturating_sub(last) < 500 {
        return CACHE.load(std::sync::atomic::Ordering::Relaxed);
    }
    let mut enabled = BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&mut enabled as *mut BOOL).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    CACHE.store(enabled.as_bool(), std::sync::atomic::Ordering::Relaxed);
    LAST_CHECK.store(now, std::sync::atomic::Ordering::Relaxed);
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
    window_class_name(hwnd) == "Zed::Window"
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

/// Switch between the pill and the open panel. The window keeps its size;
/// only focus behavior and the input region change.
pub fn set_window_mode(is_expanded: bool) {
    #[cfg(target_os = "windows")]
    {
        overlay_expanded().store(is_expanded, std::sync::atomic::Ordering::SeqCst);
        let Some(hwnd) = find_app_hwnd() else {
            return;
        };
        if is_expanded {
            capture_previous_foreground(hwnd);
        }
        let raw = hwnd.0 as isize;
        // Focus changes send window messages back into GPUI; make them off
        // the UI thread so they never arrive in the middle of an update.
        std::thread::spawn(move || apply_window_mode(HWND(raw as _), is_expanded));
    }
}

pub fn follow_current_virtual_desktop() {
    #[cfg(target_os = "windows")]
    {
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
            thread_local! {
                static VDM: Option<IVirtualDesktopManager> = unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    CoCreateInstance::<_, IVirtualDesktopManager>(
                        &VirtualDesktopManager,
                        None,
                        CLSCTX_ALL,
                    )
                    .ok()
                };
            }
            VDM.with(|vdm| {
                let Some(vdm) = vdm else {
                    return;
                };
                if vdm
                    .IsWindowOnCurrentVirtualDesktop(hwnd)
                    .is_ok_and(|v| v.as_bool())
                {
                    return;
                }
                let fg = GetForegroundWindow();
                if fg.is_invalid() || fg == hwnd {
                    return;
                }
                if let Ok(desktop_id) = vdm.GetWindowDesktopId(fg) {
                    let _ = vdm.MoveWindowToDesktop(hwnd, &desktop_id);
                    apply_overlay_window_style(hwnd);
                }
            });
        }
    }
}

#[cfg(target_os = "windows")]
pub fn position_bubble_on_preferred_monitor(hwnd: HWND) {
    if let Some((left, top, right, bottom)) = preferred_startup_work_area() {
        let (w, h) = canvas_outer_size(hwnd, (left, top, right, bottom));
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
