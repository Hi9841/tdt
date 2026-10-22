#![cfg_attr(not(test), windows_subsystem = "windows")]

#[cfg(not(target_os = "windows"))]
compile_error!("TDT desktop is a Windows-only build.");

mod audio;
mod autostart;
mod config;
mod hotkey;
mod paste;
mod ui;
mod update;

// The speech engine frees and allocates large buffers on every take; this
// allocator keeps them mapped instead of returning them to Windows.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use audio::{play_sound, prewarm, AudioRecorder, SoundEffect, SpeechTail, MAX_TAIL, VIS_BARS};
use config::{AppConfig, AppStats};
use ely_gpui_component::Assets;
use gpui::*;
use hotkey::{
    current_binding, input_appeared, input_mask, is_modifier_vk, note_user_typed,
    poll_capture_timeout, set_binding, user_typed, HotkeyAction, HotkeyBinding, HotkeyListener,
};
use parking_lot::Mutex;
use paste::{DeliveryOutcome, PasteInjector};
use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tray_icon::menu::MenuEvent;
use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
use ui::preview;
use ui::tray::tooltip_text;
use ui::window_util::{
    ensure_overlay_topmost, find_app_hwnd, follow_current_virtual_desktop, lock_overlay_chrome,
    position_bubble_on_preferred_monitor, PANEL_HEIGHT, PANEL_WIDTH,
};
use ui::{HudStatus, HudView, SystemTray};
use update::UpdatePhase;
use voice_stt_desktop::stt::{
    models, remove_fillers, EngineSlot, LiveAudio, LiveTranscript, SttEngine,
};

enum InternalEvent {
    TranscribeSuccess {
        text: String,
        auto_pasted: bool,
        notice: Option<String>,
        latency_ms: u64,
    },
    TranscribeError(String),
    NoSpeech,
}

fn deliver_transcript(
    mut result: LiveTranscript,
    injector: Arc<PasteInjector>,
    tx: crossbeam_channel::Sender<InternalEvent>,
    should_paste: bool,
    clean_fillers: bool,
    target: Option<isize>,
    user_typed: bool,
) {
    if clean_fillers {
        result.text = remove_fillers(&result.text);
    }
    let delivered = injector.apply_final(&result.text, should_paste, target, user_typed);
    // Text reaches the user first; the history write is not on the latency path.
    // A failed copy still lands in History so the transcript is not lost.
    if !result.text.trim().is_empty() {
        let mut stats = AppStats::load();
        stats.record_and_save(&result.text, result.duration_secs, result.latency_ms);
    }
    let (auto_pasted, notice) = match delivered {
        Ok(DeliveryOutcome::NoSpeech) => {
            let _ = tx.send(InternalEvent::NoSpeech);
            return;
        }
        Ok(DeliveryOutcome::Copied) => (false, None),
        Ok(DeliveryOutcome::Pasted) => (true, None),
        Ok(DeliveryOutcome::CopiedFallback(reason)) => (
            false,
            Some(if reason.starts_with("You typed") {
                reason
            } else {
                "Copied. Could not insert all text. Check the destination before pasting with Ctrl+V."
                    .to_string()
            }),
        ),
        Err(error) => {
            let _ = tx.send(InternalEvent::TranscribeError(format!(
                "Could not copy text. Open History to copy the saved transcript. {error}"
            )));
            return;
        }
    };
    let _ = tx.send(InternalEvent::TranscribeSuccess {
        text: result.text,
        auto_pasted,
        notice,
        latency_ms: result.latency_ms,
    });
}

