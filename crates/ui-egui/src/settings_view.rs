//! Settings window. Changes are saved immediately; the tray daemon reloads shortcuts on change.

use std::str::FromStr;

use egui::{Key, Modifiers, RichText, vec2};
use egui_phosphor::regular as icon;
use global_hotkey::hotkey::HotKey;

use shotcraft_engine::config::{self, ExportFormat, QuickAccessSide, Settings, WindowCaptureStyle};

use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Shortcuts,
    QuickAccess,
    Screenshots,
    Cloud,
    Advanced,
    About,
}

const TABS: [(Tab, &str, &str, &str); 7] = [
    (Tab::General, "general", icon::GEAR, "General"),
    (Tab::Shortcuts, "shortcuts", icon::KEYBOARD, "Shortcuts"),
    (Tab::QuickAccess, "quickaccess", icon::IMAGE_SQUARE, "Quick Access"),
    (Tab::Screenshots, "screenshots", icon::CAMERA, "Screenshots"),
    (Tab::Cloud, "cloud", icon::CLOUD_ARROW_UP, "Upload"),
    (Tab::Advanced, "advanced", icon::SLIDERS_HORIZONTAL, "Advanced"),
    (Tab::About, "about", icon::INFO, "About"),
];

struct SettingsApp {
    settings: Settings,
    saved: Settings,
    tab: Tab,
    recording: Option<usize>,
    error: Option<String>,
    headers_text: String,
}

pub fn run(tab: Option<String>) -> anyhow::Result<()> {
    let tab = tab.and_then(|t| TABS.iter().find(|x| x.1 == t.to_ascii_lowercase()).map(|x| x.0)).unwrap_or(Tab::General);
    let builder = egui::ViewportBuilder::default().with_title("ShotCraft Settings").with_inner_size(vec2(760.0, 520.0)).with_resizable(false);
    let settings = Settings::load();
    theme::run("shotcraft-settings", builder, move |_| SettingsApp {
        headers_text: settings.upload.headers.join("\n"),
        saved: settings.clone(),
        settings,
        tab,
        recording: None,
        error: None,
    })
}

/// global-hotkey name for an egui key (`KeyA`, `Digit1`, `F5`, …).
fn key_code(key: Key) -> Option<String> {
    let name = key.name();
    if name.len() == 1 && name.chars().all(|c| c.is_ascii_uppercase()) {
        return Some(format!("Key{name}"));
    }
    if name.len() == 1 && name.chars().all(|c| c.is_ascii_digit()) {
        return Some(format!("Digit{name}"));
    }
    if name.starts_with('F') && name[1..].parse::<u8>().is_ok() {
        return Some(name.to_owned());
    }
    let code = match key {
        Key::Space => "Space",
        Key::Enter => "Enter",
        Key::Tab => "Tab",
        Key::ArrowUp => "ArrowUp",
        Key::ArrowDown => "ArrowDown",
        Key::ArrowLeft => "ArrowLeft",
        Key::ArrowRight => "ArrowRight",
        Key::Minus => "Minus",
        Key::Equals => "Equal",
        Key::Comma => "Comma",
        Key::Period => "Period",
        Key::Slash => "Slash",
        Key::Backslash => "Backslash",
        Key::Semicolon => "Semicolon",
        Key::Quote => "Quote",
        Key::Backtick => "Backquote",
        Key::OpenBracket => "BracketLeft",
        Key::CloseBracket => "BracketRight",
        Key::Insert => "Insert",
        Key::Delete => "Delete",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "PageUp",
        Key::PageDown => "PageDown",
        _ => return None,
    };
    Some(code.to_owned())
}

fn binding(mods: Modifiers, key: Key) -> Option<String> {
    let code = key_code(key)?;
    let mut parts = Vec::new();
    if mods.ctrl {
        parts.push("Control");
    }
    if mods.alt {
        parts.push("Alt");
    }
    if mods.shift {
        parts.push("Shift");
    }
    if mods.mac_cmd {
        parts.push("Super");
    }
    // A global shortcut without modifiers would swallow normal typing (F-keys excepted).
    if parts.is_empty() && !code.starts_with('F') {
        return None;
    }
    parts.push(&code);
    Some(parts.join("+"))
}

fn pretty(binding: &str) -> String {
    if binding.is_empty() {
        return "—".into();
    }
    binding.split('+').map(|p| p.trim_start_matches("Key").trim_start_matches("Digit")).collect::<Vec<_>>().join(" + ")
}

