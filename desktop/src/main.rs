#![cfg_attr(not(test), windows_subsystem = "windows")]

#[cfg(not(target_os = "windows"))]
compile_error!("TDT desktop is a Windows-only build.");

mod audio;
mod autostart;
mod config;
mod dictation;
mod hotkey;
mod integrity;
mod keys;
mod logfile;
mod paste;
mod priority;
mod ui;
mod update;

// The speech engine frees and allocates large buffers on every take; this
// allocator keeps them mapped instead of returning them to Windows.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use audio::{play_sound, prewarm, watch_default_input, AudioRecorder, SoundEffect};
use config::{AppConfig, AppStats, Settings};
use dictation::LiveTake;
use ely_gpui_component::Assets;
use gpui::*;
use hotkey::{
    current_binding, poll_capture_timeout, poll_missed_release, reset_user_typed, set_binding,
    user_typed, wake_loop, HotkeyAction, HotkeyBinding, HotkeyListener,
};
use parking_lot::Mutex;
use paste::{DeliveryOutcome, DeliverySteps, NotTyped, PasteInjector};
use std::cell::RefCell;
use std::rc::Rc;
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
use ui::{HudView, SystemTray};
use update::{CheckKind, UpdatePhase};
use voice_stt_desktop::log;
use voice_stt_desktop::stt::{
    models, remove_fillers, EngineSlot, Latency, LiveTranscript, SttEngine,
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
    settings: &Settings,
    target: Option<isize>,
    typed_key: Option<u16>,
) {
    let (should_paste, clean_fillers, save_take) =
        settings.get(|config| (config.auto_paste, config.remove_fillers, config.save_takes));
    // The model's own text, before the filler removal. The disk write must
    // not delay the text.
    if save_take {
        let (audio, text) = (std::mem::take(&mut result.audio), result.text.clone());
        std::thread::spawn(move || {
            if let Err(error) = voice_stt_desktop::stt::takes::save(&audio, &text) {
                log::note(&format!("Could not save the test take: {error}"));
            }
        });
    }
    if clean_fillers {
        result.text = remove_fillers(&result.text);
    }
    let mut steps = DeliverySteps::default();
    let delivered = injector.deliver(
        &result.text,
        should_paste,
        target,
        typed_key.is_some(),
        &mut steps,
    );
    // The wait of the user ends when the text is in the app. The copy to the
    // clipboard comes after that.
    let latency = result.latency(steps.typed_at.unwrap_or_else(Instant::now));
    log_delivery(&delivered, target, typed_key, latency, &result, steps);
    // Text reaches the user first; the history write is not on the latency path.
    // A failed copy still lands in History so the transcript is not lost.
    if !result.text.trim().is_empty() {
        AppStats::update(|stats| {
            stats.record(&result.text, result.duration_secs, latency.total_ms())
        });
    }
    let (auto_pasted, notice) = match delivered {
        Ok(DeliveryOutcome::NoSpeech) => {
            let _ = tx.send(InternalEvent::NoSpeech);
            return;
        }
        Ok(DeliveryOutcome::Copied) => (false, None),
        Ok(DeliveryOutcome::Pasted) => (true, None),
        Ok(DeliveryOutcome::PastedNotCopied(_)) => (
            true,
            Some("TDT typed the text, but could not copy it to the clipboard.".to_string()),
        ),
        Ok(DeliveryOutcome::CopiedFallback(reason)) => (false, Some(reason.notice().to_string())),
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
        latency_ms: latency.total_ms(),
    });
}

