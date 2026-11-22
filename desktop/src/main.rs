#![cfg_attr(not(test), windows_subsystem = "windows")]

#[cfg(not(target_os = "windows"))]
compile_error!("TDT desktop is a Windows-only build.");

mod audio;
mod autostart;
mod clock;
mod config;
mod dictation;
mod hotkey;
mod integrity;
mod keys;
mod logfile;
mod paste;
mod priority;
mod target;
mod ui;
mod update;

// The speech engine frees and allocates large buffers on every take; this
// allocator keeps them mapped instead of returning them to Windows.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use audio::{prewarm, watch_default_input, AudioRecorder};
use config::{AppConfig, Settings};
use dictation::{Dictation, Input};
use ely_gpui_component::Assets;
use gpui::*;
use hotkey::{
    poll_capture_timeout, poll_missed_release, set_binding, wake_loop, HotkeyAction, HotkeyBinding,
    HotkeyListener,
};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use tray_icon::menu::MenuEvent;
use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
use ui::preview;
use ui::tray::tooltip_text;
use ui::window_util::{
    ensure_overlay_topmost, find_app_hwnd, follow_current_virtual_desktop, lock_overlay_chrome,
    position_bubble_on_preferred_monitor, PANEL_HEIGHT, PANEL_WIDTH,
};
use ui::{HudView, SystemTray};
use update::{CheckKind, UpdatePhase};
use voice_stt_desktop::log;
use voice_stt_desktop::stt::ErrorKind;
use voice_stt_desktop::stt::{models, EngineSlot, SttEngine};