impl SettingsApp {
    fn general(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.heading("After capture");
        ui.checkbox(&mut s.show_quick_access, "Show Quick Access Overlay");
        ui.checkbox(&mut s.copy_after_capture, "Copy to clipboard");
        ui.checkbox(&mut s.save_after_capture, "Save to folder");
        ui.add_space(10.0);
        ui.heading("Saving");
        ui.horizontal(|ui| {
            ui.label("Folder:");
            ui.label(RichText::new(s.save_dir.display().to_string()).monospace());
            if ui.button("Change…").clicked()
                && let Some(dir) = rfd::FileDialog::new().set_directory(&s.save_dir).pick_folder()
            {
                s.save_dir = dir;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Format:");
            for f in ExportFormat::ALL {
                ui.selectable_value(&mut s.export_format, f, f.label());
            }
        });
        if s.export_format == ExportFormat::Jpeg {
            ui.add(egui::Slider::new(&mut s.jpeg_quality, 40..=100).text("JPEG quality"));
        }
    }

    fn shortcuts(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Click Record and press a key combination. Backspace clears, Esc cancels.").weak());
        if cfg!(target_os = "linux") {
            ui.label(
                RichText::new("Global shortcuts need X11. On Wayland, bind your desktop's shortcuts to commands such as `shotcraft capture-area`.").weak(),
            );
        }
        ui.add_space(6.0);
        if let Some(row) = self.recording {
            let captured = ui.ctx().input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
            });
            if let Some((key, mods)) = captured {
                match key {
                    Key::Escape => self.recording = None,
                    Key::Backspace => {
                        *self.settings.hotkeys.entries_mut()[row].2 = String::new();
                        self.recording = None;
                    }
                    _ => {
                        if let Some(b) = binding(mods, key) {
                            *self.settings.hotkeys.entries_mut()[row].2 = b;
                            self.recording = None;
                            self.error = None;
                        } else {
                            self.error = Some("Use at least one modifier (Ctrl, Alt, Shift, Super).".into());
                        }
                    }
                }
            }
        }
        let recording = self.recording;
        let mut start = None;
        let mut errors = Vec::new();
        egui::Grid::new("shortcuts").num_columns(3).spacing(vec2(16.0, 8.0)).striped(true).show(ui, |ui| {
            for (row, (label, _, key)) in self.settings.hotkeys.entries_mut().into_iter().enumerate() {
                ui.label(label);
                if recording == Some(row) {
                    ui.label(RichText::new("Press keys…").color(theme::ACCENT));
                } else {
                    ui.label(RichText::new(pretty(key)).monospace());
                    if !key.is_empty() && HotKey::from_str(key).is_err() {
                        errors.push(format!("{label}: invalid shortcut"));
                    }
                }
                ui.horizontal(|ui| {
                    if ui.button("Record").clicked() {
                        start = Some(row);
                    }
                    if !key.is_empty() && ui.button(icon::X).on_hover_text("Clear").clicked() {
                        key.clear();
                    }
                });
                ui.end_row();
            }
        });
        if start.is_some() {
            self.recording = start;
        }
        for e in errors.iter().chain(self.error.iter()) {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
    }

    fn quick_access(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.horizontal(|ui| {
            ui.label("Position:");
            ui.selectable_value(&mut s.quick_access_side, QuickAccessSide::Left, "Bottom left");
            ui.selectable_value(&mut s.quick_access_side, QuickAccessSide::Right, "Bottom right");
        });
        ui.checkbox(&mut s.close_quick_access_after_action, "Close after copying, saving or dragging");
        ui.horizontal(|ui| {
            let mut auto = s.quick_access_auto_close > 0;
            if ui.checkbox(&mut auto, "Close automatically after").changed() {
                s.quick_access_auto_close = if auto { 10 } else { 0 };
            }
            ui.add_enabled(auto, egui::DragValue::new(&mut s.quick_access_auto_close).range(1..=600).suffix(" s"));
        });
    }

