use egui::Color32;

use crate::state::{AppConfig, HotkeyConfig, ImageFillMode, RedactionStyle};

const AVAILABLE_KEYS: &[&str] = &[
    "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ",
    "KeyK", "KeyL", "KeyM", "KeyN", "KeyO", "KeyP", "KeyQ", "KeyR", "KeyS", "KeyT",
    "KeyU", "KeyV", "KeyW", "KeyX", "KeyY", "KeyZ",
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
    "Digit0", "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7",
    "Digit8", "Digit9",
];

pub struct SettingsOutput {
    pub changed: bool,
}

pub fn show_settings_window(ctx: &egui::Context, config: &mut AppConfig) -> SettingsOutput {
    let before = config.clone();

    egui::CentralPanel::default()
        .frame(egui::Frame::none().fill(Color32::from_rgb(24, 24, 30)).inner_margin(20.0))
        .show(ctx, |ui| {
        ui.style_mut().visuals.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(210, 210, 220);
        ui.style_mut().visuals.widgets.inactive.fg_stroke.color = Color32::from_rgb(190, 190, 200);
        ui.style_mut().visuals.widgets.hovered.fg_stroke.color = Color32::WHITE;
        ui.style_mut().visuals.widgets.active.fg_stroke.color = Color32::WHITE;
        ui.style_mut().visuals.selection.bg_fill = Color32::from_rgb(220, 40, 40);

        egui::ScrollArea::vertical().show(ui, |ui| {
            let header_height = 28.0 + 4.0 + 16.0;
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), header_height),
                egui::Sense::hover(),
            );
            let painter = ui.painter_at(rect);
            let cx = rect.center().x;
            painter.text(
                egui::pos2(cx, rect.top()),
                egui::Align2::CENTER_TOP,
                "BlindSpot",
                egui::FontId::proportional(24.0),
                egui::Color32::from_rgb(240, 240, 245),
            );
            painter.text(
                egui::pos2(cx, rect.top() + 28.0),
                egui::Align2::CENTER_TOP,
                "Persistent redaction overlays for your desktop",
                egui::FontId::proportional(12.0),
                egui::Color32::from_rgb(150, 150, 160),
            );
            ui.add_space(12.0);

            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 80.0) / 2.0);
                ui.colored_label(Color32::from_rgb(100, 100, 110), "v0.1.0");
            });

            ui.add_space(16.0);

            section_header(ui, "\u{2139} Quick Start");
            ui.add_space(4.0);
            styled_label(ui, "\u{2022} Press the toggle hotkey to enter draw mode");
            styled_label(ui, "\u{2022} Click a window, then drag to draw a redaction box");
            styled_label(ui, "\u{2022} Click a box to select \u{2014} drag to move, corners to resize");
            styled_label(ui, "\u{2022} Press Escape or hotkey again to exit draw mode");
            styled_label(ui, "\u{2022} Boxes persist across app restarts");
            ui.add_space(8.0);

            ui.separator();
            ui.add_space(8.0);

            section_header(ui, "\u{2699} General");
            ui.add_space(4.0);
            ui.checkbox(&mut config.run_on_startup, "  Run on startup");
            ui.checkbox(&mut config.show_settings_on_startup, "  Show this window on startup");

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            section_header(ui, "\u{2328} Hotkeys");
            ui.add_space(4.0);
            ui.label("Toggle draw mode:");
            show_hotkey_editor(ui, "toggle", &mut config.hotkey_toggle);
            ui.add_space(6.0);
            ui.label("Clear all redactions:");
            show_hotkey_editor(ui, "clear", &mut config.hotkey_clear);

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            section_header(ui, "\u{25A0} Redaction Style");
            ui.add_space(4.0);
            egui::ComboBox::from_id_source("redaction_style")
                .selected_text(style_label(&config.redaction_style))
                .width(200.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut config.redaction_style, RedactionStyle::Solid, "Solid color");
                    ui.selectable_value(&mut config.redaction_style, RedactionStyle::AnimatedNoise, "Animated noise/static");
                    ui.selectable_value(&mut config.redaction_style, RedactionStyle::CustomImage, "Custom image");
                });

            ui.add_space(4.0);

            if config.redaction_style == RedactionStyle::Solid {
                ui.horizontal(|ui| {
                    ui.label("Color:");
                    let mut rgba = [
                        config.redaction_color[0] as f32 / 255.0,
                        config.redaction_color[1] as f32 / 255.0,
                        config.redaction_color[2] as f32 / 255.0,
                        config.redaction_color[3] as f32 / 255.0,
                    ];
                    if ui.color_edit_button_rgba_unmultiplied(&mut rgba).changed() {
                        config.redaction_color = [
                            (rgba[0] * 255.0).clamp(0.0, 255.0) as u8,
                            (rgba[1] * 255.0).clamp(0.0, 255.0) as u8,
                            (rgba[2] * 255.0).clamp(0.0, 255.0) as u8,
                            (rgba[3] * 255.0).clamp(0.0, 255.0) as u8,
                        ];
                    }
                });
            }

            if config.redaction_style == RedactionStyle::CustomImage {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Choose image...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Image", &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tiff", "tga", "ico"])
                            .pick_file()
                        {
                            config.custom_image_path = Some(path.display().to_string());
                        }
                    }
                    if let Some(ref path) = config.custom_image_path {
                        let name = std::path::Path::new(path)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.clone());
                        ui.colored_label(Color32::from_rgb(150, 150, 160), name);
                    } else {
                        ui.colored_label(Color32::from_rgb(120, 120, 130), "No image selected");
                    }
                });

                ui.colored_label(Color32::from_rgb(100, 100, 110), "Supports PNG, JPG, BMP, GIF, WebP, TIFF, TGA, ICO");

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Fill mode:");
                    egui::ComboBox::from_id_source("image_fill_mode")
                        .selected_text(fill_mode_label(&config.image_fill_mode))
                        .width(160.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut config.image_fill_mode, ImageFillMode::Tile, "Tile (repeat)");
                            ui.selectable_value(&mut config.image_fill_mode, ImageFillMode::Stretch, "Stretch to fit");
                            ui.selectable_value(&mut config.image_fill_mode, ImageFillMode::Center, "Center (no scaling)");
                        });
                });
            }

            ui.add_space(16.0);

            ui.horizontal(|ui| {
                let text = "Settings save automatically \u{2022} Use tray menu to quit";
                let text_width = ui.fonts(|f| f.layout_no_wrap(text.to_string(), egui::FontId::proportional(11.0), Color32::GRAY).size().x);
                ui.add_space(((ui.available_width() - text_width) / 2.0).max(0.0));
                ui.colored_label(Color32::from_rgb(80, 80, 90), text);
            });

            ui.add_space(8.0);
        });
    });

    let changed = *config != before;
    SettingsOutput { changed }
}

