#![windows_subsystem = "windows"]

mod boxes;
mod persistence;
mod settings;
mod startup;
mod state;
mod tracker;
mod win32_overlay;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::persistence::{load_config, save_config};
use crate::settings::show_settings_window;
use crate::state::{AppMode, AppState, HotkeyConfig, MonitorInfo, TrackedWindow};

#[derive(Debug, Clone)]
enum AppSignal {
    ToggleDrawMode,
    ClearAll,
    ShowSettings,
    Quit,
}

struct BlindSpotApp {
    state: Arc<Mutex<AppState>>,
    rx: mpsc::Receiver<AppSignal>,
    quit_flag: Arc<AtomicBool>,
    _tray_icon: TrayIcon,
    hotkey_manager: GlobalHotKeyManager,
    registered_toggle: Option<HotKey>,
    registered_clear: Option<HotKey>,
    clear_hotkey_id: Arc<AtomicU32>,
    startup_last_error: Option<String>,
    last_frame: Instant,
}

impl BlindSpotApp {
    fn new(
        state: Arc<Mutex<AppState>>,
        rx: mpsc::Receiver<AppSignal>,
        quit_flag: Arc<AtomicBool>,
        tray_icon: TrayIcon,
        hotkey_manager: GlobalHotKeyManager,
        registered_toggle: Option<HotKey>,
        registered_clear: Option<HotKey>,
        clear_hotkey_id: Arc<AtomicU32>,
    ) -> Self {
        Self {
            state,
            rx,
            quit_flag,
            _tray_icon: tray_icon,
            hotkey_manager,
            registered_toggle,
            registered_clear,
            clear_hotkey_id,
            startup_last_error: None,
            last_frame: Instant::now(),
        }
    }

    fn drain_signals(&mut self) -> bool {
        let mut should_quit = false;
        while let Ok(sig) = self.rx.try_recv() {
            let mut state = self.state.lock().expect("state lock poisoned");
            match sig {
                AppSignal::ToggleDrawMode => match state.mode {
                    AppMode::Idle => state.mode = AppMode::WindowSelect,
                    _ => {
                        state.mode = AppMode::Idle;
                        state.selected_window = None;
                        state.selected_box = None;
                        state.pending_new_box_drag = None;
                        state.redaction_overlays_enabled = true;
                    }
                },
                AppSignal::ClearAll => {
                    if let Some(hwnd) = state.selected_window {
                        if let Some(boxes) = state.boxes.get_mut(&hwnd) {
                            boxes.clear();
                        }
                        state.selected_box = None;
                    } else {
                        state.boxes.clear();
                        state.selected_box = None;
                    }
                }
                AppSignal::ShowSettings => state.show_settings = true,
                AppSignal::Quit => {
                    should_quit = true;
                }
            }
        }
        should_quit
    }

    fn reregister_hotkeys(&mut self) {
        if let Some(hk) = self.registered_toggle.take() {
            let _ = self.hotkey_manager.unregister(hk);
        }
        if let Some(hk) = self.registered_clear.take() {
            let _ = self.hotkey_manager.unregister(hk);
        }

        let config = {
            let s = self.state.lock().expect("lock");
            s.config.clone()
        };

        if let Ok(hk) = hotkey_config_to_hotkey(&config.hotkey_toggle) {
            if self.hotkey_manager.register(hk).is_ok() {
                self.registered_toggle = Some(hk);
            }
        }

        if let Ok(hk) = hotkey_config_to_hotkey(&config.hotkey_clear) {
            self.clear_hotkey_id.store(hk.id(), Ordering::Relaxed);
            if self.hotkey_manager.register(hk).is_ok() {
                self.registered_clear = Some(hk);
            }
        }
    }
}