fn main() {
    lock_working_directory();
    // A windowed program has no console: the lines of the library go to the log.
    log::set_sink(logfile::append);
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("TDT crashed: {info}");
        // The dialog shows where it panicked, not how it got there; the log does.
        log::note(&format!(
            "{msg}\n{}",
            std::backtrace::Backtrace::force_capture()
        ));
        // The engine call catches this panic: the take ends with an error on
        // the pill, and the app stays open.
        if voice_stt_desktop::stt::panic_in_engine_call() {
            return;
        }
        // MessageBox runs a message loop. On the UI thread that loop would run
        // GPUI tasks while the panicked update still holds the app, and a
        // second panic there aborts before the user sees the dialog. So the
        // dialog gets its own thread.
        if let Ok(dialog) = std::thread::Builder::new().spawn(move || show_error(&msg)) {
            let _ = dialog.join();
        }
        // A crashed process that stays open holds the single-instance claim,
        // so the next start would do nothing.
        exit_now(1);
    }));

    // The preview captures (`ui::preview`) run with none of the live
    // services: no instance claim, no settings file, no microphone, no
    // hooks, no update checks.
    let is_preview = preview::is_active();
    let (_instance, settings) = if is_preview {
        (None, Settings::new(AppConfig::default(), false))
    } else {
        let guard = match claim_instance() {
            InstanceClaim::AlreadyRunning => return,
            InstanceClaim::Owned(guard) => guard,
            InstanceClaim::Failed(error) => {
                log::note(&error);
                show_error(&error);
                return;
            }
        };
        (Some(guard), Settings::new(AppConfig::load(), true))
    };
    // A damaged settings file: say so on the pill, and write the defaults so
    // the next load is clean. `load` kept a copy of the old file.
    let config_error = config::take_reset_notice();
    if config_error.is_some() {
        if let Err(error) = settings.save() {
            log::note(&format!("Could not save the default settings: {error}"));
        }
    }
    // A retired model id moves to the model that replaces it, also in the file.
    let model_id = settings.get(|config| config.model_id.clone());
    let model_spec = models::resolve(&model_id);
    if model_spec.id != model_id {
        if let Err(error) = settings.update(|config| config.model_id = model_spec.id.to_string()) {
            log::note(&format!("Could not save the model id: {error}"));
        }
    }
    let hotkey_text = settings.get(|config| config.hotkey.clone());
    let hotkey_binding = HotkeyBinding::parse(&hotkey_text).unwrap_or_else(|| {
        log::note(&format!(
            "Could not read the shortcut '{hotkey_text}' of the settings. TDT uses the default."
        ));
        let binding = HotkeyBinding::default();
        if let Err(error) = settings.update(|config| config.hotkey = binding.display()) {
            log::note(&format!("Could not save the default shortcut: {error}"));
        }
        binding
    });
    set_binding(hotkey_binding);
    let hotkey_label = hotkey_binding.display();

    let model_dir = settings.get(|config| config.model_dir.clone());
    let stt_engine = EngineSlot::default();
    stt_engine.set(
        if let Some(dir) = models::find_dir(model_spec, model_dir.as_deref()) {
            log::note(&format!(
                "{} speech model found at: {}",
                model_spec.label,
                dir.display()
            ));
            Some(Arc::new(SttEngine::new(&dir, model_spec)))
        } else {
            log::note(&format!(
                "{} is not downloaded. Open Settings to download it.",
                model_spec.label
            ));
            None
        },
    );

    let update_state = Arc::new(Mutex::new(UpdatePhase::Idle));
    let live = if is_preview {
        Live::default()
    } else {
        Live::start(&stt_engine, &settings, &update_state)
    };
    let Live {
        recorder,
        microphone_error,
        hotkey: _hotkey_guard,
        hotkey_rx,
        hotkey_error,
    } = live;

    // Ely's components read fonts and colors registered by init.
    let app = gpui_platform::application().with_assets(Assets);
    let mut dictation = Some(Dictation::new(
        recorder,
        stt_engine.clone(),
        settings.clone(),
        is_preview,
    ));
    let stt_for_view = stt_engine.clone();

    app.run(move |cx: &mut App| {
        ely_gpui_component::init(cx);
        ui::theme::install(cx);
        // Windows requires the event loop to exist before creating the tray icon.
        let mut tray = if is_preview && !preview::wants_tray() {
            None
        } else {
            match SystemTray::new(settings.get(|config| config.auto_paste), &hotkey_label) {
                Ok(tray) => Some(tray),
                Err(error) => {
                    log::note(&format!("Failed to create system tray: {error}"));
                    None
                }
            }
        };

        // The idle tick of the UI loop is slow, so a tray click wakes it.
        let (menu_tx, menu_rx) = crossbeam_channel::unbounded::<MenuEvent>();
        MenuEvent::set_event_handler(Some(move |event| {
            let _ = menu_tx.send(event);
            wake_loop();
        }));
        let (tray_tx, tray_rx) = crossbeam_channel::unbounded::<TrayIconEvent>();
        TrayIconEvent::set_event_handler(Some(move |event| {
            // The pointer over the icon gives events that the loop does not use.
            if matches!(
                event,
                TrayIconEvent::Click { .. } | TrayIconEvent::DoubleClick { .. }
            ) {
                let _ = tray_tx.send(event);
                wake_loop();
            }
        }));

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::new(px(700.0), px(860.0)),
                size: Size::new(px(PANEL_WIDTH as f32), px(PANEL_HEIGHT as f32)),
            })),
            titlebar: None,
            focus: false,
            // GPUI only requests frames for windows it owns as visible. Showing
            // a hidden HWND later through Win32 leaves the surface transparent.
            show: true,
            // GPUI caps a window without focus at 30 fps. The pill never
            // takes focus, so the cap would hold every animation at 30 fps.
            // It only draws while it moves, so it can match the display.
            inactive_frame_interval: None,
            is_resizable: false,
            kind: WindowKind::PopUp,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };

        let settings_loop = settings.clone();
        let update_for_view = Arc::clone(&update_state);
        let update_for_loop = Arc::clone(&update_state);

        // The panel is open from the start for `VOICE_STT_OPEN_PANEL` and the
        // panel previews.
        let open_panel =
            std::env::var_os("VOICE_STT_OPEN_PANEL").is_some() || preview::wants_panel();
        // GPUI creates the window late on a cold start, so its Win32 styles
        // are set from a thread that waits for it.
        std::thread::spawn(move || {
            let mut overlay = None;
            for _ in 0..60 {
                overlay = find_app_hwnd();
                if overlay.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            // The overlay window can appear late on cold starts; keep looking
            // at a slower cadence instead of skipping styling for the session.
            let mut attempts = 0;
            while overlay.is_none() && attempts < 20 {
                std::thread::sleep(Duration::from_millis(500));
                overlay = find_app_hwnd();
                attempts += 1;
            }
            if let Some(hwnd) = overlay {
                ui::window_util::apply_overlay_window_style(hwnd);
                ui::window_util::keep_pill_from_activating(hwnd);
                ui::window_util::show_overlay_window(hwnd);

                position_bubble_on_preferred_monitor(hwnd);
                // Strips caption and resize handles and snaps the pill back to size.
                lock_overlay_chrome();
                if open_panel {
                    ui::window_util::set_window_mode(true);
                }
            }
            // The pill follows the user to another virtual desktop. The check
            // is a COM call into Explorer, so it runs here: a slow Explorer
            // must not hold the UI loop.
            loop {
                follow_current_virtual_desktop();
                std::thread::sleep(Duration::from_millis(400));
            }
        });

        match cx.open_window(window_options, move |window, cx| {
            cx.new(|cx: &mut Context<HudView>| {
                // The panel is a flyout: it closes when the user switches to
                // another app, so it never stays on top of a game or a window.
                cx.observe_window_activation(window, |view, window, cx| {
                    if !window.is_window_active() {
                        view.close_stats_settings(cx);
                    }
                })
                .detach();
                // Alt+F4 or another close message closes the panel. TDT lives
                // in the tray, so only Quit in the tray menu ends it.
                let hud = cx.weak_entity();
                window.on_window_should_close(cx, move |_, cx| {
                    let _ = hud.update(cx, |view, cx| view.close_stats_settings(cx));
                    false
                });
                let settings = settings_loop.clone();
                let update_phase = Arc::clone(&update_for_loop);
                let mut dictation = dictation.take().expect("GPUI opens the window once");

                cx.spawn(async move |this, cx| {
                    // The view has nothing that changes with time.
                    let mut at_rest = false;
                    loop {
                        let poll_interval = if dictation.is_busy() {
                            Duration::from_millis(25)
                        } else if at_rest && hotkey_rx.is_some() {
                            // Nothing moves, and the shortcut, the pill, and
                            // the tray wake the loop.
                            Duration::from_millis(250)
                        } else {
                            Duration::from_millis(75)
                        };
                        // A shortcut press or release ends the wait at once:
                        // the poll would add its time before the microphone
                        // starts, and up to 25 ms before the release is seen.
                        let woke = match &hotkey_rx {
                            Some(rx) => rx
                                .recv()
                                .with_timeout(poll_interval, cx.background_executor())
                                .await
                                .ok()
                                .and_then(Result::ok),
                            None => {
                                cx.background_executor().timer(poll_interval).await;
                                None
                            }
                        };
                        lock_overlay_chrome();
                        ensure_overlay_topmost();
                        poll_capture_timeout();
                        poll_missed_release();
                        if take_show_request() {
                            let _ = this.update(cx, |view, cx| {
                                view.open_settings(cx);
                            });
                        }
                        let retry = this
                            .update(cx, |view, cx| {
                                view.take_ready_model();
                                if view.model_load_moved() {
                                    cx.notify();
                                }
                                view.take_retry_microphone()
                            })
                            .unwrap_or(false);
                        if retry {
                            // Opening the microphone makes COM calls, and COM on
                            // this thread can run a message loop. Inside an
                            // update that loop would run GPUI tasks and panic
                            // ("RefCell already borrowed"), so open it between
                            // updates.
                            let changes = dictation.retry_microphone();
                            let _ = this.update(cx, |view, cx| {
                                view.apply(changes, cx);
                                cx.notify();
                            });
                        }

                        // Every queued event, so quick tray clicks are never dropped.
                        let mut quitting = false;
                        while let Ok(event) = menu_rx.try_recv() {
                            if let Some(ref tray) = tray {
                                if event.id == tray.quit_item.id() {
                                    let _ = this.update(cx, |_, cx| {
                                        cx.quit();
                                    });
                                    quitting = true;
                                } else if event.id == tray.auto_paste_item.id() {
                                    // The same switch as in Settings: it changes
                                    // only when the setting is saved.
                                    let _ = this.update(cx, |view, cx| {
                                        view.toggle_auto_paste();
                                        cx.notify();
                                    });
                                } else if event.id == tray.settings_item.id() {
                                    let _ = this.update(cx, |view, cx| {
                                        view.open_settings(cx);
                                    });
                                } else if event.id == tray.stop_item.id() {
                                    let _ = this.update(cx, |view, _| view.request_stop());
                                } else if event.id == tray.show_item.id() {
                                    let _ = this.update(cx, |view, cx| {
                                        view.open_settings(cx);
                                    });
                                } else if event.id == tray.updates_item.id() {
                                    let _ = this.update(cx, |view, cx| {
                                        view.check_for_update(CheckKind::Manual);
                                        view.open_settings(cx);
                                    });
                                }
                            }
                        }
                        // Setup replaces TDT.exe, so TDT quits like the Quit
                        // item and removes its tray icon first.
                        if !quitting && *update_phase.lock() == UpdatePhase::Installing {
                            let _ = this.update(cx, |_, cx| cx.quit());
                            quitting = true;
                        }
                        while let Ok(event) = tray_rx.try_recv() {
                            match event {
                                TrayIconEvent::Click {
                                    button: MouseButton::Left,
                                    button_state: MouseButtonState::Up,
                                    ..
                                }
                                | TrayIconEvent::DoubleClick {
                                    button: MouseButton::Left,
                                    ..
                                } => {
                                    let _ = this.update(cx, |view, cx| {
                                        view.open_settings(cx);
                                    });
                                }
                                _ => {}
                            }
                        }
                        if quitting {
                            dictation.quit();
                            break;
                        }

                        // The settings panel can change auto-paste too, so keep the
                        // tray checkmark in sync with the single source of truth.
                        if let Some(ref tray) = tray {
                            let current = settings.get(|config| config.auto_paste);
                            if tray.auto_paste_item.is_checked() != current {
                                tray.auto_paste_item.set_checked(current);
                            }
                        }

                        let actions = woke
                            .into_iter()
                            .chain(std::iter::from_fn(|| hotkey_rx.as_ref()?.try_recv().ok()));
                        let mut input = Vec::new();
                        for action in actions {
                            match action {
                                HotkeyAction::Captured(binding) => {
                                    let label = binding.display();
                                    set_binding(binding);
                                    let saved =
                                        settings.update(|config| config.hotkey = label.clone());
                                    if let Some(ref tray) = tray {
                                        let _ =
                                            tray.tray_icon.set_tooltip(Some(tooltip_text(&label)));
                                    }
                                    let _ = this.update(cx, |view, cx| {
                                        view.shortcut_captured(binding, saved);
                                        cx.notify();
                                    });
                                }
                                HotkeyAction::CaptureCancelled => {
                                    let _ = this.update(cx, |view, cx| {
                                        view.shortcut_capture_cancelled();
                                        cx.notify();
                                    });
                                }
                                HotkeyAction::PressStarted(generation) => {
                                    input.push(Input::Press(generation))
                                }
                                HotkeyAction::HoldReleased(asked) => {
                                    input.push(Input::HoldReleased(asked))
                                }
                                HotkeyAction::Toggled(asked) => input.push(Input::Toggled(asked)),
                                HotkeyAction::Wake => {}
                            }
                        }
                        // A click on the resting pill is a tap of the shortcut.
                        let clicked = this
                            .update(cx, |view, _| view.take_click())
                            .unwrap_or(false);
                        let changes = dictation.step(input, clicked, || {
                            this.update(cx, |view, _| view.take_stop_request())
                                .unwrap_or(false)
                        });
                        if !changes.is_empty() {
                            let _ = this.update(cx, |view, cx| {
                                view.apply(changes, cx);
                                cx.notify();
                            });
                        }
                        let view_state = this.update(cx, |view, cx| {
                            let rest = view.tick(cx);
                            let tray_state = tray
                                .is_some()
                                .then(|| (view.status().clone(), view.hotkey().display()));
                            (rest, tray_state)
                        });
                        let (rest, tray_state) = view_state.unwrap_or((false, None));
                        at_rest = rest;
                        // A tooltip change waits on Explorer, and during that wait
                        // Windows delivers tray clicks to the tray window. A right
                        // click opens the tray menu, whose message loop runs GPUI
                        // tasks. Inside an update they would panic ("RefCell
                        // already borrowed"), so update the tray between updates.
                        if let (Some(tray), Some((status, hotkey))) = (tray.as_mut(), tray_state) {
                            tray.update(&status, &hotkey);
                        }
                    }
                })
                .detach();

                let mut view = HudView::new(
                    hotkey_binding,
                    stt_for_view,
                    settings_loop.clone(),
                    update_for_view,
                    open_panel,
                    is_preview,
                    cx,
                );
                let startup_errors = [
                    (config_error, ErrorKind::Other),
                    (microphone_error, ErrorKind::Microphone),
                    (hotkey_error, ErrorKind::Other),
                ];
                for (error, kind) in startup_errors {
                    if let Some(error) = error {
                        view.show_startup_error(error, kind);
                    }
                }
                view
            })
        }) {
            Ok(_) => log::note("TDT window opened successfully."),
            Err(e) => {
                let msg = format!("Failed to open TDT window: {e:?}");
                log::note(&msg);
                show_error(&msg);
            }
        }
    });
    drop(_hotkey_guard);
    drop(_instance);
    exit_now(0);
}

