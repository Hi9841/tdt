#![cfg_attr(not(test), windows_subsystem = "windows")]

#[cfg(not(target_os = "windows"))]
compile_error!(
    "TDT desktop is a Windows-only build. Build the Android app from the `mobile` folder instead."
);

mod audio;
mod autostart;
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
use paste::{DeliveryOutcome, PasteInjector};
use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use stt::{models, SharedEngine, SttEngine};
use tray_icon::menu::MenuEvent;
use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
use ui::preview;
use ui::tray::tooltip_text;
use ui::window_util::{
    find_app_hwnd, follow_current_virtual_desktop, lock_overlay_chrome,
    position_bubble_on_preferred_monitor, set_overlay_hidden, BUBBLE_HEIGHT, BUBBLE_WIDTH,
};
use ui::{HudStatus, HudView, SystemTray};
use update::UpdatePhase;

enum InternalEvent {
    TranscribeSuccess {
        text: String,
        auto_pasted: bool,
        notice: Option<String>,
        _duration_secs: f32,
        latency_ms: u64,
    },
    TranscribeError(String),
    NoSpeech,
    PrepareFailed {
        session: u64,
        message: String,
    },
}

fn main() {
    lock_working_directory();
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("TDT crashed: {info}");
        append_log(&msg);
        show_error(&msg);
    }));

    let _instance = if preview::is_active() {
        None
    } else {
        Some(match claim_instance() {
            InstanceClaim::AlreadyRunning => return,
            InstanceClaim::Owned(guard) => guard,
            InstanceClaim::Failed(error) => {
                append_log(&error);
                show_error(&error);
                return;
            }
        })
    };

    let config = if preview::is_active() {
        AppConfig::default()
    } else {
        AppConfig::load()
    };
    let auto_paste = config.auto_paste;
    let hotkey_binding = HotkeyBinding::parse(&config.hotkey).unwrap_or_default();
    set_binding(hotkey_binding);
    let hotkey_label = hotkey_binding.display();

    // 1. Initialize audio recorder
    let (recorder, microphone_error) = if preview::is_active() {
        (None, None)
    } else {
        match AudioRecorder::new() {
            Ok(recorder) => (Some(recorder), None),
            Err(error) => (None, Some(format!("Could not start the microphone. Connect a microphone and choose Retry microphone. {error}"))),
        }
    };
    let recorder = Rc::new(RefCell::new(recorder));

    // 2. Initialize STT Engine
    let model_spec = models::resolve(&config.model_id);
    let stt_engine: SharedEngine = Arc::new(Mutex::new(
        match models::find_dir(model_spec, config.model_dir.as_deref()) {
            Some(dir) => match SttEngine::new(model_spec, &dir, &config.language) {
                Ok(engine) => {
                    append_log(&format!(
                        "Sherpa-ONNX {} model found at: {}",
                        model_spec.label,
                        dir.display()
                    ));
                    Some(Arc::new(engine))
                }
                Err(e) => {
                    append_log(&format!(
                        "Failed to initialize {} STT Engine: {e}",
                        model_spec.label
                    ));
                    None
                }
            },
            None => {
                append_log(&format!(
                    "{} is not downloaded. Open Settings to download it, or run models/download-models.ps1.",
                    model_spec.label
                ));
                None
            }
        },
    ));

    // 3. Initialize paste injector
    let injector = Arc::new(PasteInjector::new());

    // 4. Initialize hotkey hook
    let (_hotkey_guard, hotkey_rx) = if preview::is_active() {
        (None, crossbeam_channel::never())
    } else {
        let (guard, rx) = HotkeyListener::start();
        (Some(guard), rx)
    };

    // 5. Launch GPUI application with floating bubble window
    let app = Application::new();
    let recorder_clone = Rc::clone(&recorder);
    let injector_clone = Arc::clone(&injector);
    let stt_clone = stt_engine.clone();
    let stt_for_view = stt_engine.clone();

    app.run(move |cx: &mut App| {
        // Windows requires the event loop to exist before creating the tray icon.
        let tray = if preview::is_active() { None } else { match SystemTray::new(auto_paste, &hotkey_label) {
            Ok(tray) => Some(tray),
            Err(error) => {
                append_log(&format!("Failed to create system tray: {error}"));
                None
            }
        } };

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::new(px(700.0), px(860.0)),
                size: Size::new(px(BUBBLE_WIDTH), px(BUBBLE_HEIGHT)),
            })),
            titlebar: None,
            focus: false,
            // GPUI only requests frames for windows it owns as visible. Showing
            // a hidden HWND later through Win32 leaves the surface transparent.
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
            let mut overlay = None;
            for _ in 0..60 {
                overlay = find_app_hwnd();
                if overlay.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let Some(hwnd) = overlay else {
                return;
            };

            ui::window_util::apply_overlay_window_style(hwnd);

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
                    let mut recording_session = 0u64;
                    let mut limit_notice = false;

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
                        if take_show_request() {
                            set_overlay_hidden(false);
                            let _ = this.update(cx, |view, cx| {
                                view.reveal_overlay();
                                cx.notify();
                            });
                        }
                        while update_ping_rx.try_recv().is_ok() {
                            let _ = this.update(cx, |view, cx| {
                                view.take_ready_model();
                                cx.notify();
                            });
                        }
                        let _ = this.update(cx, |view, cx| {
                            view.take_ready_model();
                            if view.retry_microphone {
                                view.retry_microphone = false;
                                if !preview::is_active() && !is_recording_state && !is_processing_state {
                                    match AudioRecorder::new() {
                                        Ok(recorder) => {
                                            *rec.borrow_mut() = Some(recorder);
                                            view.status = HudStatus::Idle;
                                            view.recovery_message = None;
                                        }
                                        Err(error) => view.set_error(format!("Could not open the microphone. Check Windows microphone access and try again. {error}")),
                                    }
                                }
                                cx.notify();
                            }
                        });

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
                                } else if event.id == tray.show_item.id() {
                                    set_overlay_hidden(false);
                                    let _ = this.update(cx, |view, cx| {
                                        view.reveal_overlay();
                                        cx.notify();
                                    });
                                } else if event.id == tray.updates_item.id() {
                                    let _ = this.update(cx, |view, cx| {
                                        view.start_update_check();
                                        view.open_settings(cx);
                                    });
                                }
                            }
                        }
                        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
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
                                    set_overlay_hidden(false);
                                    let _ = this.update(cx, |view, cx| {
                                        view.reveal_overlay();
                                        cx.notify();
                                    });
                                }
                                _ => {}
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

                        // Helper closure to run transcription in background thread
                        let dispatch_transcribe = |samples: Vec<f32>,
                                                   stt_worker: SharedEngine,
                                                   inj_worker: Arc<PasteInjector>,
                                                   tx: crossbeam_channel::Sender<InternalEvent>,
                                                   should_paste: bool,
                                                   target: Option<isize>| {
                            std::thread::spawn(move || {
                                let engine = stt_worker.lock().clone();
                                if let Some(ref engine) = engine {
                                    let start_t = Instant::now();
                                    let duration_secs = samples.len() as f32 / 16000.0;
                                    match engine.transcribe(&samples) {
                                        Ok(text) => {
                                            let latency_ms = start_t.elapsed().as_millis() as u64;
                                            if !text.trim().is_empty() {
                                                let mut stats = AppStats::load();
                                                stats.record_and_save(&text, duration_secs, latency_ms);
                                            }
                                            let (auto_pasted, notice) = match inj_worker.deliver(&text, should_paste, target) {
                                                Ok(DeliveryOutcome::NoSpeech) => { let _ = tx.send(InternalEvent::NoSpeech); return; }
                                                Ok(DeliveryOutcome::Copied) => (false, None),
                                                Ok(DeliveryOutcome::Pasted) => (true, None),
                                                Ok(DeliveryOutcome::CopiedFallback(_)) => (false, Some("Copied. Could not insert all text. Check the destination before pasting with Ctrl+V.".into())),
                                                Err(error) => {
                                                    let _ = tx.send(InternalEvent::TranscribeError(format!("Could not copy text. Open History to copy the saved transcript. {error}")));
                                                    return;
                                                }
                                            };
                                            let _ = tx.send(InternalEvent::TranscribeSuccess {
                                                text,
                                                auto_pasted,
                                                notice,
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
                                        "Speech model not ready. Open Settings to download it."
                                            .to_string(),
                                    ));
                                }
                            });
                        };

                        // 2. Process Hotkey Events
                        if preview::is_active() {
                            while hotkey_rx.try_recv().is_ok() {}
                        }
                        let mut actions: Vec<_> = hotkey_rx.try_iter().collect();
                        let stream_error = rec.borrow().as_ref().and_then(AudioRecorder::take_error);
                        if let Some(error) = stream_error {
                            if let Some(recorder) = rec.borrow().as_ref() { recorder.stop(); }
                            *rec.borrow_mut() = None;
                            is_recording_state = false;
                            let _ = this.update(cx, |view, cx| { view.set_error(error); cx.notify(); });
                        }
                        let at_limit = is_recording_state && rec.borrow().as_ref().is_some_and(AudioRecorder::limit_reached);
                        let stop_requested = this.update(cx, |view, _| std::mem::take(&mut view.stop_requested)).unwrap_or(false);
                        if is_recording_state && (at_limit || stop_requested) {
                            limit_notice = at_limit;
                            actions.insert(0, HotkeyAction::HoldReleased);
                        }
                        for action in actions {
                            if preview::is_active() {
                                continue;
                            }
                            match action {
                                HotkeyAction::Captured(binding) => {
                                    let label = binding.display();
                                    set_binding(binding);
                                    let mut cfg = AppConfig::load();
                                    cfg.hotkey = label.clone();
                                    let _ = cfg.save();
                                    if let Some(ref tray) = tray {
                                        let _ = tray
                                            .tray_icon
                                            .set_tooltip(Some(tooltip_text(&label)));
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
                                        set_overlay_hidden(false);
                                        if stt.lock().is_none() {
                                            let _ = this.update(cx, |view, cx| {
                                                view.set_error("Download a speech model in Settings before recording.".into());
                                                view.open_settings(cx);
                                                cx.notify();
                                            });
                                            continue;
                                        }
                                        if rec.borrow().is_none() {
                                            match AudioRecorder::new() {
                                                Ok(recorder) => *rec.borrow_mut() = Some(recorder),
                                                Err(error) => {
                                                    let _ = this.update(cx, |view, cx| { view.set_error(format!("Could not open the microphone. Connect a microphone and retry. {error}")); cx.notify(); });
                                                    continue;
                                                }
                                            }
                                        }
                                        *target_hwnd.lock() = ui::window_util::dictation_target_window();
                                        let _ = this.update(cx, |view, cx| view.close_stats_settings(cx));
                                        if let Some(recorder) = rec.borrow().as_ref() { recorder.start(); }
                                        recording_session = recording_session.wrapping_add(1);
                                        limit_notice = false;
                                        if let Some(engine) = stt.lock().clone() {
                                            let tx = internal_tx.clone();
                                            let session = recording_session;
                                            std::thread::spawn(move || {
                                                if let Err(error) = engine.prepare() {
                                                    let _ = tx.send(InternalEvent::PrepareFailed { session, message: format!("Could not load the speech model. Choose another model in Settings. {error}") });
                                                }
                                            });
                                        }
                                        play_sound(SoundEffect::StartListening);
                                        is_recording_state = true;
                                        let started_at = Instant::now();
                                        let _ = this.update(cx, |view, cx| {
                                            view.recovery_message = None;
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
                                        let samples = rec.borrow().as_ref().map(AudioRecorder::stop).unwrap_or_default();
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
                                            if limit_notice { view.recovery_message = Some("Recording stopped at the 2-minute limit. Transcribing the recorded audio.".into()); }
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
                            match event {
                                InternalEvent::TranscribeSuccess { text, auto_pasted, notice, latency_ms, .. } => {
                                    is_processing_state = false;
                                    play_sound(SoundEffect::Success);
                                    let word_count = text.split_whitespace().count();
                                    println!("Transcription completed ({} words, latency: {}ms)", word_count, latency_ms);
                                    let finished_at = Instant::now();
                                    let _ = this.update(cx, |view, cx| {
                                        view.stats = AppStats::load();
                                        if notice.is_some() { view.recovery_message = notice; }
                                        view.status = HudStatus::Success {
                                            text,
                                            auto_pasted,
                                            finished_at,
                                        };
                                        cx.notify();
                                    });
                                }
                                InternalEvent::TranscribeError(err) => {
                                    is_processing_state = false;
                                    play_sound(SoundEffect::Error);
                                    let _ = this.update(cx, |view, cx| {
                                        view.stats = AppStats::load();
                                        view.set_error(err);
                                        cx.notify();
                                    });
                                }
                                InternalEvent::NoSpeech => {
                                    is_processing_state = false;
                                    let _ = this.update(cx, |view, cx| {
                                        view.status = HudStatus::NoSpeech;
                                        view.recovery_message = Some("No speech detected. Try again and check your microphone.".into());
                                        cx.notify();
                                    });
                                }
                                InternalEvent::PrepareFailed { session, message } => {
                                    if session == recording_session && is_recording_state {
                                        if let Some(recorder) = rec.borrow().as_ref() { recorder.stop(); }
                                        is_recording_state = false;
                                        let _ = this.update(cx, |view, cx| { view.set_error(message); cx.notify(); });
                                    }
                                }
                            }
                        }

                        // 4. Animation frame & audio level updates
                        let current_level = if is_recording_state { rec.borrow().as_ref().map_or(0.0, AudioRecorder::audio_level) } else { 0.0 };
                        // Avoid locking the audio visualization buffer when it
                        // cannot be displayed.
                        let vis_peaks = if is_recording_state { rec.borrow().as_ref().map(AudioRecorder::vis_peaks) } else { None };

                        let _ = this.update(cx, |view, cx| {
                            if !preview::is_active() {
                                if let Some(copied_at) = view.copied_at {
                                    if copied_at.elapsed() > Duration::from_millis(1400) {
                                        view.copied_key = None;
                                        view.copied_at = None;
                                        cx.notify();
                                    }
                                }
                            }

                            match &mut view.status {
                                HudStatus::Listening { audio_level, .. } => {
                                    if preview::is_active() {
                                        cx.notify();
                                    } else {
                                        *audio_level = current_level;
                                        if let Some(peaks) = vis_peaks {
                                            // Already smoothed in audio time. A second frame-based
                                            // filter would smear syllables across historical bars.
                                            view.wave_peaks = peaks;
                                        }
                                        cx.notify();
                                    }
                                }
                                // GPUI's pulse requests its own animation frames.
                                // A second 40Hz redraw loop is redundant, and keeps
                                // repainting even when reduced motion is enabled.
                                HudStatus::Transcribing { .. } => {}
                                HudStatus::Success { finished_at, .. } => {
                                    if !preview::is_active()
                                        && finished_at.elapsed() > Duration::from_millis(1800)
                                    {
                                        view.status = HudStatus::Idle;
                                        cx.notify();
                                    }
                                }
                                HudStatus::Error { .. } | HudStatus::NoSpeech | HudStatus::Idle => {}
                            }
                        });
                    }
                })
                .detach();

                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(8));
                    if preview::is_active() || update::updates_disabled() {
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

                let mut view = HudView::new(
                    auto_paste,
                    hotkey_label,
                    stt_for_view,
                    auto_paste_view,
                    update_for_view,
                    update_ping_view,
                    cx,
                );
                if let Some(error) = microphone_error { view.set_error(error); }
                view
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
    mutex: windows::Win32::Foundation::HANDLE,
    #[cfg(windows)]
    show_event: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
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
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::WaitForSingleObject;
        let Some(raw) = *show_event_slot().lock() else {
            return false;
        };
        unsafe { WaitForSingleObject(HANDLE(raw as _), 0) == WAIT_OBJECT_0 }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn signal_running_instance() {
    #[cfg(windows)]
    {
        use windows::core::w;
        use windows::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};
        unsafe {
            if let Ok(handle) = OpenEventW(EVENT_MODIFY_STATE, false, w!("Local\\TDT-ShowOverlay"))
            {
                let _ = SetEvent(handle);
                let _ = windows::Win32::Foundation::CloseHandle(handle);
            }
        }
    }
}

fn claim_instance() -> InstanceClaim {
    #[cfg(windows)]
    {
        use windows::core::w;
        use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows::Win32::System::Threading::{CreateEventW, CreateMutexW};

        unsafe {
            match CreateMutexW(None, true, w!("Local\\TDT-TalkDontType")) {
                Ok(mutex) => {
                    if GetLastError() == ERROR_ALREADY_EXISTS {
                        let _ = windows::Win32::Foundation::CloseHandle(mutex);
                        signal_running_instance();
                        InstanceClaim::AlreadyRunning
                    } else {
                        match CreateEventW(None, false, false, w!("Local\\TDT-ShowOverlay")) {
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
    #[cfg(not(windows))]
    {
        InstanceClaim::Owned(InstanceGuard {})
    }
}

fn lock_working_directory() {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let _ = std::env::set_current_dir(dir);
        }
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
