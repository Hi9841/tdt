#![cfg_attr(not(test), windows_subsystem = "windows")]

#[cfg(not(target_os = "windows"))]
compile_error!(
    "TDT desktop is a Windows-only build. Build the Android app from the `mobile` folder instead."
);

mod audio;
mod config;
mod hotkey;
mod paste;
mod stt;
mod ui;
mod update;

use audio::{play_sound, AudioRecorder, SoundEffect, VIS_BARS};
use config::{AppConfig, AppStats};
use gpui::*;
use hotkey::{
    poll_capture_timeout, set_binding, tap_should_stop, HotkeyAction, HotkeyBinding, HotkeyListener,
};
use parking_lot::Mutex;
use paste::PasteInjector;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use stt::SttEngine;
use tray_icon::menu::MenuEvent;
use ui::window_util::{
    find_app_hwnd, follow_current_virtual_desktop, lock_overlay_chrome,
    position_bubble_on_preferred_monitor, BUBBLE_HEIGHT, BUBBLE_WIDTH,
};
use ui::{HudStatus, HudView, SystemTray};
use update::UpdatePhase;

enum InternalEvent {
    TranscribeSuccess {
        text: String,
        auto_pasted: bool,
        _duration_secs: f32,
        latency_ms: u64,
    },
    TranscribeError(String),
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("TDT crashed: {info}");
        append_log(&msg);
        show_error(&msg);
    }));

    let _instance = match claim_instance() {
        InstanceClaim::AlreadyRunning => return,
        InstanceClaim::Owned(guard) => guard,
        InstanceClaim::Failed(error) => {
            append_log(&error);
            show_error(&error);
            return;
        }
    };

    let config = AppConfig::load();
    let auto_paste = config.auto_paste;
    let hotkey_binding = HotkeyBinding::parse(&config.hotkey).unwrap_or_default();
    set_binding(hotkey_binding);
    let hotkey_label = hotkey_binding.display();

    // 1. Initialize audio recorder
    let recorder = match AudioRecorder::new() {
        Ok(r) => Rc::new(r),
        Err(e) => {
            let msg = format!("Could not start the microphone: {e}");
            append_log(&msg);
            show_error(&msg);
            return;
        }
    };

    // 2. Initialize STT Engine
    let stt_engine = match config.find_model_dir() {
        Some(dir) => match SttEngine::new(&dir, &config.language) {
            Ok(engine) => {
                append_log(&format!(
                    "Sherpa-ONNX SenseVoice model found at: {}",
                    dir.display()
                ));
                Some(Arc::new(engine))
            }
            Err(e) => {
                append_log(&format!("Failed to initialize SenseVoice STT Engine: {e}"));
                None
            }
        },
        None => {
            append_log("SenseVoice model not found. Run models/download-models.ps1 to download.");
            None
        }
    };

    // 3. Initialize paste injector
    let injector = Arc::new(PasteInjector::new());

    // 4. Initialize hotkey hook
    let (_hotkey_guard, hotkey_rx) = HotkeyListener::start();

    // 5. Launch GPUI application with floating bubble window
    let app = Application::new();
    let recorder_clone = Rc::clone(&recorder);
    let injector_clone = Arc::clone(&injector);
    let stt_clone = stt_engine.clone();
    let initial_language = config.language.clone();
    let stt_for_view = stt_engine.clone();

    app.run(move |cx: &mut App| {
        // Windows requires the event loop to exist before creating the tray icon.
        let tray = match SystemTray::new(auto_paste, &hotkey_label) {
            Ok(tray) => Some(tray),
            Err(error) => {
                append_log(&format!("Failed to create system tray: {error}"));
                None
            }
        };

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::new(px(700.0), px(860.0)),
                size: Size::new(px(BUBBLE_WIDTH), px(BUBBLE_HEIGHT)),
            })),
            titlebar: None,
            focus: false,
            show: true,
            is_resizable: false,
            kind: WindowKind::PopUp,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };

        let auto_paste_state = Arc::new(Mutex::new(auto_paste));
        let auto_paste_bg = Arc::clone(&auto_paste_state);
        let auto_paste_view = Arc::clone(&auto_paste_state);
        let update_state = Arc::new(Mutex::new(UpdatePhase::Idle));
        let update_for_view = Arc::clone(&update_state);
        let update_for_boot = Arc::clone(&update_state);
        let (update_ping_tx, update_ping_rx) = crossbeam_channel::unbounded::<()>();
        let update_ping_view = update_ping_tx.clone();
        let update_ping_boot = update_ping_tx.clone();

        // Store foreground window HWND so auto-paste always restores focus to user target app
        let target_hwnd_state = Arc::new(Mutex::new(None::<isize>));
        let target_hwnd_loop = Arc::clone(&target_hwnd_state);

        let (internal_tx, internal_rx) = crossbeam_channel::unbounded::<InternalEvent>();

        // Background thread to apply Win32 WS_EX_TOPMOST & WS_EX_NOACTIVATE & WS_EX_TOOLWINDOW
        #[cfg(target_os = "windows")]
        std::thread::spawn(move || {
            use windows::Win32::UI::WindowsAndMessaging::{
                GetWindowLongW, SetWindowLongW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
                WS_EX_TOPMOST,
            };

            std::thread::sleep(Duration::from_millis(300));

            let Some(hwnd) = find_app_hwnd() else {
                return;
            };

            unsafe {
                let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
                let _ = SetWindowLongW(
                    hwnd,
                    GWL_EXSTYLE,
                    ex_style | (WS_EX_TOPMOST.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as i32,
                );
            }

            position_bubble_on_preferred_monitor(hwnd);
            follow_current_virtual_desktop();
            // Strips caption and resize handles and snaps the pill back to size.
            lock_overlay_chrome();
        });

        match cx.open_window(window_options, move |_window, cx| {
            cx.new(|cx: &mut Context<HudView>| {
                let rec = Rc::clone(&recorder_clone);
                let inj = Arc::clone(&injector_clone);
                let stt = stt_clone.clone();
                let auto_paste_flag = Arc::clone(&auto_paste_bg);
                let target_hwnd = Arc::clone(&target_hwnd_loop);

                // Spawn UI coordination loop running on GPUI executor
                cx.spawn(async move |this, cx| {
                    let mut is_recording_state = false;
                    let mut is_processing_state = false;
                    let mut recording_before_press = false;

                    loop {
                        let poll_interval = if is_recording_state || is_processing_state {
                            Duration::from_millis(25)
                        } else {
                            Duration::from_millis(75)
                        };
                        cx.background_executor()
                            .timer(poll_interval)
                            .await;
                        follow_current_virtual_desktop();
                        lock_overlay_chrome();
                        poll_capture_timeout();
                        while update_ping_rx.try_recv().is_ok() {
                            let _ = this.update(cx, |_, cx| cx.notify());
                        }

                        // 1. Process Menu Events from tray. Drain every queued
                        // event so quick tray clicks are never dropped.
                        let mut quitting = false;
                        while let Ok(event) = MenuEvent::receiver().try_recv() {
                            if let Some(ref tray) = tray {
                                if event.id == tray.quit_item.id() {
                                    let _ = this.update(cx, |_, cx| {
                                        cx.quit();
                                    });
                                    quitting = true;
                                } else if event.id == tray.auto_paste_item.id() {
                                    let mut flag = auto_paste_flag.lock();
                                    *flag = !*flag;
                                    let current = *flag;
                                    let mut cfg = AppConfig::load();
                                    cfg.auto_paste = current;
                                    let _ = cfg.save();
                                    let _ = this.update(cx, |view, cx| {
                                        view.auto_paste_enabled = current;
                                        cx.notify();
                                    });
                                } else if event.id == tray.settings_item.id() {
                                    let _ = this.update(cx, |view, cx| {
                                        view.open_settings(cx);
                                    });
                                } else if event.id == tray.updates_item.id() {
                                    let _ = this.update(cx, |view, cx| {
                                        view.start_update_check();
                                        view.open_settings(cx);
                                    });
                                }
                            }
                        }
                        if quitting {
                            break;
                        }

                        // The settings panel can change auto-paste too, so keep the
                        // tray checkmark in sync with the single source of truth.
                        if let Some(ref tray) = tray {
                            let current = *auto_paste_flag.lock();
                            if tray.auto_paste_item.is_checked() != current {
                                tray.auto_paste_item.set_checked(current);
                            }
                        }

                        // Helper closure to capture user's active editor window before recording
                        let capture_fg = || {
                            #[cfg(target_os = "windows")]
                            {
                                use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
                                let fg = unsafe { GetForegroundWindow() };
                                if fg.0 != 0 as _ {
                                    return Some(fg.0 as isize);
                                }
                            }
                            None
                        };

                        // Helper closure to run transcription in background thread
                        let dispatch_transcribe = |samples: Vec<f32>,
                                                   stt_worker: Option<Arc<SttEngine>>,
                                                   inj_worker: Arc<PasteInjector>,
                                                   tx: crossbeam_channel::Sender<InternalEvent>,
                                                   should_paste: bool,
                                                   target: Option<isize>| {
                            std::thread::spawn(move || {
                                if let Some(ref engine) = stt_worker {
                                    let start_t = Instant::now();
                                    let duration_secs = samples.len() as f32 / 16000.0;
                                    match engine.transcribe(&samples) {
                                        Ok(text) => {
                                            let latency_ms = start_t.elapsed().as_millis() as u64;
                                            if !text.is_empty() {
                                                let mut stats = AppStats::load();
                                                stats.record_and_save(&text, duration_secs, latency_ms);

                                                let paste_result = if should_paste {
                                                    inj_worker.auto_paste(&text, target)
                                                } else {
                                                    inj_worker.copy_to_clipboard(&text)
                                                };

                                                if let Err(error) = paste_result {
                                                    let _ = tx.send(InternalEvent::TranscribeError(
                                                        format!("Transcribed and copied, but could not insert text: {error}"),
                                                    ));
                                                    return;
                                                }
                                            }
                                            let _ = tx.send(InternalEvent::TranscribeSuccess {
                                                text,
                                                auto_pasted: should_paste,
                                                _duration_secs: duration_secs,
                                                latency_ms,
                                            });
                                        }
                                        Err(e) => {
                                            let _ = tx.send(InternalEvent::TranscribeError(e));
                                        }
                                    }
                                } else {
                                    let _ = tx.send(InternalEvent::TranscribeError(
                                        "SenseVoice model not loaded".to_string(),
                                    ));
                                }
                            });
                        };

                        // 2. Process Hotkey Events
                        while let Ok(action) = hotkey_rx.try_recv() {
                            match action {
                                HotkeyAction::Captured(binding) => {
                                    let label = binding.display();
                                    set_binding(binding);
                                    let mut cfg = AppConfig::load();
                                    cfg.hotkey = label.clone();
                                    let _ = cfg.save();
                                    if let Some(ref tray) = tray {
                                        tray.shortcut_item
                                            .set_text(format!("Shortcut: {label}"));
                                        let _ = tray.tray_icon.set_tooltip(Some(format!(
                                            "TDT - Talk Don't Type. Tap or hold {label} to talk."
                                        )));
                                    }
                                    let _ = this.update(cx, |view, cx| {
                                        view.hotkey_label = label;
                                        view.hotkey_capturing = false;
                                        cx.notify();
                                    });
                                }
                                HotkeyAction::CaptureCancelled => {
                                    let _ = this.update(cx, |view, cx| {
                                        view.hotkey_capturing = false;
                                        cx.notify();
                                    });
                                }
                                HotkeyAction::PressStarted => {
                                    recording_before_press = is_recording_state;
                                    // Do not overlap recordings with an in-flight
                                    // transcription. The current recorder and STT
                                    // engine are single-session by design.
                                    if !is_recording_state && !is_processing_state {
                                        *target_hwnd.lock() = capture_fg();
                                        rec.start();
                                        if let Some(engine) = stt.clone() {
                                            std::thread::spawn(move || {
                                                if let Err(error) = engine.prepare() {
                                                    eprintln!("Failed to prepare STT model: {error}");
                                                }
                                            });
                                        }
                                        play_sound(SoundEffect::StartListening);
                                        is_recording_state = true;
                                        let started_at = Instant::now();
                                        let _ = this.update(cx, |view, cx| {
                                            view.status = HudStatus::Listening {
                                                audio_level: 0.0,
                                                started_at,
                                            };
                                            cx.notify();
                                        });
                                    }
                                }
                                HotkeyAction::HoldReleased | HotkeyAction::Toggled => {
                                    // A hold always stops; a tap only stops a session it
                                    // started, otherwise it has just started one.
                                    let should_stop = if matches!(action, HotkeyAction::Toggled)
                                    {
                                        let stop = tap_should_stop(recording_before_press);
                                        recording_before_press = false;
                                        stop
                                    } else {
                                        true
                                    };
                                    if should_stop && is_recording_state {
                                        is_recording_state = false;
                                        is_processing_state = true;
                                        let samples = rec.stop();
                                        play_sound(SoundEffect::StopListening);
                                        let _ = this.update(cx, |view, cx| {
                                            // Recording is over; drop the live
                                            // waveform here rather than in render.
                                            view.wave_peaks = [0.0; VIS_BARS];
                                            let recorded_for = match &view.status {
                                                HudStatus::Listening { started_at, .. } => {
                                                    started_at.elapsed()
                                                }
                                                _ => Duration::from_secs(0),
                                            };
                                            view.status = HudStatus::Transcribing { recorded_for };
                                            cx.notify();
                                        });

                                        dispatch_transcribe(
                                            samples,
                                            stt.clone(),
                                            Arc::clone(&inj),
                                            internal_tx.clone(),
                                            *auto_paste_flag.lock(),
                                            *target_hwnd.lock(),
                                        );
                                    }
                                }
                            }
                        }

                        // 3. Process background transcription results
                        while let Ok(event) = internal_rx.try_recv() {
                            is_processing_state = false;
                            match event {
                                InternalEvent::TranscribeSuccess { text, auto_pasted, latency_ms, .. } => {
                                    play_sound(SoundEffect::Success);
                                    let word_count = text.split_whitespace().count();
                                    println!("Transcription completed ({} words, latency: {}ms)", word_count, latency_ms);
                                    let finished_at = Instant::now();
                                    let _ = this.update(cx, |view, cx| {
                                        view.stats = AppStats::load();
                                        view.status = HudStatus::Success {
                                            text,
                                            auto_pasted,
                                            finished_at,
                                        };
                                        cx.notify();
                                    });
                                }
                                InternalEvent::TranscribeError(err) => {
                                    play_sound(SoundEffect::Error);
                                    let occurred_at = Instant::now();
                                    let _ = this.update(cx, |view, cx| {
                                        view.status = HudStatus::Error {
                                            message: err,
                                            occurred_at,
                                        };
                                        cx.notify();
                                    });
                                }
                            }
                        }

                        // 4. Animation frame & audio level updates
                        let current_level = if is_recording_state { rec.audio_level() } else { 0.0 };
                        let vis_peaks = rec.vis_peaks();

                        let _ = this.update(cx, |view, cx| {
                            if let Some(copied_at) = view.copied_at {
                                if copied_at.elapsed() > Duration::from_millis(1400) {
                                    view.copied_key = None;
                                    view.copied_at = None;
                                    cx.notify();
                                }
                            }

                            match &mut view.status {
                                HudStatus::Listening { audio_level, .. } => {
                                    *audio_level = current_level;
                                    for (slot, target) in
                                        view.wave_peaks.iter_mut().zip(vis_peaks)
                                    {
                                        if target > *slot {
                                            *slot += (target - *slot) * 0.58;
                                        } else {
                                            *slot += (target - *slot) * 0.24;
                                        }
                                    }
                                    cx.notify();
                                }
                                HudStatus::Transcribing { .. } => {
                                    cx.notify();
                                }
                                HudStatus::Success { finished_at, .. } => {
                                    if finished_at.elapsed() > Duration::from_millis(1800) {
                                        view.status = HudStatus::Idle;
                                        cx.notify();
                                    }
                                }
                                HudStatus::Error { occurred_at, .. } => {
                                    if occurred_at.elapsed() > Duration::from_millis(2400) {
                                        view.status = HudStatus::Idle;
                                        cx.notify();
                                    }
                                }
                                HudStatus::Idle => {}
                            }
                        });
                    }
                })
                .detach();

                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(8));
                    if update::updates_disabled() {
                        return;
                    }
                    let ticket = update::begin_update_check();
                    *update_for_boot.lock() = UpdatePhase::Checking;
                    let _ = update_ping_boot.send(());
                    let next = match update::check_latest() {
                        Ok(phase) => phase,
                        Err(error) => UpdatePhase::Failed(error),
                    };
                    // A user-initiated check superseded this boot check; do not
                    // clobber its result.
                    if update::update_check_is_current(ticket) {
                        *update_for_boot.lock() = next;
                        let _ = update_ping_boot.send(());
                    }
                });

                HudView::new(
                    auto_paste,
                    hotkey_label,
                    initial_language,
                    stt_for_view,
                    auto_paste_view,
                    update_for_view,
                    update_ping_view,
                )
            })
        }) {
            Ok(_) => append_log("TDT window opened successfully."),
            Err(e) => {
                let msg = format!("Failed to open TDT window: {e:?}");
                append_log(&msg);
                show_error(&msg);
            }
        }
    });
}

