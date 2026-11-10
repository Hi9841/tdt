use async_channel::{Receiver, Sender};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    PressStarted,
    /// A hold came up. The `Instant` is the key-up: a hold asks the take to
    /// stop when the user lets go.
    HoldReleased(Instant),
    /// A short tap. The `Instant` is the key-down: a tap that stops a take
    /// asks to stop when the user presses it, not when the key comes up.
    Toggled(Instant),
    Captured(HotkeyBinding),
    CaptureCancelled,
    /// No key: a tray click or a click on the pill asks the UI loop to run now.
    Wake,
}

/// A press longer than this records until release instead of toggling.
pub const HOLD_THRESHOLD: Duration = Duration::from_millis(350);

/// A rebind request that is left unanswered stops listening for keys after
/// this long, so a forgotten capture cannot swallow the keyboard.
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(6);

/// Virtual-key codes of the side mouse buttons (XBUTTON1 and XBUTTON2).
const VK_MOUSE4: u16 = 0x05;
const VK_MOUSE5: u16 = 0x06;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub vk: u16,
}

impl HotkeyBinding {
    pub const DEFAULT: Self = Self {
        ctrl: true,
        alt: false,
        shift: false,
        win: false,
        vk: 0xBA,
    };

    pub fn parse(raw: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut win = false;
        let mut vk = None;
        for part in raw.split('+') {
            let token = part.trim();
            if token.is_empty() {
                continue;
            }
            match token.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" | "option" => alt = true,
                "shift" => shift = true,
                "win" | "windows" | "super" | "meta" => win = true,
                other => vk = parse_vk(other),
            }
        }
        let binding = Self {
            ctrl,
            alt,
            shift,
            win,
            vk: vk?,
        };
        binding.is_valid().then_some(binding)
    }

    pub fn display(self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.win {
            parts.push("Win".to_string());
        }
        parts.push(vk_label(self.vk));
        parts.join("+")
    }

    pub fn is_valid(self) -> bool {
        if self.vk == 0 || self.vk == 0x1B || is_modifier_vk(self.vk) {
            return false;
        }
        let has_mod = self.ctrl || self.alt || self.shift || self.win;
        let is_function = (0x70..=0x87).contains(&self.vk);
        has_mod || is_function || self.is_mouse()
    }

    /// Mouse 4 and Mouse 5, the side buttons. Other mouse buttons stay free.
    pub fn is_mouse(self) -> bool {
        matches!(self.vk, VK_MOUSE4 | VK_MOUSE5)
    }
}

impl Default for HotkeyBinding {
    fn default() -> Self {
        Self::DEFAULT
    }
}

static BINDING: Mutex<HotkeyBinding> = Mutex::new(HotkeyBinding::DEFAULT);
static CAPTURE: AtomicBool = AtomicBool::new(false);
static CAPTURE_START: Mutex<Option<Instant>> = Mutex::new(None);
static PRESS_START: Mutex<Option<Instant>> = Mutex::new(None);
static SENDER: Mutex<Option<Sender<HotkeyAction>>> = Mutex::new(None);

pub fn set_binding(binding: HotkeyBinding) {
    if binding.is_valid() {
        *BINDING.lock() = binding;
        // The mark is for the key of the binding before.
        SHORTCUT_DOWN.store(false, Ordering::SeqCst);
    }
}

/// The shortcut key is down from a press that TDT kept from the apps. TDT
/// keeps the repeats of the key and its key-up from the apps too. A hold that
/// ends with the modifier up first gave the repeats of the key to the app:
/// the app got characters that the user did not type, and TDT then did not
/// type the text, because a key went down.
static SHORTCUT_DOWN: AtomicBool = AtomicBool::new(false);
/// When the shortcut key last went down or repeated, as a Windows tick count.
static SHORTCUT_DOWN_TICK: AtomicU32 = AtomicU32::new(0);
/// A held key repeats after 1 s at most, the longest repeat delay of
/// Windows. A key-down after a longer time is a new press: the key-up of the
/// press before did not reach the hook.
const REPEAT_WITHIN_MS: u32 = 1_500;