impl eframe::App for BlindSpotApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.drain_signals() {
            self.quit_flag.store(true, Ordering::Relaxed);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        let now = Instant::now();
        let elapsed = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |_ui| {});

        let state_for_settings = self.state.clone();
        let state_for_save = self.state.clone();
        let show_settings = {
            let s = state_for_settings.lock().expect("state lock poisoned");
            s.show_settings
        };

        if show_settings {
            let settings_id = egui::ViewportId::from_hash_of("settings_window");

            let icon = {
                static ICON_BYTES: &[u8] = include_bytes!("../assets/blindspot.png");
                let img = image::load_from_memory(ICON_BYTES).ok().map(|i| i.into_rgba8());
                img.map(|rgba| {
                    let w = rgba.width();
                    let h = rgba.height();
                    egui::IconData { rgba: rgba.into_raw(), width: w, height: h }
                })
            };

            let mut settings_builder = egui::ViewportBuilder::default()
                .with_title("BlindSpot")
                .with_inner_size([440.0, 640.0])
                .with_resizable(false);

            if let Some(icon_data) = icon {
                settings_builder = settings_builder.with_icon(std::sync::Arc::new(icon_data));
            }

            let startup_error = &mut self.startup_last_error;
            ctx.show_viewport_immediate(settings_id, settings_builder, move |ctx, _| {
                let mut state = state_for_save.lock().expect("state lock poisoned");
                if ctx.input(|i| i.viewport().close_requested()) {
                    state.show_settings = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }

                let output = show_settings_window(ctx, &mut state.config);
                if output.changed {
                    state.config.first_run = false;
                    state.dirty_config = true;
                    state.redaction_repaint_needed = true;
                    match startup::set_run_on_startup(state.config.run_on_startup) {
                        Ok(_) => *startup_error = None,
                        Err(e) => *startup_error = Some(e.to_string()),
                    }
                }
            });
        }

        {
            let state = self.state.lock().expect("state lock poisoned");
            if state.dirty_config {
                drop(state);
                self.reregister_hotkeys();
            }
        }

        let _ = (now, elapsed);
        {
            let mut state = self.state.lock().expect("state lock poisoned");
            if state.dirty_config {
                let _ = save_config(&state.config);
                state.dirty_config = false;
            }
        }

        if show_settings {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
}

fn main() -> Result<(), String> {
    let _single_instance = ensure_single_instance()?;

    tracker::set_dpi_awareness_per_monitor_v2();

    let (config, first_run_file_missing) = load_config().map_err(|e| e.to_string())?;
    let mut state = AppState::new(config, first_run_file_missing);
    if let Ok(run) = startup::is_run_on_startup_enabled() {
        state.config.run_on_startup = run;
        if run {
            let _ = startup::refresh_startup_path_if_enabled();
        }
    }

    let shared_state = Arc::new(Mutex::new(state));
    let quit_flag = Arc::new(AtomicBool::new(false));
    start_tracker_thread(shared_state.clone(), quit_flag.clone());
    win32_overlay::spawn_overlay_thread(shared_state.clone());

    let (tx, rx) = mpsc::channel::<AppSignal>();
    let (hotkey_manager, registered_toggle, registered_clear, clear_id) = {
        let s = shared_state.lock().expect("lock");
        setup_hotkeys(&s.config.hotkey_toggle, &s.config.hotkey_clear, tx.clone())?
    };
    let tray_icon = setup_tray(tx.clone())?;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_visible(false)
            .with_taskbar(false)
            .with_decorations(false)
            .with_position(egui::pos2(-1000.0, -1000.0))
            .with_inner_size([1.0, 1.0]),
        ..Default::default()
    };

    let app_state = shared_state.clone();
    let app_quit = quit_flag.clone();
    eframe::run_native(
        "BlindSpot",
        options,
        Box::new(move |_cc| {
            Box::new(BlindSpotApp::new(
                app_state.clone(),
                rx,
                app_quit.clone(),
                tray_icon,
                hotkey_manager,
                registered_toggle,
                registered_clear,
                clear_id,
            ))
        }),
    )
    .map_err(|e| e.to_string())
}

