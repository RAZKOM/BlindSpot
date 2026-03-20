use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::boxes::RedactBox;
use crate::tracker::{RectPx, WindowHandle};

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum RedactionStyle {
    Solid,
    AnimatedNoise,
    CustomImage,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ImageFillMode {
    Tile,
    Stretch,
    Center,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HotkeyConfig {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub key: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            ctrl: true,
            alt: false,
            shift: false,
            win: true,
            key: "KeyR".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AppConfig {
    pub first_run: bool,
    pub run_on_startup: bool,
    #[serde(default = "default_true")]
    pub show_settings_on_startup: bool,
    pub redaction_style: RedactionStyle,
    pub redaction_color: [u8; 4],
    pub custom_image_path: Option<String>,
    #[serde(default = "default_image_fill_mode")]
    pub image_fill_mode: ImageFillMode,
    #[serde(default)]
    pub hotkey_toggle: HotkeyConfig,
    #[serde(default = "default_clear_hotkey")]
    pub hotkey_clear: HotkeyConfig,
}

fn default_true() -> bool { true }
fn default_image_fill_mode() -> ImageFillMode { ImageFillMode::Tile }

fn default_clear_hotkey() -> HotkeyConfig {
    HotkeyConfig {
        ctrl: true,
        alt: false,
        shift: true,
        win: true,
        key: "KeyR".to_string(),
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            first_run: true,
            run_on_startup: false,
            show_settings_on_startup: true,
            redaction_style: RedactionStyle::Solid,
            redaction_color: [0, 0, 0, 255],
            custom_image_path: None,
            image_fill_mode: ImageFillMode::Tile,
            hotkey_toggle: HotkeyConfig::default(),
            hotkey_clear: default_clear_hotkey(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TrackedWindow {
    pub rect: RectPx,
    pub minimized: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MonitorInfo {
    pub rect: RectPx,
}

#[derive(Clone, Debug)]
pub enum AppMode {
    Idle,
    WindowSelect,
    DrawTarget(WindowHandle),
}

#[derive(Clone, Debug)]
pub struct AppState {
    pub config: AppConfig,
    pub boxes: HashMap<WindowHandle, Vec<RedactBox>>,
    pub mode: AppMode,
    pub show_settings: bool,
    pub selected_window: Option<WindowHandle>,
    pub selected_box: Option<usize>,
    pub tracked_windows: HashMap<WindowHandle, TrackedWindow>,
    pub z_ordered_windows: Vec<WindowHandle>,
    pub monitor_infos: Vec<MonitorInfo>,
    pub hovered_window: Option<WindowHandle>,
    pub pending_new_box_drag: Option<(WindowHandle, (f32, f32))>,
    pub dirty_config: bool,
    pub redaction_repaint_needed: bool,
    pub redaction_overlays_enabled: bool,
}

impl AppState {
    pub fn new(config: AppConfig, first_run: bool) -> Self {
        let show_settings = first_run || config.first_run || config.show_settings_on_startup;
        Self {
            config,
            boxes: HashMap::new(),
            mode: AppMode::Idle,
            show_settings,
            selected_window: None,
            selected_box: None,
            tracked_windows: HashMap::new(),
            z_ordered_windows: Vec::new(),
            monitor_infos: Vec::new(),
            hovered_window: None,
            pending_new_box_drag: None,
            dirty_config: false,
            redaction_repaint_needed: false,
            redaction_overlays_enabled: true,
        }
    }

    pub fn boxes_for_window_mut(&mut self, hwnd: WindowHandle) -> &mut Vec<RedactBox> {
        self.boxes.entry(hwnd).or_insert_with(Vec::new)
    }
}