/// Marks the shortcut key as down from a press that TDT kept.
#[cfg(target_os = "windows")]
fn shortcut_went_down() {
    SHORTCUT_DOWN_TICK.store(tick_count(), Ordering::SeqCst);
    SHORTCUT_DOWN.store(true, Ordering::SeqCst);
}

/// True for a key-down of the shortcut key that is a repeat of a press that
/// TDT kept.
#[cfg(target_os = "windows")]
fn shortcut_repeats() -> bool {
    if !SHORTCUT_DOWN.load(Ordering::SeqCst) {
        return false;
    }
    let now = tick_count();
    // Tick counts wrap, so compare them through their difference.
    if now.wrapping_sub(SHORTCUT_DOWN_TICK.load(Ordering::SeqCst)) > REPEAT_WITHIN_MS {
        SHORTCUT_DOWN.store(false, Ordering::SeqCst);
        return false;
    }
    SHORTCUT_DOWN_TICK.store(now, Ordering::SeqCst);
    true
}

pub fn current_binding() -> HotkeyBinding {
    *BINDING.lock()
}

pub fn begin_capture() {
    // A press was possibly still active; never leave the app thinking it is.
    if PRESS_START.lock().take().is_some() {
        send_action(HotkeyAction::HoldReleased(Instant::now()));
    }
    *CAPTURE_START.lock() = Some(Instant::now());
    CAPTURE.store(true, Ordering::SeqCst);
}

pub fn end_capture() {
    *CAPTURE_START.lock() = None;
    CAPTURE.store(false, Ordering::SeqCst);
}

/// Cancel an abandoned capture from the application poll loop. Keyboard hooks
/// only run when a key changes, so relying on the hook alone would leave the
/// settings UI in capture mode forever when the user walks away.
pub fn poll_capture_timeout() {
    if CAPTURE.load(Ordering::SeqCst) && capture_expired() {
        end_capture();
        send_action(HotkeyAction::CaptureCancelled);
    }
}

fn capture_expired() -> bool {
    CAPTURE_START
        .lock()
        .is_some_and(|start| start.elapsed() >= CAPTURE_TIMEOUT)
}

/// Ends the wait of the UI loop. Its idle tick is slow, and a click must
/// not wait for it.
pub fn wake_loop() {
    send_action(HotkeyAction::Wake);
}

/// Ends a press whose key-up did not reach the hook: the lock screen or an
/// administrator window took it. Windows still knows that a modifier of the
/// shortcut is up. A shortcut with no modifier has no such sign: Windows
/// does not keep the state of a key that the hook holds back.
#[cfg(target_os = "windows")]
pub fn poll_missed_release() {
    if PRESS_START.lock().is_none() {
        return;
    }
    let binding = current_binding();
    let (ctrl, alt, shift, win) = modifiers_down();
    let released = (binding.ctrl && !ctrl)
        || (binding.alt && !alt)
        || (binding.shift && !shift)
        || (binding.win && !win);
    if released {
        finish_press();
    }
}

#[cfg(not(target_os = "windows"))]
pub fn poll_missed_release() {}

fn send_action(action: HotkeyAction) {
    if let Some(ref sender) = *SENDER.lock() {
        // Unbounded, so this never waits inside the hook. The receiving
        // loop wakes at once instead of at its next poll.
        let _ = sender.try_send(action);
    }
}

/// The key that last counted as typing, for the delivery log. 0 is none.
static TYPED_KEY: AtomicU16 = AtomicU16::new(0);

/// Forget earlier keys. Called when the shortcut goes down, and when a click
/// starts or stops a dictation.
pub fn reset_user_typed() {
    TYPED_KEY.store(0, Ordering::SeqCst);
}

/// A physical non-modifier key arrived, or a key was already down at release.
pub fn note_user_typed(vk: u16) {
    TYPED_KEY.store(vk.max(1), Ordering::SeqCst);
}

/// The key the user pressed since the dictation started, if any.
pub fn user_typed() -> Option<u16> {
    Some(TYPED_KEY.load(Ordering::SeqCst)).filter(|&vk| vk != 0)
}