/// The services of a real run. The preview captures use `Live::default()`.
#[derive(Default)]
struct Live {
    recorder: Option<AudioRecorder>,
    microphone_error: Option<String>,
    hotkey: Option<HotkeyListener>,
    hotkey_rx: Option<async_channel::Receiver<HotkeyAction>>,
    hotkey_error: Option<String>,
}

impl Live {
    fn start(
        stt_engine: &EngineSlot,
        settings: &Settings,
        update_state: &Arc<Mutex<UpdatePhase>>,
    ) -> Self {
        // Task Scheduler calls are slow enough to delay the first window.
        std::thread::spawn(autostart::refresh_if_enabled);
        update::remove_downloaded_setups();

        // Open the microphone before the window. Doing it on the GPUI executor
        // blocks the first frame, and the popup then stays without WS_VISIBLE.
        std::thread::spawn(prewarm);
        watch_default_input();
        let (recorder, microphone_error) = match AudioRecorder::new() {
            Ok(recorder) => {
                log::note(&format!("Microphone: {}", recorder.describe()));
                (Some(recorder), None)
            }
            Err(error) => (
                None,
                Some(format!(
                    "Could not start the microphone. Connect a microphone and choose Retry microphone. {error}"
                )),
            ),
        };

        // Load and warm the selected model while the app opens, so a short first
        // dictation does not pay the full initialization cost after release.
        // prepare() serializes with recording and retries on failure.
        if let Some(engine) = stt_engine.get() {
            std::thread::spawn(move || {
                if let Err(error) = engine.prepare() {
                    log::note(&format!("Could not prepare the speech model: {error}"));
                }
            });
        }

        // A failed install must not look like a working shortcut.
        let (hotkey, hotkey_rx, hotkey_error) = match HotkeyListener::start() {
            Ok((guard, rx)) => {
                // The hook installs fine at low integrity but never sees keys
                // typed into normal apps, so say why instead of failing silently.
                let error = low_rights_message();
                if let Some(message) = &error {
                    log::note(message);
                }
                (Some(guard), Some(rx), error)
            }
            Err(error) => {
                log::note(&format!("Hotkey hook failed: {error}"));
                (None, None, Some(error))
            }
        };

        let settings = settings.clone();
        update::start_automatic_checks(Arc::clone(update_state), move || {
            settings.get(|config| config.check_updates)
        });

        Self {
            recorder,
            microphone_error,
            hotkey,
            hotkey_rx,
            hotkey_error,
        }
    }
}