    fn screenshots(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.checkbox(&mut s.show_crosshair, "Show crosshair");
        ui.checkbox(&mut s.show_magnifier, "Show magnifier");
        ui.add_space(8.0);
        ui.heading("Window capture");
        ui.radio_value(&mut s.window_style, WindowCaptureStyle::Shadow, "Transparent with shadow");
        ui.radio_value(&mut s.window_style, WindowCaptureStyle::Background, "On a background");
        ui.radio_value(&mut s.window_style, WindowCaptureStyle::Plain, "Window only");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Self-timer:");
            ui.add(egui::DragValue::new(&mut s.timer_seconds).range(1..=60).suffix(" s"));
        });
        ui.checkbox(&mut s.ocr_keep_linebreaks, "Keep line breaks in recognized text");
    }

    fn cloud(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Uploads send a multipart/form-data POST to your endpoint and copy the returned link.").weak());
        ui.add_space(6.0);
        let u = &mut self.settings.upload;
        egui::Grid::new("upload").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
            ui.label("Endpoint URL");
            ui.add(egui::TextEdit::singleline(&mut u.url).hint_text("https://example.com/upload").desired_width(420.0));
            ui.end_row();
            ui.label("File field");
            ui.add(egui::TextEdit::singleline(&mut u.file_field).desired_width(160.0));
            ui.end_row();
            ui.label("Link in response");
            ui.add(egui::TextEdit::singleline(&mut u.response_path).hint_text("e.g. data.link (empty = whole body)").desired_width(420.0));
            ui.end_row();
            ui.label("Headers");
            if ui.add(egui::TextEdit::multiline(&mut self.headers_text).hint_text("Authorization: Bearer …").desired_rows(3).desired_width(420.0)).changed() {
                u.headers = self.headers_text.lines().map(str::trim).filter(|l| l.contains(':')).map(str::to_owned).collect();
            }
            ui.end_row();
        });
    }

    fn advanced(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.horizontal(|ui| {
            ui.label("Keep capture history for");
            ui.add(egui::DragValue::new(&mut s.history_days).range(1..=365).suffix(" days"));
        });
        ui.horizontal(|ui| {
            ui.label("History folder:");
            ui.label(RichText::new(config::history_dir().display().to_string()).monospace());
        });
        ui.horizontal(|ui| {
            ui.label("OCR models:");
            ui.label(RichText::new(config::models_dir().display().to_string()).monospace());
        });
    }

    fn about(&mut self, ui: &mut egui::Ui) {
        ui.heading(format!("ShotCraft {}", env!("CARGO_PKG_VERSION")));
        ui.label("Capture, annotate, redact and share screenshots on Windows, macOS and Linux.");
        ui.add_space(6.0);
        ui.label(RichText::new("Every mode is also a command: shotcraft --help").monospace());
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::left("tabs").exact_size(180.0).resizable(false).show(root, |ui| {
            ui.add_space(10.0);
            for (tab, _, ic, label) in TABS {
                let on = self.tab == tab;
                if ui.add_sized([ui.available_width(), 32.0], egui::Button::selectable(on, format!("{ic}  {label}"))).clicked() {
                    self.tab = tab;
                    self.recording = None;
                }
            }
        });
        egui::CentralPanel::default().show(root, |ui| {
            ui.add_space(6.0);
            match self.tab {
                Tab::General => self.general(ui),
                Tab::Shortcuts => self.shortcuts(ui),
                Tab::QuickAccess => self.quick_access(ui),
                Tab::Screenshots => self.screenshots(ui),
                Tab::Cloud => self.cloud(ui),
                Tab::Advanced => self.advanced(ui),
                Tab::About => self.about(ui),
            }
        });
        if self.settings != self.saved {
            // Only what changed here goes to disk, over the newest settings, so an editor's palette or preset edits survive.
            match self.settings.save_changes(&self.saved) {
                Ok(merged) => {
                    self.settings = merged.clone();
                    self.saved = merged;
                }
                Err(e) => self.error = Some(format!("Could not save settings: {e:#}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_bindings_parse_as_global_hotkeys() {
        let mods = Modifiers { alt: true, shift: true, ..Default::default() };
        for key in [Key::A, Key::Num4, Key::F5, Key::ArrowUp, Key::Minus] {
            let b = binding(mods, key).unwrap();
            assert!(HotKey::from_str(&b).is_ok(), "{b}");
        }
        assert!(binding(Modifiers::NONE, Key::A).is_none(), "bare letters are rejected");
        assert!(binding(Modifiers::NONE, Key::F9).is_some(), "bare F-keys are allowed");
    }

    #[test]
    fn default_hotkeys_parse() {
        for (cmd, key) in Settings::default().hotkeys.entries() {
            if !key.is_empty() {
                assert!(HotKey::from_str(&key).is_ok(), "{cmd}: {key}");
            }
        }
    }
}