/// Mouse buttons are 0x01 through 0x06. The shortcut key and modifiers are not typing.
pub fn note_physical_key(vk: u16, binding_vk: u16) {
    if vk > 0x06 && !is_modifier_vk(vk) && vk != binding_vk {
        note_user_typed(vk);
    }
}

/// Keys the hook saw go down and not up. Keys that a program sent are not here.
static PHYSICAL_DOWN: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];

fn is_mouse_vk(vk: u16) -> bool {
    matches!(vk, 0x01 | 0x02 | 0x04 | 0x05 | 0x06)
}

/// The shortcut went down. Key repeat sends more downs; only the first counts.
fn start_press() {
    let mut start_guard = PRESS_START.lock();
    if start_guard.is_none() {
        *start_guard = Some(Instant::now());
        reset_user_typed();
        send_action(HotkeyAction::PressStarted);
    }
}

/// A valid capture replaces the shortcut. An invalid one, such as a lone
/// letter, ends capture instead of swallowing input until the timeout.
fn finish_capture(captured: HotkeyBinding) {
    end_capture();
    if captured.is_valid() {
        set_binding(captured);
        send_action(HotkeyAction::Captured(captured));
    } else {
        send_action(HotkeyAction::CaptureCancelled);
    }
}

fn finish_press() {
    let Some(start) = PRESS_START.lock().take() else {
        return;
    };
    let action = if start.elapsed() > HOLD_THRESHOLD {
        HotkeyAction::HoldReleased(Instant::now())
    } else {
        HotkeyAction::Toggled(start)
    };
    send_action(action);
}

pub struct HotkeyListener {
    running: Arc<AtomicBool>,
}

impl HotkeyListener {
    #[cfg(target_os = "windows")]
    pub fn start() -> Result<(Self, Receiver<HotkeyAction>), String> {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, GetMessageW, KillTimer, SetTimer, MSG, WM_TIMER,
        };

        let (sender, receiver) = async_channel::unbounded();
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        let running = Arc::new(AtomicBool::new(true));
        let running_thread = Arc::clone(&running);
        {
            let mut guard = SENDER.lock();
            if guard.is_some() {
                return Err("Hotkey listener is already running.".to_string());
            }
            *guard = Some(sender);
        }

        // SAFETY: this thread installs the hooks, runs their message loop, and removes them, as
        // WH_KEYBOARD_LL and WH_MOUSE_LL require. `msg` outlives the loop. The timer has no
        // window, so its messages come to this loop.
        std::thread::spawn(move || unsafe {
            // Each key event of every app waits for this thread, and so does
            // each character TDT types.
            crate::raise_thread();
            HOOK_THREAD_ID.store(
                windows::Win32::System::Threading::GetCurrentThreadId(),
                Ordering::SeqCst,
            );
            match install_hooks() {
                Err(error) => {
                    let _ = ready_tx.send(Err(format!(
                        "Could not install the keyboard hook. Restart TDT. {error}"
                    )));
                    return;
                }
                Ok(Some(mouse_error)) => crate::append_log(&format!(
                    "Could not install the mouse hook, so a mouse button cannot be the shortcut. {mouse_error}"
                )),
                Ok(None) => {}
            }
            let _ = ready_tx.send(Ok(()));
            let timer = SetTimer(HWND::default(), 0, HOOK_WATCH_MS, None);
            let mut watch = HookWatch::default();
            let mut msg = MSG::default();
            while running_thread.load(Ordering::Relaxed)
                && GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool()
            {
                if msg.message == WM_TIMER && msg.hwnd == HWND::default() {
                    watch.check();
                } else {
                    let _ = DispatchMessageW(&msg);
                }
            }
            let _ = KillTimer(HWND::default(), timer);
            remove_hooks();
            if running_thread.load(Ordering::Relaxed) {
                crate::append_log("The message loop of the hooks ended. The shortcut does not work until TDT starts again.");
            }
        });

        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok((
                Self {
                    running: running.clone(),
                },
                receiver,
            )),
            failed => {
                // A hook that installs after the timeout must not keep
                // swallowing the shortcut for a listener that is gone, and a
                // later start must not find the old sender.
                drop(Self { running });
                *SENDER.lock() = None;
                Err(match failed {
                    Ok(Err(error)) => error,
                    _ => "Could not install the keyboard hook. Restart TDT.".to_string(),
                })
            }
        }
    }

    #[cfg(target_os = "windows")]
    fn shutdown_thread(&self) {
        self.running.store(false, Ordering::SeqCst);
        let thread_id = HOOK_THREAD_ID.load(Ordering::SeqCst);
        if thread_id != 0 {
            // SAFETY: PostThreadMessageW only queues WM_QUIT; a dead thread id makes it fail.
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
                let _ = PostThreadMessageW(
                    thread_id,
                    WM_QUIT,
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                );
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub fn start() -> Result<(Self, Receiver<HotkeyAction>), String> {
        let (_sender, receiver) = async_channel::unbounded();
        let running = Arc::new(AtomicBool::new(true));
        Ok((Self { running }, receiver))
    }
}

#[cfg(target_os = "windows")]
impl Drop for HotkeyListener {
    fn drop(&mut self) {
        self.shutdown_thread();
    }
}

#[cfg(target_os = "windows")]
static HOOK_THREAD_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(target_os = "windows")]
static HOOK_PTR: std::sync::atomic::AtomicPtr<core::ffi::c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