fn main() {
    lock_working_directory();
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("TDT crashed: {info}");
        append_log(&msg);
        // Best effort: a panic here would abort without a useful log.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            show_error(&msg);
        }));
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
    if !preview::is_active() {
        autostart::refresh_if_enabled();
        update::remove_downloaded_setups();
    }

    let mut config = if preview::is_active() {
        AppConfig::default()
    } else {
        AppConfig::load()
    };
    // Persist the exact resolved internal model id so app restarts reopen the
    // same STT backend (and migrate retired ids forward).
    let resolved_id = models::resolve(&config.model_id).id.to_string();
    if !preview::is_active() && resolved_id != config.model_id {
        config.model_id = resolved_id;
        let _ = config.save();
    } else {
        config.model_id = resolved_id;
    }
    let auto_paste = config.auto_paste;
    let remove_fillers_state = Arc::new(AtomicBool::new(config.remove_fillers));
    update::set_auto_checks(config.check_updates);
    let hotkey_binding = HotkeyBinding::parse(&config.hotkey).unwrap_or_else(|| {
        eprintln!(
            "Could not parse the configured hotkey '{}'; using the default.",
            config.hotkey
        );
        let binding = HotkeyBinding::default();
        config.hotkey = binding.display();
        if !preview::is_active() {
            let _ = config.save();
        }
        binding
    });
    set_binding(hotkey_binding);
    let hotkey_label = hotkey_binding.display();

    // Open the microphone before the window. Doing it on the GPUI executor
    // blocks the first frame, and the popup then stays without WS_VISIBLE.
    let (recorder, microphone_error) = if preview::is_active() {
        (None, None)
    } else {
        std::thread::spawn(prewarm);
        match AudioRecorder::new() {
            Ok(recorder) => {
                append_log(&format!("Microphone: {}", recorder.device_name()));
                (Some(recorder), None)
            }
            Err(error) => (
                None,
                Some(format!(
                    "Could not start the microphone. Connect a microphone and choose Retry microphone. {error}"
                )),
            ),
        }
    };
    let recorder = Rc::new(RefCell::new(recorder));

    // 2. Initialize STT Engine
    let model_spec = models::resolve(&config.model_id);
    let stt_engine = EngineSlot::default();
    stt_engine.set(
        if let Some(dir) = models::find_dir(model_spec, config.model_dir.as_deref()) {
            append_log(&format!(
                "{} speech model found at: {}",
                model_spec.label,
                dir.display()
            ));
            Some(Arc::new(SttEngine::new(&dir)))
        } else {
            append_log(&format!(
                "{} is not downloaded. Open Settings to download it, or run models/download-models.ps1.",
                model_spec.label
            ));
            None
        },
    );

    // Load and warm the selected model while the app opens, so a short first
    // dictation does not pay the full initialization cost after release.
    // prepare() serializes with recording and retries on failure.
    if !preview::is_active() {
        if let Some(engine) = stt_engine.get() {
            std::thread::spawn(move || {
                if let Err(error) = engine.prepare() {
                    append_log(&format!("Could not prepare the speech model: {error}"));
                }
            });
        }
    }

    // 3. Initialize paste injector
    let injector = Arc::new(PasteInjector::new());

    // 4. Initialize hotkey hook. A failed install must not look like a working shortcut.
    let _hotkey_guard;
    let hotkey_rx;
    let hotkey_error;
    if preview::is_active() {
        _hotkey_guard = None;
        hotkey_rx = crossbeam_channel::never();
        hotkey_error = None;
    } else {
        match HotkeyListener::start() {
            Ok((guard, rx)) => {
                _hotkey_guard = Some(guard);
                hotkey_rx = rx;
                // The hook installs fine at low integrity but never sees keys
                // typed into normal apps, so say why instead of failing silently.
                hotkey_error = low_rights_message();
                if let Some(message) = &hotkey_error {
                    append_log(message);
                }
            }
            Err(error) => {
                append_log(&format!("Hotkey hook failed: {error}"));
                _hotkey_guard = None;
                hotkey_rx = crossbeam_channel::never();
                hotkey_error = Some(error);
            }
        }
    };

    // 5. Launch GPUI application with floating bubble window.
    // Ely's components read fonts and colors registered by init.
    let app = gpui_platform::application().with_assets(Assets);
    let recorder_clone = Rc::clone(&recorder);
    let injector_clone = Arc::clone(&injector);
    let stt_clone = stt_engine.clone();
    let stt_for_view = stt_engine.clone();

    app.run(move |cx: &mut App| {
        ely_gpui_component::init(cx);
        ui::theme::install(cx);
        // Windows requires the event loop to exist before creating the tray icon.
        let mut tray = if preview::is_active() && !preview::wants_tray() { None } else { match SystemTray::new(auto_paste, &hotkey_label) {
            Ok(tray) => Some(tray),
            Err(error) => {
                append_log(&format!("Failed to create system tray: {error}"));
                None
            }
        } };

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
            is_resizable: false,
            kind: WindowKind::PopUp,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };

        let auto_paste_state = Arc::new(Mutex::new(auto_paste));
        let auto_paste_bg = Arc::clone(&auto_paste_state);
        let auto_paste_view = Arc::clone(&auto_paste_state);
        let fillers_bg = Arc::clone(&remove_fillers_state);
        let fillers_view = Arc::clone(&remove_fillers_state);
        let update_state = Arc::new(Mutex::new(UpdatePhase::Idle));
        let update_for_view = Arc::clone(&update_state);
        let update_for_boot = Arc::clone(&update_state);
        let update_for_loop = Arc::clone(&update_state);
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
            // The overlay window can appear late on cold starts; keep looking
            // at a slower cadence instead of skipping styling for the session.
            let mut attempts = 0;
            while overlay.is_none() && attempts < 20 {
                std::thread::sleep(Duration::from_millis(500));
                overlay = find_app_hwnd();
                attempts += 1;
            }
            let Some(hwnd) = overlay else {
                return;
            };

            ui::window_util::apply_overlay_window_style(hwnd);
            ui::window_util::keep_pill_from_activating(hwnd);
            ui::window_util::show_overlay_window(hwnd);

            position_bubble_on_preferred_monitor(hwnd);
            follow_current_virtual_desktop();
            // Strips caption and resize handles and snaps the pill back to size.
            lock_overlay_chrome();
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
                let rec = Rc::clone(&recorder_clone);
                let inj = Arc::clone(&injector_clone);
                let stt = stt_clone.clone();
                let auto_paste_flag = Arc::clone(&auto_paste_bg);
                let fillers_flag = Arc::clone(&fillers_bg);
                let target_hwnd = Arc::clone(&target_hwnd_loop);
                let update_phase = Arc::clone(&update_for_loop);

                // Spawn UI coordination loop running on GPUI executor
                cx.spawn(async move |this, cx| {
                    struct LiveTake {
                        tail: Arc<Mutex<SpeechTail>>,
                        release: Arc<AtomicBool>,
                        halt: Arc<AtomicBool>,
                        done: Arc<AtomicBool>,
                        hit_limit: Arc<AtomicBool>,
                        discard: Arc<AtomicBool>,
                        baseline: Option<[bool; 256]>,
                        released_at: Option<Instant>,
                    }
                    let mut is_recording_state = false;
                    let mut is_processing_state = false;
                    let mut recording_before_press = false;
                    let mut recording_session = 0u64;
                    let mut limit_notice = false;
                    let mut no_speech_at: Option<Instant> = None;
                    let mut session: Option<LiveTake> = None;

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
                        ensure_overlay_topmost();
                        poll_capture_timeout();
                        if take_show_request() {
                            let _ = this.update(cx, |view, cx| {
                                view.open_settings(cx);
                            });
                        }
                        while update_ping_rx.try_recv().is_ok() {}
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
                                    if !preview::is_active() {
                                        let mut cfg = AppConfig::load();
                                        cfg.auto_paste = current;
                                        let _ = cfg.save();
                                    }
                                    let _ = this.update(cx, |view, cx| {
                                        view.auto_paste_enabled = current;
                                        cx.notify();
                                    });
                                } else if event.id == tray.settings_item.id() {
                                    let _ = this.update(cx, |view, cx| {
                                        view.open_settings(cx);
                                    });
                                } else if event.id == tray.stop_item.id() {
                                    let _ = this.update(cx, |view, _| view.stop_requested = true);
                                } else if event.id == tray.show_item.id() {
                                    let _ = this.update(cx, |view, cx| {
                                        view.open_settings(cx);
                                    });
                                } else if event.id == tray.updates_item.id() {
                                    let _ = this.update(cx, |view, cx| {
                                        view.check_for_update(true);
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
                                    let _ = this.update(cx, |view, cx| {
                                        view.open_settings(cx);
                                    });
                                }
                                _ => {}
                            }
                        }
                        if quitting {
                            if let Some(live) = session.take() {
                                live.discard.store(true, Ordering::SeqCst);
                                live.halt.store(true, Ordering::SeqCst);
                            }
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

                        // 2. Process Hotkey Events
                        if preview::is_active() {
                            while hotkey_rx.try_recv().is_ok() {}
                        }
                        let mut actions: Vec<_> = hotkey_rx.try_iter().collect();
                        // A click on the resting pill is a tap of the shortcut.
                        let clicked = this
                            .update(cx, |view, _| std::mem::take(&mut view.click_dictation))
                            .unwrap_or(false);
                        if clicked && !is_recording_state && !is_processing_state {
                            actions.push(HotkeyAction::PressStarted);
                            actions.push(HotkeyAction::Toggled);
                        }
                        let stream_error = rec.borrow().as_ref().and_then(AudioRecorder::take_error);
                        if let Some(error) = stream_error {
                            if let Some(live) = session.take() {
                                live.discard.store(true, Ordering::SeqCst);
                                live.halt.store(true, Ordering::SeqCst);
                            }
                            *rec.borrow_mut() = None;
                            is_recording_state = false;
                            is_processing_state = false;
                            let _ = this.update(cx, |view, cx| { view.set_error(error); cx.notify(); });
                        }
                        let hit_limit = is_recording_state
                            && session.as_ref().is_some_and(|live| live.hit_limit.load(Ordering::SeqCst));
                        let at_limit = is_recording_state
                            && (hit_limit
                                || rec.borrow().as_ref().is_some_and(AudioRecorder::limit_reached));
                        let stop_requested = if is_recording_state {
                            this.update(cx, |view, _| std::mem::take(&mut view.stop_requested)).unwrap_or(false)
                        } else {
                            false
                        };
                        let mut stop_now = false;
                        if is_recording_state && (at_limit || stop_requested) {
                            limit_notice = at_limit;
                            stop_now = true;
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
                                    // Snapshot the before-press state; it is
                                    // cleared on every release below, so a
                                    // swallowed press can never stop a session
                                    // it did not start.
                                    recording_before_press = is_recording_state;
                                    // Do not overlap recordings with an in-flight
                                    // transcription. The current recorder and STT
                                    // engine are single-session by design.
                                    let pump_busy = session
                                        .as_ref()
                                        .is_some_and(|live| !live.done.load(Ordering::SeqCst));
                                    if !is_recording_state && !is_processing_state && !pump_busy {
                                        if stt.get().is_none() {
                                            let _ = this.update(cx, |view, cx| {
                                                view.set_error("Download a speech model in Settings before recording.".into());
                                                view.open_settings(cx);
                                                cx.notify();
                                            });
                                            continue;
                                        }
                                        let reopen = {
                                            let borrowed = rec.borrow();
                                            borrowed.as_ref().is_some_and(|recorder| {
                                                recorder.needs_reopen()
                                                    || recorder.take_error().is_some()
                                            })
                                        };
                                        if rec.borrow().is_none() || reopen {
                                            *rec.borrow_mut() = None;
                                            match AudioRecorder::new() {
                                                Ok(recorder) => {
                                                    append_log(&format!(
                                                        "Microphone: {}",
                                                        recorder.device_name()
                                                    ));
                                                    *rec.borrow_mut() = Some(recorder);
                                                }
                                                Err(error) => {
                                                    let _ = this.update(cx, |view, cx| { view.set_error(format!("Could not open the microphone. Connect a microphone and retry. {error}")); cx.notify(); });
                                                    continue;
                                                }
                                            }
                                        }
                                        *target_hwnd.lock() = ui::window_util::dictation_target_window();
                                        let _ = this.update(cx, |view, cx| view.close_stats_settings(cx));
                                        if let Some(recorder) = rec.borrow().as_ref() { recorder.resume_input(); recorder.start(); }
                                        let Some(handle) = rec.borrow().as_ref().map(|recorder| recorder.handle()) else {
                                            continue;
                                        };
                                        recording_session = recording_session.wrapping_add(1);
                                        limit_notice = false;
                                        let tail = Arc::new(Mutex::new(SpeechTail::default()));
                                        let release = Arc::new(AtomicBool::new(false));
                                        let halt = Arc::new(AtomicBool::new(false));
                                        let done = Arc::new(AtomicBool::new(false));
                                        let hit_limit = Arc::new(AtomicBool::new(false));
                                        let discard = Arc::new(AtomicBool::new(false));
                                        let (tx, rx) = crossbeam_channel::unbounded();
                                        if let Some(engine) = stt.get() {
                                            let events = internal_tx.clone();
                                            let inj_worker = Arc::clone(&inj);
                                            let paste_flag = Arc::clone(&auto_paste_flag);
                                            let fillers_worker = Arc::clone(&fillers_flag);
                                            let target_flag = Arc::clone(&target_hwnd);
                                            let discard_worker = Arc::clone(&discard);
                                            std::thread::spawn(move || {
                                                match engine.transcribe_live_reporting(&rx) {
                                                    Ok(result) => {
                                                        if discard_worker.load(Ordering::SeqCst) {
                                                            return;
                                                        }
                                                        let should_paste = *paste_flag.lock();
                                                        let clean = fillers_worker.load(Ordering::Relaxed);
                                                        let target = *target_flag.lock();
                                                        deliver_transcript(result, inj_worker, events, should_paste, clean, target, user_typed());
                                                    }
                                                    Err(error) => {
                                                        if discard_worker.load(Ordering::SeqCst) {
                                                            return;
                                                        }
                                                        let _ = events.send(InternalEvent::TranscribeError(format!("Could not load the speech model. Choose another model in Settings. {error}")));
                                                    }
                                                }
                                            });
                                        } else {
                                            drop(rx);
                                        }
                                        let pump_tx = tx.clone();
                                        let pump_tail = Arc::clone(&tail);
                                        let pump_release = Arc::clone(&release);
                                        let pump_halt = Arc::clone(&halt);
                                        let pump_done = Arc::clone(&done);
                                        let pump_limit = Arc::clone(&hit_limit);
                                        drop(tx);
                                        std::thread::spawn(move || {
                                            loop {
                                                if pump_halt.load(Ordering::SeqCst) {
                                                    break;
                                                }
                                                if handle.limit_reached() {
                                                    pump_limit.store(true, Ordering::SeqCst);
                                                    break;
                                                }
                                                let chunk = handle.drain();
                                                let empty = chunk.is_empty();
                                                if !empty {
                                                    pump_tail.lock().push_samples(&chunk);
                                                    let _ = pump_tx.send(LiveAudio::Chunk(chunk));
                                                }
                                                if pump_release.load(Ordering::SeqCst) && pump_tail.lock().should_stop() {
                                                    break;
                                                }
                                                if empty {
                                                    handle.wait_for_audio(Duration::from_millis(8));
                                                }
                                            }
                                            let rest = handle.stop();
                                            if !rest.is_empty() {
                                                let _ = pump_tx.send(LiveAudio::Chunk(rest));
                                            }
                                            let _ = pump_tx.send(LiveAudio::Finish(Instant::now()));
                                            pump_done.store(true, Ordering::SeqCst);
                                        });
                                        session = Some(LiveTake {
                                            tail,
                                            release,
                                            halt,
                                            done,
                                            hit_limit,
                                            discard,
                                            baseline: None,
                                            released_at: None,
                                        });
                                        play_sound(SoundEffect::StartListening);
                                        is_recording_state = true;
                                        no_speech_at = None;
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
                                        let stop = recording_before_press;
                                        recording_before_press = false;
                                        stop
                                    } else {
                                        recording_before_press = false;
                                        true
                                    };
                                    if should_stop && is_recording_state && !stop_now {
                                        let tail_open = session.as_ref().is_some_and(|live| live.baseline.is_some());
                                        if !tail_open {
                                            let baseline = input_mask();
                                            let typing = baseline.iter().enumerate().any(|(vk, down)| {
                                                *down
                                                    && !is_modifier_vk(vk as u16)
                                                    && vk as u16 != current_binding().vk
                                            });
                                            if typing {
                                                note_user_typed();
                                                stop_now = true;
                                            } else if let Some(live) = session.as_mut() {
                                                live.tail.lock().mark_release();
                                                live.released_at = Some(Instant::now());
                                                live.release.store(true, Ordering::SeqCst);
                                                live.baseline = Some(baseline);
                                            } else {
                                                stop_now = true;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if is_recording_state {
                            if let Some(baseline) = session.as_ref().and_then(|live| live.baseline.as_ref()) {
                                if input_appeared(baseline) {
                                    note_user_typed();
                                    stop_now = true;
                                }
                            }
                            // Sample time stops if the callback stalls. The cap is wall time too.
                            if session.as_ref().is_some_and(|live| {
                                live.released_at.is_some_and(|at| at.elapsed() >= MAX_TAIL)
                            }) {
                                stop_now = true;
                            }
                        }
                        if is_recording_state && session.as_ref().is_some_and(|live| live.done.load(Ordering::SeqCst)) {
                            if session.as_ref().is_some_and(|live| live.hit_limit.load(Ordering::SeqCst)) {
                                limit_notice = true;
                            }
                            stop_now = true;
                        }
                        if stop_now && is_recording_state {
                            if let Some(live) = session.as_mut() {
                                live.halt.store(true, Ordering::SeqCst);
                                live.baseline = None;
                            }
                            is_recording_state = false;
                            is_processing_state = true;
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
                        }

                        // 3. Process background transcription results
                        while let Ok(event) = internal_rx.try_recv() {
                            match event {
                                InternalEvent::TranscribeSuccess { text, auto_pasted, notice, latency_ms, .. } => {
                                    is_processing_state = false;
                                    play_sound(SoundEffect::Success);
                                    let word_count = text.split_whitespace().count();
                                    println!("Transcription completed ({word_count} words, latency: {latency_ms}ms)");
                                    let finished_at = Instant::now();
                                    let _ = this.update(cx, |view, cx| {
                                        view.stats = AppStats::load();
                                        view.recovery_message = notice;
                                        view.status = HudStatus::Success {
                                            text,
                                            auto_pasted,
                                            latency_ms,
                                            finished_at,
                                        };
                                        cx.notify();
                                    });
                                }
                                InternalEvent::TranscribeError(err) => {
                                    is_processing_state = false;
                                    if is_recording_state {
                                        if let Some(live) = session.as_mut() {
                                            live.halt.store(true, Ordering::SeqCst);
                                            live.baseline = None;
                                        }
                                        is_recording_state = false;
                                    }
                                    play_sound(SoundEffect::Error);
                                    let _ = this.update(cx, |view, cx| {
                                        view.stats = AppStats::load();
                                        view.set_error(err);
                                        cx.notify();
                                    });
                                }
                                InternalEvent::NoSpeech => {
                                    is_processing_state = false;
                                    no_speech_at = Some(Instant::now());
                                    let _ = this.update(cx, |view, cx| {
                                        view.status = HudStatus::NoSpeech;
                                        view.recovery_message = Some("No speech detected. Try again and check your microphone.".into());
                                        cx.notify();
                                    });
                                }

                            }
                        }

                        // 4. Animation frame & audio level updates
                        // Fully idle means no recording and no transcription:
                        // park the microphone until the next press.
                        if !is_recording_state && !is_processing_state {
                            if let Some(active) = rec.borrow().as_ref() {
                                active.pause_input();
                            }
                        }
                        let current_level = if is_recording_state { rec.borrow().as_ref().map_or(0.0, AudioRecorder::audio_level) } else { 0.0 };
                        // Avoid locking the audio visualization buffer when it
                        // cannot be displayed.
                        let vis_peaks = if is_recording_state { rec.borrow().as_ref().map(AudioRecorder::vis_peaks) } else { None };
                        let clear_no_speech = !preview::is_active()
                            && no_speech_at.is_some_and(|at| {
                                ui::hud::success_returns_to_idle(at.elapsed(), true)
                            });
                        if clear_no_speech {
                            no_speech_at = None;
                        }

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
                                        && ui::hud::success_returns_to_idle(
                                            finished_at.elapsed(),
                                            view.recovery_message.is_some(),
                                        )
                                    {
                                        view.status = HudStatus::Idle;
                                        view.recovery_message = None;
                                        cx.notify();
                                    }
                                }
                                HudStatus::NoSpeech if clear_no_speech => {
                                    view.status = HudStatus::Idle;
                                    view.recovery_message = None;
                                    cx.notify();
                                }
                                HudStatus::NoSpeech | HudStatus::Error { .. } | HudStatus::Idle => {}
                            }
                            if let Some(tray) = tray.as_mut() {
                                tray.update(&view.status, &view.hotkey_label);
                            }
                        });
                    }
                })
                .detach();

                // Check soon after start, then every hour, like Prism.
                if !preview::is_active() {
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(8));
                        loop {
                            update::check(&update_for_boot, false, &update_ping_boot);
                            std::thread::sleep(Duration::from_secs(60 * 60));
                        }
                    });
                }

                let mut view = HudView::new(
                    hotkey_label,
                    stt_for_view,
                    auto_paste_view,
                    fillers_view,
                    update_for_view,
                    update_ping_view,
                    cx,
                );
                if let Some(error) = microphone_error {
                    view.set_error(error);
                }
                if let Some(error) = hotkey_error {
                    let message = match &view.status {
                        HudStatus::Error { message: existing, .. }
                            if !existing.is_empty() && existing != &error =>
                        {
                            format!("{existing} {error}")
                        }
                        _ => error,
                    };
                    view.set_error(message);
                }
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