/// One line per dictation, so a paste that did not arrive or a slow take can
/// be explained from the log. It names the app, never the text.
fn log_delivery(
    delivered: &Result<DeliveryOutcome, String>,
    target: Option<isize>,
    typed_key: Option<u16>,
    latency: Latency,
    take: &LiveTranscript,
    steps: DeliverySteps,
) {
    let result = match delivered {
        Ok(DeliveryOutcome::NoSpeech) => return,
        Ok(DeliveryOutcome::Pasted) => "typed".to_string(),
        Ok(DeliveryOutcome::PastedNotCopied(error)) => format!("typed, not copied: {error}"),
        Ok(DeliveryOutcome::Copied) => "copied (auto-paste is off)".to_string(),
        Ok(DeliveryOutcome::CopiedFallback(NotTyped::UserTyped)) => format!(
            "copied, not typed: key 0x{:02X} went down while TDT was finishing",
            typed_key.unwrap_or(0)
        ),
        Ok(DeliveryOutcome::CopiedFallback(reason)) => format!("copied, not typed: {reason}"),
        Err(error) => format!("not copied: {error}"),
    };
    let app = target.map_or_else(
        || "no window".to_string(),
        |raw| paste::Target::of(raw).to_string(),
    );
    log::note(&format!(
        "Delivery: {result}. App: {app}. Latency: {latency}. Take {:.1} s, last part {:.1} s, {} characters ({steps}).",
        take.duration_secs,
        take.last_part_secs,
        take.text.chars().count()
    ));
}

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

    let _instance = if preview::is_active() {
        None
    } else {
        Some(match claim_instance() {
            InstanceClaim::AlreadyRunning => return,
            InstanceClaim::Owned(guard) => guard,
            InstanceClaim::Failed(error) => {
                log::note(&error);
                show_error(&error);
                return;
            }
        })
    };
    if !preview::is_active() {
        // Task Scheduler calls are slow enough to delay the first window.
        std::thread::spawn(autostart::refresh_if_enabled);
        update::remove_downloaded_setups();
    }

    let mut config = if preview::is_active() {
        AppConfig::default()
    } else {
        AppConfig::load()
    };
    // A damaged settings file: say so on the pill, and write the defaults so
    // the next load is clean. `load` kept a copy of the old file.
    let config_error = config::take_reset_notice();
    if config_error.is_some() {
        if let Err(error) = config.save() {
            log::note(&format!("Could not save the default settings: {error}"));
        }
    }
    // A retired model id moves to the model that replaces it, also in the file.
    let resolved_id = models::resolve(&config.model_id).id.to_string();
    if !preview::is_active() && resolved_id != config.model_id {
        config.model_id = resolved_id;
        if let Err(error) = config.save() {
            log::note(&format!("Could not save the model id: {error}"));
        }
    } else {
        config.model_id = resolved_id;
    }
    let hotkey_binding = HotkeyBinding::parse(&config.hotkey).unwrap_or_else(|| {
        log::note(&format!(
            "Could not read the shortcut '{}' of the settings. TDT uses the default.",
            config.hotkey
        ));
        let binding = HotkeyBinding::default();
        config.hotkey = binding.display();
        if !preview::is_active() {
            if let Err(error) = config.save() {
                log::note(&format!("Could not save the default shortcut: {error}"));
            }
        }
        binding
    });
    set_binding(hotkey_binding);
    let settings = Settings::new(config.clone(), !preview::is_active());
    let hotkey_label = hotkey_binding.display();

    // Open the microphone before the window. Doing it on the GPUI executor
    // blocks the first frame, and the popup then stays without WS_VISIBLE.
    let (recorder, microphone_error) = if preview::is_active() {
        (None, None)
    } else {
        std::thread::spawn(prewarm);
        watch_default_input();
        match AudioRecorder::new() {
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
        }
    };
    let recorder = Rc::new(RefCell::new(recorder));

    let model_spec = models::resolve(&config.model_id);
    let stt_engine = EngineSlot::default();
    stt_engine.set(
        if let Some(dir) = models::find_dir(model_spec, config.model_dir.as_deref()) {
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

    // Load and warm the selected model while the app opens, so a short first
    // dictation does not pay the full initialization cost after release.
    // prepare() serializes with recording and retries on failure.
    if !preview::is_active() {
        if let Some(engine) = stt_engine.get() {
            std::thread::spawn(move || {
                if let Err(error) = engine.prepare() {
                    log::note(&format!("Could not prepare the speech model: {error}"));
                }
            });
        }
    }

    let injector = Arc::new(PasteInjector::new());

    // A failed install must not look like a working shortcut.
    let _hotkey_guard;
    let hotkey_rx;
    let hotkey_error;
    if preview::is_active() {
        _hotkey_guard = None;
        hotkey_rx = None;
        hotkey_error = None;
    } else {
        match HotkeyListener::start() {
            Ok((guard, rx)) => {
                _hotkey_guard = Some(guard);
                hotkey_rx = Some(rx);
                // The hook installs fine at low integrity but never sees keys
                // typed into normal apps, so say why instead of failing silently.
                hotkey_error = low_rights_message();
                if let Some(message) = &hotkey_error {
                    log::note(message);
                }
            }
            Err(error) => {
                log::note(&format!("Hotkey hook failed: {error}"));
                _hotkey_guard = None;
                hotkey_rx = None;
                hotkey_error = Some(error);
            }
        }
    };

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
        let mut tray = if preview::is_active() && !preview::wants_tray() { None } else { match SystemTray::new(settings.get(|config| config.auto_paste), &hotkey_label) {
            Ok(tray) => Some(tray),
            Err(error) => {
                log::note(&format!("Failed to create system tray: {error}"));
                None
            }
        } };

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
        let settings_boot = settings.clone();
        let update_state = Arc::new(Mutex::new(UpdatePhase::Idle));
        let update_for_view = Arc::clone(&update_state);
        let update_for_boot = Arc::clone(&update_state);
        let update_for_loop = Arc::clone(&update_state);

        // The window that had the focus when the take started gets its text.
        let target_hwnd_state = Arc::new(Mutex::new(None::<isize>));
        let target_hwnd_loop = Arc::clone(&target_hwnd_state);

        let (internal_tx, internal_rx) = crossbeam_channel::unbounded::<InternalEvent>();

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
                let rec = Rc::clone(&recorder_clone);
                let inj = Arc::clone(&injector_clone);
                let stt = stt_clone.clone();
                let settings = settings_loop.clone();
                let target_hwnd = Arc::clone(&target_hwnd_loop);
                let update_phase = Arc::clone(&update_for_loop);

                cx.spawn(async move |this, cx| {
                    let mut is_recording_state = false;
                    let mut is_processing_state = false;
                    let mut recording_before_press = false;
                    let mut limit_notice = false;
                    // A microphone that failed during the take, for the
                    // notice after its text.
                    let mut microphone_lost: Option<String> = None;
                    let mut session: Option<LiveTake> = None;
                    // The view has nothing that changes with time.
                    let mut at_rest = false;
                    // A stream that gave no audio: the take starts again one
                    // time, with the microphone opened again.
                    let mut restart_take = false;
                    let mut reopened_in_take = false;
                    let mut late_start = false;

                    loop {
                        let poll_interval = if is_recording_state || is_processing_state {
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
                            let opened = (!preview::is_active() && !is_recording_state && !is_processing_state)
                                .then(AudioRecorder::new);
                            let _ = this.update(cx, |view, cx| {
                                match opened {
                                    Some(Ok(recorder)) => {
                                        log::note(&format!("Microphone: {}", recorder.describe()));
                                        *rec.borrow_mut() = Some(recorder);
                                        view.microphone_ready();
                                    }
                                    Some(Err(error)) => view.set_error(format!("Could not open the microphone. Check Windows microphone access and try again. {error}")),
                                    None => {}
                                }
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
                            if let Some(live) = session.take() {
                                live.discard();
                            }
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

                        let mut actions: Vec<_> = woke
                            .into_iter()
                            .chain(std::iter::from_fn(|| hotkey_rx.as_ref()?.try_recv().ok()))
                            .collect();
                        // A click on the resting pill is a tap of the shortcut.
                        let clicked = this
                            .update(cx, |view, _| view.take_click())
                            .unwrap_or(false);
                        if clicked && !is_recording_state && !is_processing_state {
                            // The hook clears this when the shortcut goes down. A
                            // click must clear it too, or a key typed before the
                            // click stops the paste.
                            reset_user_typed();
                            actions.push(HotkeyAction::PressStarted);
                            actions.push(HotkeyAction::Toggled(Instant::now()));
                        }
                        let mut stop_now = false;
                        let stream_error = rec.borrow().as_ref().and_then(|recorder| {
                            recorder.take_error().or_else(|| {
                                (is_recording_state && recorder.stalled()).then(|| {
                                    "The microphone stopped during the recording. Check the input device and try again.".to_string()
                                })
                            })
                        });
                        if let Some(error) = stream_error {
                            let silent = rec
                                .borrow()
                                .as_ref()
                                .is_some_and(|recorder| recorder.captured_samples() == 0);
                            let held = session.as_ref().is_some_and(LiveTake::is_held);
                            // The next press opens the microphone again.
                            *rec.borrow_mut() = None;
                            if is_recording_state && silent && held && !reopened_in_take {
                                // The stream gave no audio in this take, as it
                                // can after the PC wakes. The user still talks,
                                // so open the microphone again in this press.
                                log::note(&format!(
                                    "The microphone gave no audio. TDT opens it again in the same take. {error}"
                                ));
                                if let Some(live) = session.take() {
                                    live.discard();
                                }
                                is_recording_state = false;
                                reopened_in_take = true;
                                restart_take = true;
                                late_start = true;
                                actions.insert(0, HotkeyAction::PressStarted);
                            } else if is_recording_state || is_processing_state {
                                // The audio before the failure is already with
                                // the engine: finish the take, then say why it
                                // ended.
                                stop_now = is_recording_state;
                                microphone_lost = Some(error);
                            } else {
                                let _ = this.update(cx, |view, cx| { view.set_error(error); cx.notify(); });
                            }
                        }
                        let hit_limit = is_recording_state
                            && session.as_ref().is_some_and(LiveTake::hit_limit);
                        let at_limit = is_recording_state
                            && (hit_limit
                                || rec.borrow().as_ref().is_some_and(AudioRecorder::limit_reached));
                        let stop_requested = if is_recording_state {
                            this.update(cx, |view, _| view.take_stop_request()).unwrap_or(false)
                        } else {
                            false
                        };
                        if is_recording_state && (at_limit || stop_requested) {
                            if stop_requested {
                                // As a stop press of the shortcut does: only keys
                                // typed after the stop keep the text off the app.
                                reset_user_typed();
                            }
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
                                    let saved = settings.update(|config| config.hotkey = label.clone());
                                    if let Some(ref tray) = tray {
                                        let _ = tray
                                            .tray_icon
                                            .set_tooltip(Some(tooltip_text(&label)));
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
                                HotkeyAction::PressStarted => {
                                    // Snapshot the before-press state; it is
                                    // cleared on every release below, so a
                                    // swallowed press can never stop a session
                                    // it did not start.
                                    recording_before_press = is_recording_state;
                                    // A press during a take can only be the stop
                                    // of a take that a tap started. The stop
                                    // starts when the key goes down. At the
                                    // key-up, the click of the key is in the
                                    // audio, and the microphone stays open until
                                    // that sound ends (about 300 ms).
                                    if is_recording_state && !stop_now {
                                        stop_now = LiveTake::begin_stop(
                                            &mut session,
                                            Instant::now(),
                                            Some(current_binding().vk),
                                        );
                                    }
                                    // Do not overlap recordings with an in-flight
                                    // transcription. The current recorder and STT
                                    // engine are single-session by design.
                                    let pump_busy = session.as_ref().is_some_and(|live| !live.is_done());
                                    if !is_recording_state && !is_processing_state && !pump_busy {
                                        let restarted = std::mem::take(&mut restart_take);
                                        if !restarted {
                                            reopened_in_take = false;
                                            late_start = false;
                                        }
                                        if stt.get().is_none() {
                                            let _ = this.update(cx, |view, cx| {
                                                view.set_error("Download a speech model in Settings before recording.".into());
                                                view.open_settings(cx);
                                                cx.notify();
                                            });
                                            continue;
                                        }
                                        // A stream that does not start again is opened again too.
                                        let reopen = rec.borrow().as_ref().and_then(|recorder| {
                                            recorder
                                                .reopen_reason()
                                                .or_else(|| recorder.resume_input().err())
                                        });
                                        if rec.borrow().is_none() || reopen.is_some() {
                                            *rec.borrow_mut() = None;
                                            match AudioRecorder::new() {
                                                Ok(recorder) => {
                                                    let reason = reopen.map(|reason| format!(", opened again: {reason}")).unwrap_or_default();
                                                    log::note(&format!(
                                                        "Microphone: {}{reason}",
                                                        recorder.describe()
                                                    ));
                                                    *rec.borrow_mut() = Some(recorder);
                                                }
                                                Err(error) => {
                                                    let _ = this.update(cx, |view, cx| { view.set_error(format!("Could not open the microphone. Connect a microphone and retry. {error}")); cx.notify(); });
                                                    continue;
                                                }
                                            }
                                        }
                                        // Capture first: the buffer holds the audio until the pump below drains it.
                                        if let Some(recorder) = rec.borrow().as_ref() { recorder.start(); }
                                        *target_hwnd.lock() = ui::window_util::dictation_target_window();
                                        // The pill shows the take on the screen where the text goes.
                                        ui::window_util::move_pill_to_screen_of(*target_hwnd.lock());
                                        let _ = this.update(cx, |view, cx| view.close_stats_settings(cx));
                                        let Some(handle) = rec.borrow().as_ref().map(|recorder| recorder.handle()) else {
                                            continue;
                                        };
                                        limit_notice = false;
                                        let events = internal_tx.clone();
                                        let inj_worker = Arc::clone(&inj);
                                        let settings_worker = settings.clone();
                                        let target_flag = Arc::clone(&target_hwnd);
                                        session = Some(LiveTake::start(handle, stt.get(), move |result| match result {
                                            Ok(result) => {
                                                let target = *target_flag.lock();
                                                deliver_transcript(result, inj_worker, events, &settings_worker, target, user_typed());
                                            }
                                            // The engine's errors are written for the user.
                                            Err(error) => {
                                                let _ = events.send(InternalEvent::TranscribeError(error));
                                            }
                                        }));
                                        if !restarted {
                                            play_sound(SoundEffect::StartListening);
                                        }
                                        is_recording_state = true;
                                        let wave = rec.borrow().as_ref().map(AudioRecorder::live_wave);
                                        let _ = this.update(cx, |view, cx| {
                                            view.show_listening(wave);
                                            cx.notify();
                                        });
                                    }
                                }
                                HotkeyAction::Wake => {}
                                HotkeyAction::HoldReleased(asked) | HotkeyAction::Toggled(asked) => {
                                    // A hold always stops; a tap only stops a session it
                                    // started, otherwise it has just started one.
                                    let should_stop = if matches!(action, HotkeyAction::Toggled(_))
                                    {
                                        let stop = recording_before_press;
                                        recording_before_press = false;
                                        stop
                                    } else {
                                        recording_before_press = false;
                                        true
                                    };
                                    if should_stop && is_recording_state && !stop_now {
                                        stop_now = LiveTake::begin_stop(&mut session, asked, None);
                                    }
                                }
                            }
                        }

                        if is_recording_state && session.as_ref().is_some_and(LiveTake::must_stop_now) {
                            stop_now = true;
                        }
                        if is_recording_state && session.as_ref().is_some_and(LiveTake::is_done) {
                            if session.as_ref().is_some_and(LiveTake::hit_limit) {
                                limit_notice = true;
                            }
                            stop_now = true;
                        }
                        if stop_now && is_recording_state {
                            if let Some(live) = session.as_mut() {
                                live.halt();
                            }
                            is_recording_state = false;
                            is_processing_state = true;
                            play_sound(SoundEffect::StopListening);
                            let _ = this.update(cx, |view, cx| {
                                view.show_transcribing(limit_notice);
                                cx.notify();
                            });
                        }

                        while let Ok(event) = internal_rx.try_recv() {
                            match event {
                                InternalEvent::TranscribeSuccess { text, auto_pasted, notice, latency_ms } => {
                                    is_processing_state = false;
                                    play_sound(SoundEffect::Success);
                                    let notice = notice.or(microphone_lost.take()).or_else(|| {
                                        std::mem::take(&mut late_start).then(|| {
                                            "The microphone started late, so the first words can be missing.".to_string()
                                        })
                                    });
                                    let _ = this.update(cx, |view, cx| {
                                        view.show_success(text, auto_pasted, latency_ms, notice);
                                        cx.notify();
                                    });
                                }
                                InternalEvent::TranscribeError(err) => {
                                    is_processing_state = false;
                                    microphone_lost = None;
                                    late_start = false;
                                    if is_recording_state {
                                        if let Some(live) = session.as_mut() {
                                            live.halt();
                                        }
                                        is_recording_state = false;
                                    }
                                    play_sound(SoundEffect::Error);
                                    let _ = this.update(cx, |view, cx| {
                                        view.show_take_error(err);
                                        cx.notify();
                                    });
                                }
                                InternalEvent::NoSpeech => {
                                    is_processing_state = false;
                                    late_start = false;
                                    if let Some(error) = microphone_lost.take() {
                                        let _ = this.update(cx, |view, cx| { view.set_error(error); cx.notify(); });
                                        continue;
                                    }
                                    let _ = this.update(cx, |view, cx| {
                                        view.show_no_speech();
                                        cx.notify();
                                    });
                                }

                            }
                        }

                        // Fully idle means no recording and no transcription:
                        // park the microphone until the next press.
                        if !is_recording_state && !is_processing_state {
                            if let Some(active) = rec.borrow().as_ref() {
                                active.pause_input();
                            }
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

                // Check soon after start, then every hour, like Prism.
                if !preview::is_active() {
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(8));
                        loop {
                            let auto_checks = settings_boot.get(|config| config.check_updates);
                            update::check(&update_for_boot, CheckKind::Automatic, auto_checks);
                            std::thread::sleep(Duration::from_secs(60 * 60));
                        }
                    });
                }

                let mut view = HudView::new(
                    hotkey_binding,
                    stt_for_view,
                    settings_loop.clone(),
                    update_for_view,
                    cx,
                );
                for error in [config_error, microphone_error, hotkey_error]
                    .into_iter()
                    .flatten()
                {
                    view.show_startup_error(error);
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