#[cfg(target_os = "windows")]
static MOUSE_HOOK_PTR: std::sync::atomic::AtomicPtr<core::ffi::c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

/// When a hook last got an event, as a Windows tick count.
#[cfg(target_os = "windows")]
static HOOK_SEEN_TICK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The time between two checks of the hooks.
#[cfg(target_os = "windows")]
const HOOK_WATCH_MS: u32 = 2_000;
/// Input this much newer than the last hook event did not reach the hooks.
#[cfg(target_os = "windows")]
const UNSEEN_INPUT_MS: i32 = 1_000;

/// Key events and mouse events that the hooks got since the last check of the
/// watch, for its log line.
#[cfg(target_os = "windows")]
static KEY_EVENTS: AtomicU32 = AtomicU32::new(0);
#[cfg(target_os = "windows")]
static MOUSE_EVENTS: AtomicU32 = AtomicU32::new(0);

#[cfg(target_os = "windows")]
fn tick_count() -> u32 {
    // SAFETY: GetTickCount takes no pointers.
    unsafe { windows::Win32::System::SystemInformation::GetTickCount() }
}

#[cfg(target_os = "windows")]
fn note_hook_event(events: &AtomicU32) {
    HOOK_SEEN_TICK.store(tick_count(), Ordering::Relaxed);
    events.fetch_add(1, Ordering::Relaxed);
}

/// Installs the keyboard hook and the mouse hook on the calling thread.
/// Without the mouse hook only a mouse shortcut fails, so that is no error:
/// the result then holds the cause.
#[cfg(target_os = "windows")]
unsafe fn install_hooks() -> Result<Option<String>, String> {
    use windows::Win32::Foundation::HINSTANCE;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_KEYBOARD_LL, WH_MOUSE_LL};
    let module = GetModuleHandleW(None).unwrap_or_default();
    let keyboard = SetWindowsHookExW(
        WH_KEYBOARD_LL,
        Some(hotkey_hook_proc),
        HINSTANCE(module.0),
        0,
    )
    .map_err(|error| error.to_string())?;
    HOOK_PTR.store(keyboard.0, Ordering::SeqCst);
    let mouse_error =
        match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), HINSTANCE(module.0), 0) {
            Ok(mouse) => {
                MOUSE_HOOK_PTR.store(mouse.0, Ordering::SeqCst);
                None
            }
            Err(error) => Some(error.to_string()),
        };
    HOOK_SEEN_TICK.store(tick_count(), Ordering::Relaxed);
    Ok(mouse_error)
}