fn setup_hotkeys(
    toggle_cfg: &HotkeyConfig,
    clear_cfg: &HotkeyConfig,
    tx: mpsc::Sender<AppSignal>,
) -> Result<(GlobalHotKeyManager, Option<HotKey>, Option<HotKey>, Arc<AtomicU32>), String> {
    let hotkey_manager = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;

    let toggle_hk = hotkey_config_to_hotkey(toggle_cfg).map_err(|e| e.to_string())?;
    let clear_hk = hotkey_config_to_hotkey(clear_cfg).map_err(|e| e.to_string())?;

    let registered_toggle = if hotkey_manager.register(toggle_hk).is_ok() {
        Some(toggle_hk)
    } else {
        None
    };

    let clear_id = Arc::new(AtomicU32::new(clear_hk.id()));
    let registered_clear = if hotkey_manager.register(clear_hk).is_ok() {
        Some(clear_hk)
    } else {
        None
    };

    let clear_id_thread = clear_id.clone();
    thread::spawn(move || loop {
        if let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
            if event.state == HotKeyState::Pressed {
                if event.id == clear_id_thread.load(Ordering::Relaxed) {
                    let _ = tx.send(AppSignal::ClearAll);
                } else {
                    let _ = tx.send(AppSignal::ToggleDrawMode);
                }
            }
        }
    });

    Ok((hotkey_manager, registered_toggle, registered_clear, clear_id))
}

fn hotkey_config_to_hotkey(cfg: &HotkeyConfig) -> Result<HotKey, String> {
    let mut mods = Modifiers::empty();
    if cfg.ctrl { mods |= Modifiers::CONTROL; }
    if cfg.alt { mods |= Modifiers::ALT; }
    if cfg.shift { mods |= Modifiers::SHIFT; }
    if cfg.win { mods |= Modifiers::SUPER; }
    let code = string_to_code(&cfg.key)
        .ok_or_else(|| format!("Unknown key: {}", cfg.key))?;
    let mods_opt = if mods.is_empty() { None } else { Some(mods) };
    Ok(HotKey::new(mods_opt, code))
}

fn string_to_code(s: &str) -> Option<Code> {
    match s {
        "KeyA" => Some(Code::KeyA),
        "KeyB" => Some(Code::KeyB),
        "KeyC" => Some(Code::KeyC),
        "KeyD" => Some(Code::KeyD),
        "KeyE" => Some(Code::KeyE),
        "KeyF" => Some(Code::KeyF),
        "KeyG" => Some(Code::KeyG),
        "KeyH" => Some(Code::KeyH),
        "KeyI" => Some(Code::KeyI),
        "KeyJ" => Some(Code::KeyJ),
        "KeyK" => Some(Code::KeyK),
        "KeyL" => Some(Code::KeyL),
        "KeyM" => Some(Code::KeyM),
        "KeyN" => Some(Code::KeyN),
        "KeyO" => Some(Code::KeyO),
        "KeyP" => Some(Code::KeyP),
        "KeyQ" => Some(Code::KeyQ),
        "KeyR" => Some(Code::KeyR),
        "KeyS" => Some(Code::KeyS),
        "KeyT" => Some(Code::KeyT),
        "KeyU" => Some(Code::KeyU),
        "KeyV" => Some(Code::KeyV),
        "KeyW" => Some(Code::KeyW),
        "KeyX" => Some(Code::KeyX),
        "KeyY" => Some(Code::KeyY),
        "KeyZ" => Some(Code::KeyZ),
        "F1" => Some(Code::F1),
        "F2" => Some(Code::F2),
        "F3" => Some(Code::F3),
        "F4" => Some(Code::F4),
        "F5" => Some(Code::F5),
        "F6" => Some(Code::F6),
        "F7" => Some(Code::F7),
        "F8" => Some(Code::F8),
        "F9" => Some(Code::F9),
        "F10" => Some(Code::F10),
        "F11" => Some(Code::F11),
        "F12" => Some(Code::F12),
        "Digit0" => Some(Code::Digit0),
        "Digit1" => Some(Code::Digit1),
        "Digit2" => Some(Code::Digit2),
        "Digit3" => Some(Code::Digit3),
        "Digit4" => Some(Code::Digit4),
        "Digit5" => Some(Code::Digit5),
        "Digit6" => Some(Code::Digit6),
        "Digit7" => Some(Code::Digit7),
        "Digit8" => Some(Code::Digit8),
        "Digit9" => Some(Code::Digit9),
        _ => None,
    }
}