enum InstanceClaim {
    Owned(InstanceGuard),
    AlreadyRunning,
    Failed(String),
}

struct InstanceGuard {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

fn claim_instance() -> InstanceClaim {
    #[cfg(windows)]
    {
        use windows::core::w;
        use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows::Win32::System::Threading::CreateMutexW;

        unsafe {
            match CreateMutexW(None, true, w!("Local\\TDT-TalkDontType")) {
                Ok(handle) => {
                    if GetLastError() == ERROR_ALREADY_EXISTS {
                        let _ = windows::Win32::Foundation::CloseHandle(handle);
                        InstanceClaim::AlreadyRunning
                    } else {
                        InstanceClaim::Owned(InstanceGuard { handle })
                    }
                }
                Err(error) => InstanceClaim::Failed(format!("Could not start TDT: {error}")),
            }
        }
    }
    #[cfg(not(windows))]
    {
        InstanceClaim::Owned(InstanceGuard {})
    }
}

fn log_file() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("tdt.log")))
        .unwrap_or_else(|| std::env::temp_dir().join("tdt.log"))
}

fn append_log(message: &str) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file())
    {
        let _ = writeln!(file, "{message}");
    }
}

fn show_error(message: &str) {
    #[cfg(windows)]
    {
        use windows::core::HSTRING;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        unsafe {
            let _ = MessageBoxW(
                None,
                &HSTRING::from(message),
                &HSTRING::from("TDT"),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("{message}");
    }
}