/// Ends the process without DLL and thread-local teardown. Teardown runs
/// after Windows stops every other thread, so a step that waits on one, as a
/// COM release into Explorer did, keeps a windowless TDT alive forever.
fn exit_now(code: u32) -> ! {
    // SAFETY: TerminateProcess on this process's own pseudo-handle.
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        let _ = TerminateProcess(GetCurrentProcess(), code);
    }
    std::process::exit(code as i32)
}

/// The single-instance mutex. Setup (`packaging/tdt-setup`, `INSTANCE_MUTEX`)
/// waits on this name for TDT to quit: change both together.
const INSTANCE_MUTEX: windows::core::PCWSTR = windows::core::w!("Local\\TDT-TalkDontType");
/// A second start sets this event, and the running TDT opens its panel.
const SHOW_EVENT: windows::core::PCWSTR = windows::core::w!("Local\\TDT-ShowOverlay");

enum InstanceClaim {
    Owned(InstanceGuard),
    AlreadyRunning,
    Failed(String),
}

struct InstanceGuard {
    mutex: windows::Win32::Foundation::HANDLE,
    show_event: windows::Win32::Foundation::HANDLE,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: both handles come from `claim_instance` and are closed once, here. The slot is cleared first,
        // so `take_show_request` cannot use the event after it closes.
        unsafe {
            *show_event_slot().lock() = None;
            let _ = windows::Win32::Foundation::CloseHandle(self.show_event);
            let _ = windows::Win32::Foundation::CloseHandle(self.mutex);
        }
    }
}