/// What Windows said when TDT removed one hook.
#[cfg(target_os = "windows")]
enum HookEnd {
    /// TDT had no hook of this kind.
    NotInstalled,
    /// Windows still had the hook.
    Removed,
    /// Windows did not know the hook: it removed the hook before.
    Gone(String),
}

#[cfg(target_os = "windows")]
impl std::fmt::Display for HookEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled => f.write_str("was not installed"),
            Self::Removed => f.write_str("was installed"),
            Self::Gone(error) => write!(f, "was gone ({error})"),
        }
    }
}

/// Removes the hooks on the thread that installed them. Returns what Windows
/// said for the mouse hook and for the keyboard hook.
#[cfg(target_os = "windows")]
unsafe fn remove_hooks() -> [HookEnd; 2] {
    use windows::Win32::UI::WindowsAndMessaging::{UnhookWindowsHookEx, HHOOK};
    [&MOUSE_HOOK_PTR, &HOOK_PTR].map(|slot| {
        let hook = slot.swap(std::ptr::null_mut(), Ordering::SeqCst);
        if hook.is_null() {
            return HookEnd::NotInstalled;
        }
        match UnhookWindowsHookEx(HHOOK(hook)) {
            Ok(()) => HookEnd::Removed,
            Err(error) => HookEnd::Gone(error.to_string()),
        }
    })
}

/// Windows removes a low-level hook that answers too slowly, and gives no
/// notice: the shortcut then does nothing until TDT starts again. Windows
/// also reports the time of the last input. Input that no hook saw means the
/// hooks are gone, so this installs them again.
///
/// The hooks also see nothing while an administrator window has the input:
/// that case is no reason for an install. For other input that the hooks
/// cannot see (the lock screen), the installs become less frequent, down to
/// about one each minute, until a hook sees an event again.
#[cfg(target_os = "windows")]
#[derive(Default)]
struct HookWatch {
    /// Installs in sequence that no hook event followed.
    failed: u32,
    /// Checks to skip before the next install.
    skip: u32,
    /// The last line in the log. Touch input can cause an install each
    /// minute, and the log needs only one line for that.
    logged: Option<Instant>,
    /// Installs since that line that got no line.
    unlogged: u32,
}

#[cfg(target_os = "windows")]
impl HookWatch {
    /// Runs on the hook thread.
    unsafe fn check(&mut self) {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
        let mut last = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // The events of the time since the last check, for the log line.
        let keys = KEY_EVENTS.swap(0, Ordering::Relaxed);
        let mouse = MOUSE_EVENTS.swap(0, Ordering::Relaxed);
        if !GetLastInputInfo(&mut last).as_bool() {
            return;
        }
        // Tick counts wrap, so compare them through their difference.
        let seen = HOOK_SEEN_TICK.load(Ordering::Relaxed);
        let unseen_ms = last.dwTime.wrapping_sub(seen) as i32;
        if unseen_ms < UNSEEN_INPUT_MS {
            self.failed = 0;
            self.skip = 0;
            return;
        }
        // An administrator window in front takes the input: Windows gives it
        // to no hook of a normal program. The hooks are still there.
        let front = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
        let front_app = (!front.is_invalid()).then(|| crate::paste::Target::of(front.0 as isize));
        if front_app
            .as_ref()
            .is_some_and(crate::paste::Target::blocks_input)
        {
            return;
        }
        if self.skip > 0 {
            self.skip -= 1;
            return;
        }
        let [mouse_end, keyboard_end] = remove_hooks();
        let installed = install_hooks();
        // A hook that Windows removed is the case that this watch is for. A
        // hook that was still installed means input that no hook can get.
        // The line holds the data that tells the two apart.
        let gone =
            matches!(mouse_end, HookEnd::Gone(_)) || matches!(keyboard_end, HookEnd::Gone(_));
        let line_due = |limit: u64| {
            self.logged
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(limit))
        };
        if (gone && line_due(600)) || (self.failed == 0 && line_due(3600)) {
            self.logged = Some(Instant::now());
            let app = front_app.map_or_else(|| "no window".to_string(), |app| app.to_string());
            let result = match installed {
                Ok(None) => "TDT installed the hooks again.".to_string(),
                Ok(Some(error)) => {
                    format!(
                        "TDT installed the keyboard hook again, but not the mouse hook: {error}"
                    )
                }
                Err(error) => format!("TDT could not install the hooks again: {error}"),
            };
            crate::append_log(&format!(
                "The hooks got no event for input that Windows reported {unseen_ms} ms after the last hook event. \
                 Since the check before: {keys} key events, {mouse} mouse events. Keyboard hook: {keyboard_end}. \
                 Mouse hook: {mouse_end}. App in front: {app}. {result} Installs with no line before this one: {}.",
                self.unlogged
            ));
            self.unlogged = 0;
        } else {
            self.unlogged += 1;
        }
        self.failed = (self.failed + 1).min(5);
        self.skip = (1 << self.failed) - 1;
    }
}

