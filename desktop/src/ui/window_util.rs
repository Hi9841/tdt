#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, WPARAM};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible,
    PostMessageW, SetWindowPos, SystemParametersInfoW, HTCAPTION, HWND_TOPMOST,
    SPI_GETCLIENTAREAANIMATION, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOZORDER,
    SWP_SHOWWINDOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WM_NCLBUTTONDOWN,
};

pub const BUBBLE_WIDTH: f32 = 340.0;
pub const BUBBLE_HEIGHT: f32 = 44.0;
pub const PANEL_WIDTH: i32 = 360;
pub const PANEL_HEIGHT: i32 = 528;
pub const PANEL_MIN_WIDTH: i32 = 320;
pub const PANEL_MAX_WIDTH: i32 = 400;
pub const PANEL_MIN_HEIGHT: i32 = 460;
pub const PANEL_MAX_HEIGHT: i32 = 600;
pub const PANEL_OPEN_MS: u64 = 240;
pub const PANEL_CLOSE_MS: u64 = 180;

fn bubble_anchor() -> &'static parking_lot::Mutex<Option<(i32, i32)>> {
    static POS: parking_lot::Mutex<Option<(i32, i32)>> = parking_lot::Mutex::new(None);
    &POS
}

/// Size the expanded panel from the monitor work area so it stays on-screen
/// without clipping controls.
pub fn panel_size_for_work(work_w: i32, work_h: i32) -> (i32, i32) {
    if work_w <= 0 || work_h <= 0 {
        return (PANEL_WIDTH, PANEL_HEIGHT);
    }
    (
        (work_w - 40).clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH),
        (work_h - 48).clamp(PANEL_MIN_HEIGHT, PANEL_MAX_HEIGHT),
    )
}

/// Clamp the expanded panel to a monitor work area. Origins may be negative
/// or beyond the primary width when the overlay lives on a second display.
pub fn panel_origin(
    bubble: (i32, i32),
    work: (i32, i32, i32, i32),
    panel: (i32, i32),
) -> (i32, i32) {
    let pad = 20;
    let (bubble_left, bubble_top) = bubble;
    let (work_left, work_top, work_right, work_bottom) = work;
    let (panel_w, panel_h) = panel;
    let bubble_bottom = bubble_top + BUBBLE_HEIGHT as i32;
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

/// Keep the bottom edge stable while height changes, so the pill does not bounce
/// against the taskbar.
pub fn lerp_pinned_bottom(from_y: i32, from_h: i32, to_y: i32, to_h: i32, t: f32) -> (i32, i32) {
    let from_bottom = from_y + from_h;
    let to_bottom = to_y + to_h;
    let bottom = lerp_i32(from_bottom, to_bottom, t);
    let h = lerp_i32(from_h, to_h, t).max(1);
    (bottom - h, h)
}

fn anim_generation() -> &'static std::sync::atomic::AtomicU64 {
    static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    &GEN
}

#[cfg(target_os = "windows")]
fn client_animations_enabled() -> bool {
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

#[cfg(target_os = "windows")]
pub fn find_app_hwnd() -> Option<HWND> {
    struct Context {
        pid: u32,
        found: Option<HWND>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut Context);
        let mut pid = 0;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == ctx.pid && IsWindowVisible(hwnd).as_bool() {
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

    ctx.found
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
        if is_expanded {
            if let Some(hwnd) = find_app_hwnd() {
                let mut rect = RECT::default();
                unsafe {
                    let _ = GetWindowRect(hwnd, &mut rect);
                }
                let mut saved = bubble_anchor().lock();
                if saved.is_none() {
                    *saved = Some((rect.left, rect.top));
                }
            }
        }

        let gen = anim_generation().fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;

        std::thread::spawn(move || {
            let Some(hwnd) = find_app_hwnd() else {
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

                let (to_x, to_y, to_w, to_h) = if is_expanded {
                    let (orig_left, orig_top) = {
                        let mut saved = bubble_anchor().lock();
                        if saved.is_none() {
                            *saved = Some((rect.left, rect.top));
                        }
                        (*saved).expect("bubble origin")
                    };
                    let panel = panel_size_for_work(work.2 - work.0, work.3 - work.1);
                    let (left, top) = panel_origin((orig_left, orig_top), work, panel);
                    (left, top, panel.0, panel.1)
                } else {
                    let (left, top) = (*bubble_anchor().lock()).unwrap_or((rect.left, rect.top));
                    (left, top, BUBBLE_WIDTH as i32, BUBBLE_HEIGHT as i32)
                };

                let from_x = rect.left;
                let from_y = rect.top;
                let from_w = rect.right - rect.left;
                let from_h = rect.bottom - rect.top;

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
                };

                if !client_animations_enabled()
                    || (from_x == to_x && from_y == to_y && from_w == to_w && from_h == to_h)
                {
                    apply(to_x, to_y, to_w, to_h);
                    if !is_expanded {
                        bubble_anchor().lock().take();
                    }
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
                    apply(lerp_i32(from_x, to_x, e), y, lerp_i32(from_w, to_w, e), h);
                    if t >= 1.0 {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                apply(to_x, to_y, to_w, to_h);
                if !is_expanded {
                    bubble_anchor().lock().take();
                }
            }
        });
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
        let w = BUBBLE_WIDTH as i32;
        let h = BUBBLE_HEIGHT as i32;
        let x = left + ((right - left - w) / 2).max(0);
        let y = (bottom - h - 70).max(top + 20);
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
        assert_eq!(panel_size_for_work(1920, 1080), (400, 600));
        assert_eq!(
            panel_size_for_work(300, 400),
            (PANEL_MIN_WIDTH, PANEL_MIN_HEIGHT)
        );
    }

    #[test]
    fn ease_drawer_starts_fast_and_ends_at_one() {
        assert!((ease_drawer(0.0) - 0.0).abs() < 1e-5);
        assert!((ease_drawer(1.0) - 1.0).abs() < 1e-5);
        assert!(ease_drawer(0.2) > 0.2);
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
}