fn show_event_slot() -> &'static Mutex<Option<isize>> {
    static SLOT: Mutex<Option<isize>> = Mutex::new(None);
    &SLOT
}

fn take_show_request() -> bool {
    use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::WaitForSingleObject;
    // The lock stays held, so `InstanceGuard::drop` cannot close the event mid-call.
    let slot = show_event_slot().lock();
    let Some(raw) = *slot else {
        return false;
    };
    // SAFETY: the slot holds the event until `InstanceGuard::drop` clears it under this
    // lock, so the handle is open. A zero timeout only reads the event state.
    unsafe { WaitForSingleObject(HANDLE(raw as _), 0) == WAIT_OBJECT_0 }
}

fn signal_running_instance() {
    use windows::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};
    // SAFETY: the name is a static null-terminated string, and the opened handle is closed after use.
    unsafe {
        if let Ok(handle) = OpenEventW(EVENT_MODIFY_STATE, false, SHOW_EVENT) {
            let _ = SetEvent(handle);
            let _ = windows::Win32::Foundation::CloseHandle(handle);
        }
    }
}

/// Windows starts an exe at low integrity when the file carries a low-rights
/// label (files saved into a folder with that label inherit it). A low process
/// cannot read keys from, or paste into, normal apps.
fn low_rights_message() -> Option<String> {
    if integrity::own()? >= integrity::MEDIUM {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    Some(format!(
        "Windows started TDT with low rights, so the shortcut and auto-paste cannot reach \
         other apps. The TDT.exe file has a low-rights label. Run this in PowerShell, then \
         start TDT again: icacls \"{}\" /setintegritylevel M",
        exe.display()
    ))
}

fn claim_instance() -> InstanceClaim {
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::{CreateEventW, CreateMutexW};

    // SAFETY: Win32 object calls with static names. Each handle is closed on every failure path or owned
    // by the returned guard.
    unsafe {
        // CreateMutexW's wrapper does not surface ERROR_ALREADY_EXISTS.
        // Open the existing object first so a second launch cannot start
        // another speech worker.
        if let Ok(existing) = windows::Win32::System::Threading::OpenMutexW(
            windows::Win32::System::Threading::SYNCHRONIZATION_ACCESS_RIGHTS(0x00100000),
            false,
            INSTANCE_MUTEX,
        ) {
            let _ = windows::Win32::Foundation::CloseHandle(existing);
            signal_running_instance();
            return InstanceClaim::AlreadyRunning;
        }
        match CreateMutexW(None, true, INSTANCE_MUTEX) {
            Ok(mutex) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    let _ = windows::Win32::Foundation::CloseHandle(mutex);
                    signal_running_instance();
                    InstanceClaim::AlreadyRunning
                } else {
                    match CreateEventW(None, false, false, SHOW_EVENT) {
                        Ok(show_event) => {
                            *show_event_slot().lock() = Some(show_event.0 as isize);
                            InstanceClaim::Owned(InstanceGuard { mutex, show_event })
                        }
                        Err(error) => {
                            let _ = windows::Win32::Foundation::CloseHandle(mutex);
                            InstanceClaim::Failed(format!("Could not start TDT: {error}"))
                        }
                    }
                }
            }
            Err(error) => InstanceClaim::Failed(format!("Could not start TDT: {error}")),
        }
    }
}

fn lock_working_directory() {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let _ = std::env::set_current_dir(dir);
        }
    }
}

fn show_error(message: &str) {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    // SAFETY: MessageBoxW gets owned strings that outlive the call.
    unsafe {
        let _ = MessageBoxW(
            None,
            &HSTRING::from(message),
            &HSTRING::from("TDT"),
            MB_OK | MB_ICONERROR,
        );
    }
}