#[cfg(target_os = "windows")]
fn key_down(vk: i32) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    // SAFETY: GetAsyncKeyState takes no pointers.
    unsafe { GetAsyncKeyState(vk) as u16 & 0x8000 != 0 }
}

/// Keys and mouse buttons that are down right now. Index is the virtual-key code.
#[cfg(target_os = "windows")]
pub fn input_mask() -> [bool; 256] {
    let mut mask = [false; 256];
    for (vk, slot) in mask.iter_mut().enumerate().skip(1) {
        *slot = key_down(vk as i32);
    }
    mask
}

/// A key or button that was up when `baseline` was taken and is down now.
/// Modifiers are ignored so the shortcut's Ctrl can stay held.
#[cfg(target_os = "windows")]
pub fn input_appeared(baseline: &[bool; 256]) -> Option<u16> {
    let now = input_mask();
    (0..now.len())
        .find(|&vk| now[vk] && !baseline[vk] && !is_modifier_vk(vk as u16))
        .map(|vk| vk as u16)
}

/// A key or mouse button, other than the shortcut, that is down now.
///
/// A keyboard key counts only when the hook saw it go down. Some laptop
/// drivers report a key as down that never goes up, and that key would stop
/// every paste.
#[cfg(target_os = "windows")]
pub fn input_held(binding_vk: u16) -> Option<u16> {
    (1..=255u16).find(|&vk| {
        let real = is_mouse_vk(vk) || PHYSICAL_DOWN[usize::from(vk)].load(Ordering::SeqCst);
        real && !is_modifier_vk(vk) && vk != binding_vk && key_down(i32::from(vk))
    })
}

#[cfg(target_os = "windows")]
fn modifiers_down() -> (bool, bool, bool, bool) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU,
        VK_RSHIFT, VK_RWIN, VK_SHIFT,
    };
    let ctrl = key_down(VK_CONTROL.0 as i32)
        || key_down(VK_LCONTROL.0 as i32)
        || key_down(VK_RCONTROL.0 as i32);
    let alt =
        key_down(VK_MENU.0 as i32) || key_down(VK_LMENU.0 as i32) || key_down(VK_RMENU.0 as i32);
    let shift =
        key_down(VK_SHIFT.0 as i32) || key_down(VK_LSHIFT.0 as i32) || key_down(VK_RSHIFT.0 as i32);
    let win = key_down(VK_LWIN.0 as i32) || key_down(VK_RWIN.0 as i32);
    (ctrl, alt, shift, win)
}

#[cfg(target_os = "windows")]
fn binding_with_modifiers(vk: u16) -> HotkeyBinding {
    let (ctrl, alt, shift, win) = modifiers_down();
    HotkeyBinding {
        ctrl,
        alt,
        shift,
        win,
        vk,
    }
}

#[cfg(target_os = "windows")]
fn modifiers_match(binding: HotkeyBinding) -> bool {
    let (ctrl, alt, shift, win) = modifiers_down();
    ctrl == binding.ctrl && alt == binding.alt && shift == binding.shift && win == binding.win
}