/// Windows starts an exe at low integrity when the file carries a low-rights
/// label (files saved into a folder with that label inherit it). A low process
/// cannot read keys from, or paste into, normal apps.
#[cfg(windows)]
fn low_rights_message() -> Option<String> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    const MEDIUM_RID: u32 = 0x2000;

    let rid = unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut buf = [0u64; 16];
        let mut len = 0u32;
        let read = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buf.as_mut_ptr().cast()),
            std::mem::size_of_val(&buf) as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        read.ok()?;
        let sid = (*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL)).Label.Sid;
        *GetSidSubAuthority(sid, u32::from(*GetSidSubAuthorityCount(sid)) - 1)
    };
    if rid >= MEDIUM_RID {
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

#[cfg(not(windows))]
fn low_rights_message() -> Option<String> {
    None
}

fn claim_instance() -> InstanceClaim {
    #[cfg(windows)]
    {
        use windows::core::w;
        use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows::Win32::System::Threading::{CreateEventW, CreateMutexW};

        unsafe {
            // CreateMutexW's wrapper does not surface ERROR_ALREADY_EXISTS.
            // Open the existing object first so a second launch cannot start
            // another speech worker.
            if let Ok(existing) = windows::Win32::System::Threading::OpenMutexW(
                windows::Win32::System::Threading::SYNCHRONIZATION_ACCESS_RIGHTS(0x00100000),
                false,
                w!("Local\\TDT-TalkDontType"),
            ) {
                let _ = windows::Win32::Foundation::CloseHandle(existing);
                signal_running_instance();
                return InstanceClaim::AlreadyRunning;
            }
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

const MAX_LOG_BYTES: u64 = 512 * 1024;

fn append_log(message: &str) {
    let path = log_file();
    let mut path = path;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .or_else(|_| {
            // The exe directory may be read-only (Program Files); fall back
            // to the temp dir once.
            path = std::env::temp_dir().join("tdt.log");
            OpenOptions::new().create(true).append(true).open(&path)
        });
    if let Ok(ref mut file) = file {
        if let Ok(metadata) = file.metadata() {
            if metadata.len() > MAX_LOG_BYTES {
                // Keep the latest half rather than growing forever.
                if let Ok(contents) = std::fs::read(&path) {
                    let keep = &contents[contents.len() / 2..];
                    let _ = std::fs::write(&path, keep);
                }
            }
        }
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