fn setup_tray(tx: mpsc::Sender<AppSignal>) -> Result<TrayIcon, String> {
    let menu = Menu::new();
    let settings = MenuItem::new("Settings", true, None);
    let draw_mode = MenuItem::new("Draw Mode", true, None);
    let clear_all = MenuItem::new("Clear All Redactions", true, None);
    let quit = MenuItem::new("Quit", true, None);

    menu.append(&settings).map_err(|e| e.to_string())?;
    menu.append(&draw_mode).map_err(|e| e.to_string())?;
    menu.append(&clear_all).map_err(|e| e.to_string())?;
    menu.append(&quit).map_err(|e| e.to_string())?;

    let settings_id = settings.id().clone();
    let draw_id = draw_mode.id().clone();
    let clear_id = clear_all.id().clone();
    let quit_id = quit.id().clone();
    spawn_tray_listener(tx, settings_id, draw_id, clear_id, quit_id);

    let icon = {
        static ICON_BYTES: &[u8] = include_bytes!("../assets/blindspot.png");
        let img = image::load_from_memory(ICON_BYTES)
            .map_err(|e| format!("Failed to decode icon: {}", e))?
            .into_rgba8();
        Icon::from_rgba(img.clone().into_raw(), img.width(), img.height())
            .map_err(|e| format!("Failed to create tray icon: {}", e))?
    };
    TrayIconBuilder::new()
        .with_tooltip("BlindSpot")
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .build()
        .map_err(|e| e.to_string())
}

fn spawn_tray_listener(
    tx: mpsc::Sender<AppSignal>,
    settings_id: MenuId,
    draw_id: MenuId,
    clear_id: MenuId,
    quit_id: MenuId,
) {
    thread::spawn(move || loop {
        let Ok(event) = MenuEvent::receiver().recv() else {
            break;
        };
        if event.id == settings_id {
            let _ = tx.send(AppSignal::ShowSettings);
        } else if event.id == draw_id {
            let _ = tx.send(AppSignal::ToggleDrawMode);
        } else if event.id == clear_id {
            let _ = tx.send(AppSignal::ClearAll);
        } else if event.id == quit_id {
            let _ = tx.send(AppSignal::Quit);
            break;
        }
    });
}

fn start_tracker_thread(state: Arc<Mutex<AppState>>, quit_flag: Arc<AtomicBool>) {
    thread::spawn(move || {
        while !quit_flag.load(Ordering::Relaxed) {
            let windows = tracker::enumerate_windows();
            let monitors = tracker::enumerate_monitors();
            let is_idle;
            {
                let mut app = state.lock().expect("state lock poisoned");
                is_idle = matches!(app.mode, AppMode::Idle);
                app.monitor_infos = monitors
                    .into_iter()
                    .map(|m| MonitorInfo {
                        rect: m.rect,
                    })
                    .collect();
                app.tracked_windows.clear();
                app.z_ordered_windows.clear();
                for w in windows {
                    if w.title.starts_with("BlindSpot.") {
                        continue;
                    }
                    app.z_ordered_windows.push(w.hwnd);
                    app.tracked_windows.insert(
                        w.hwnd,
                        TrackedWindow {
                            rect: w.rect,
                            minimized: w.minimized,
                        },
                    );
                }
            }
            if is_idle {
                thread::sleep(Duration::from_millis(500));
            } else {
                thread::sleep(Duration::from_millis(32));
            }
        }
    });
}

struct SingleInstanceGuard(windows::Win32::Foundation::HANDLE);

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

fn ensure_single_instance() -> Result<SingleInstanceGuard, String> {
    use windows::core::PCSTR;
    use windows::Win32::System::Threading::CreateMutexA;
    use windows::Win32::Foundation::GetLastError;

    let name = b"Global\\BlindSpot_SingleInstance\0";
    let handle = unsafe {
        CreateMutexA(None, false, PCSTR(name.as_ptr()))
            .map_err(|e| format!("Failed to create mutex: {}", e))?
    };

    let last_error = unsafe { GetLastError() };
    if last_error.0 == 183 {
        unsafe { let _ = windows::Win32::Foundation::CloseHandle(handle); }
        return Err("BlindSpot is already running.".to_string());
    }

    Ok(SingleInstanceGuard(handle))
}