fn is_required_modifier(vk: u16, binding: HotkeyBinding) -> bool {
    (binding.ctrl && matches!(vk, 0x11 | 0xA2 | 0xA3))
        || (binding.alt && matches!(vk, 0x12 | 0xA4 | 0xA5))
        || (binding.shift && matches!(vk, 0x10 | 0xA0 | 0xA1))
        || (binding.win && matches!(vk, 0x5B | 0x5C))
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn hotkey_hook_proc(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, WM_KEYDOWN, WM_KEYUP,
        WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    if code >= 0 {
        note_hook_event(&KEY_EVENTS);
        let kbd = *(lparam.0 as *const KBDLLHOOKSTRUCT);
        let msg = wparam.0 as u32;
        let vk = kbd.vkCode as u16;
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

        // Pass on the keys that TDT sends: auto-paste types text with
        // SendInput, and that text must never start a recording or be
        // mistaken for a new shortcut. Keys that other programs send (a remap
        // tool, a remote desktop) can be the shortcut.
        // With no listener left (a failed start), every key passes on.
        if kbd.dwExtraInfo == crate::paste::OWN_INPUT || SENDER.lock().is_none() {
            return CallNextHookEx(
                HHOOK(HOOK_PTR.load(Ordering::Relaxed)),
                code,
                wparam,
                lparam,
            );
        }
        // Only keys from the keyboard count as typing by the user.
        let physical = kbd.flags & LLKHF_INJECTED != LLKHF_INJECTED;
        if physical && (is_down || is_up) {
            PHYSICAL_DOWN[(kbd.vkCode & 0xFF) as usize].store(is_down, Ordering::SeqCst);
        }

        if CAPTURE.load(Ordering::SeqCst) {
            if capture_expired() {
                end_capture();
                send_action(HotkeyAction::CaptureCancelled);
            } else if is_down && vk == 0x1B {
                end_capture();
                send_action(HotkeyAction::CaptureCancelled);
                return LRESULT(1);
            } else if !is_modifier_vk(vk) && (is_down || is_up) {
                if is_down {
                    finish_capture(binding_with_modifiers(vk));
                }
                return LRESULT(1);
            }
        } else {
            let binding = current_binding();
            if is_down && vk == binding.vk {
                if modifiers_match(binding) {
                    start_press();
                    shortcut_went_down();
                    return LRESULT(1);
                }
                // A repeat of a press that TDT kept, after a modifier came up.
                if shortcut_repeats() {
                    return LRESULT(1);
                }
            }
            if is_down && physical {
                note_physical_key(vk, binding.vk);
            }
            if is_up && vk == binding.vk {
                // The apps did not get the key-down of a press that TDT
                // kept, so they do not get its key-up.
                let kept = SHORTCUT_DOWN.swap(false, Ordering::SeqCst);
                if PRESS_START.lock().is_some() {
                    finish_press();
                    return LRESULT(1);
                }
                if kept {
                    return LRESULT(1);
                }
            } else if is_up && is_required_modifier(vk, binding) {
                // Releasing a required modifier ends the press even if the
                // main key is still held; the later keyup of the main key is
                // then ignored on purpose (no repeat-press handling).
                finish_press();
            }
        }
    }
    CallNextHookEx(
        HHOOK(HOOK_PTR.load(Ordering::Relaxed)),
        code,
        wparam,
        lparam,
    )
}

/// Side buttons whose down event TDT swallowed. Their up event is swallowed
/// too, so an app never gets a lone up and goes Back or Forward.
#[cfg(target_os = "windows")]
static MOUSE_SWALLOWED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Only Mouse 4 and Mouse 5 events are read. All other mouse events pass on
/// at once, because this hook runs for each mouse move on the system.
#[cfg(target_os = "windows")]
unsafe extern "system" fn mouse_hook_proc(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, HHOOK, MSLLHOOKSTRUCT, WM_XBUTTONDOWN, WM_XBUTTONUP,
    };

    if code >= 0 {
        note_hook_event(&MOUSE_EVENTS);
    }
    let msg = wparam.0 as u32;
    if code >= 0 && (msg == WM_XBUTTONDOWN || msg == WM_XBUTTONUP) {
        let info = *(lparam.0 as *const MSLLHOOKSTRUCT);
        let vk = match info.mouseData >> 16 {
            1 => Some(VK_MOUSE4),
            2 => Some(VK_MOUSE5),
            _ => None,
        };
        // A button that another program sends can be the shortcut. TDT sends none.
        if let Some(vk) = vk.filter(|_| SENDER.lock().is_some()) {
            if handle_mouse_button(vk, msg == WM_XBUTTONDOWN) {
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(
        HHOOK(MOUSE_HOOK_PTR.load(Ordering::Relaxed)),
        code,
        wparam,
        lparam,
    )
}

/// Returns true when TDT keeps the event from other apps.
#[cfg(target_os = "windows")]
fn handle_mouse_button(vk: u16, is_down: bool) -> bool {
    let bit = 1u8 << (vk - VK_MOUSE4);
    if !is_down {
        if current_binding().vk == vk {
            finish_press();
        }
        return MOUSE_SWALLOWED.fetch_and(!bit, Ordering::SeqCst) & bit != 0;
    }
    if CAPTURE.load(Ordering::SeqCst) {
        if capture_expired() {
            end_capture();
            send_action(HotkeyAction::CaptureCancelled);
            return false;
        }
        finish_capture(binding_with_modifiers(vk));
    } else {
        let binding = current_binding();
        if vk != binding.vk || !modifiers_match(binding) {
            return false;
        }
        start_press();
    }
    MOUSE_SWALLOWED.fetch_or(bit, Ordering::SeqCst);
    true
}

pub fn is_modifier_vk(vk: u16) -> bool {
    matches!(
        vk,
        0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0 | 0xA1 | 0xA2 | 0xA3 | 0xA4 | 0xA5
    )
}

fn parse_vk(token: &str) -> Option<u16> {
    let lower = token.trim().to_ascii_lowercase().replace(' ', "");
    match lower.as_str() {
        "mouse4" | "xbutton1" => Some(VK_MOUSE4),
        "mouse5" | "xbutton2" => Some(VK_MOUSE5),
        ";" | "semicolon" => Some(0xBA),
        "=" | "plus" | "equal" => Some(0xBB),
        "," | "comma" => Some(0xBC),
        "-" | "minus" => Some(0xBD),
        "." | "period" | "dot" => Some(0xBE),
        "/" | "slash" => Some(0xBF),
        "`" | "grave" => Some(0xC0),
        "[" => Some(0xDB),
        "\\" | "backslash" => Some(0xDC),
        "]" => Some(0xDD),
        "'" | "quote" => Some(0xDE),
        "space" => Some(0x20),
        "enter" | "return" => Some(0x0D),
        "tab" => Some(0x09),
        "backspace" => Some(0x08),
        other => {
            if let Some(num) = other.strip_prefix('f') {
                let n: u16 = num.parse().ok()?;
                if (1..=24).contains(&n) {
                    return Some(0x6F + n);
                }
            }
            if other.len() == 1 {
                let ch = other.chars().next()?.to_ascii_uppercase();
                if ch.is_ascii_alphanumeric() {
                    return Some(ch as u16);
                }
            }
            None
        }
    }
}

fn vk_label(vk: u16) -> String {
    match vk {
        VK_MOUSE4 => "Mouse 4".into(),
        VK_MOUSE5 => "Mouse 5".into(),
        0x08 => "Backspace".into(),
        0x09 => "Tab".into(),
        0x0D => "Enter".into(),
        0x20 => "Space".into(),
        0xBA => ";".into(),
        0xBB => "=".into(),
        0xBC => ",".into(),
        0xBD => "-".into(),
        0xBE => ".".into(),
        0xBF => "/".into(),
        0xC0 => "`".into(),
        0xDB => "[".into(),
        0xDC => "\\".into(),
        0xDD => "]".into(),
        0xDE => "'".into(),
        0x30..=0x39 | 0x41..=0x5A => char::from(vk as u8).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => format!("Vk{vk:02X}"),
    }
}