fn section_header(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(15.0).color(Color32::from_rgb(220, 45, 45)).strong());
}

fn styled_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(12.0).color(Color32::from_rgb(170, 170, 180)));
}

fn show_hotkey_editor(ui: &mut egui::Ui, id_prefix: &str, hotkey: &mut HotkeyConfig) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut hotkey.ctrl, "Ctrl");
        ui.checkbox(&mut hotkey.alt, "Alt");
        ui.checkbox(&mut hotkey.shift, "Shift");
        ui.checkbox(&mut hotkey.win, "Win");
        ui.add_space(4.0);
        let display_key = key_display_name(&hotkey.key);
        egui::ComboBox::from_id_source(format!("{}_key", id_prefix))
            .selected_text(display_key)
            .width(70.0)
            .show_ui(ui, |ui| {
                for &k in AVAILABLE_KEYS {
                    let label = key_display_name(k);
                    ui.selectable_value(&mut hotkey.key, k.to_string(), label);
                }
            });
    });
}

fn key_display_name(key: &str) -> &str {
    if let Some(letter) = key.strip_prefix("Key") { return letter; }
    if let Some(digit) = key.strip_prefix("Digit") { return digit; }
    key
}

fn style_label(style: &RedactionStyle) -> &'static str {
    match style {
        RedactionStyle::Solid => "Solid color",
        RedactionStyle::AnimatedNoise => "Animated noise/static",
        RedactionStyle::CustomImage => "Custom image",
    }
}

fn fill_mode_label(mode: &ImageFillMode) -> &'static str {
    match mode {
        ImageFillMode::Tile => "Tile (repeat)",
        ImageFillMode::Stretch => "Stretch to fit",
        ImageFillMode::Center => "Center (no scaling)",
    }
}