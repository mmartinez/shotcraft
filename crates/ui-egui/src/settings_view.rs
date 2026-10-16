//! Settings window: a sidebar of pages, each a stack of grouped rows with the label on the left and its control on
//! the right. Changes are saved immediately; the tray daemon reloads shortcuts on change.

use std::str::FromStr;

use egui::{
    Align2, Color32, ComboBox, DragValue, FontFamily, FontId, Frame, Key, Margin, Modifiers, Response, RichText, Sense, Stroke, StrokeKind, TextEdit,
    TextureHandle, Ui, WidgetInfo, WidgetType, vec2,
};
use egui_phosphor::regular as icon;
use global_hotkey::hotkey::HotKey;

use shotcraft_engine::config::{self, ExportFormat, QuickAccessSide, Settings, WindowCaptureStyle};
use shotcraft_engine::doc::fonts;

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

/// Page, the name `open-settings --tab` takes, sidebar icon and title.
const TABS: [(Tab, &str, &str, &str); 7] = [
    (Tab::General, "general", icon::GEAR, "General"),
    (Tab::Shortcuts, "shortcuts", icon::KEYBOARD, "Shortcuts"),
    (Tab::QuickAccess, "quickaccess", icon::IMAGE_SQUARE, "Quick Access"),
    (Tab::Screenshots, "screenshots", icon::CAMERA, "Capture"),
    (Tab::Cloud, "cloud", icon::CLOUD_ARROW_UP, "Upload"),
    (Tab::Advanced, "advanced", icon::SLIDERS_HORIZONTAL, "Advanced"),
    (Tab::About, "about", icon::INFO, "About"),
];

const WINDOW_STYLES: [(WindowCaptureStyle, &str); 3] =
    [(WindowCaptureStyle::Shadow, "Transparent with shadow"), (WindowCaptureStyle::Background, "On a background"), (WindowCaptureStyle::Plain, "Window only")];

/// Settings rows: minimum height, vertical padding, and the gap between a label and its help line.
const ROW_MIN: f32 = 40.0;
const ROW_PAD: f32 = 9.0;
const HELP_GAP: f32 = 2.0;

struct SettingsApp {
    settings: Settings,
    saved: Settings,
    tab: Tab,
    recording: Option<usize>,
    /// Why the last key combination was rejected while recording a shortcut.
    shortcut_error: Option<String>,
    /// The last save failure; cleared by the next successful save.
    error: Option<String>,
    headers_text: String,
    icon: Option<TextureHandle>,
}

pub fn run(tab: Option<String>) -> anyhow::Result<()> {
    let tab = tab.and_then(|t| TABS.iter().find(|x| x.1 == t.to_ascii_lowercase()).map(|x| x.0)).unwrap_or(Tab::General);
    let builder = egui::ViewportBuilder::default().with_title("ShotCraft Settings").with_inner_size(vec2(780.0, 560.0)).with_resizable(false);
    let settings = Settings::load();
    theme::run("shotcraft-settings", builder, move |ctx| SettingsApp {
        headers_text: settings.upload.headers.join("\n"),
        saved: settings.clone(),
        settings,
        tab,
        recording: None,
        shortcut_error: None,
        error: None,
        icon: theme::app_icon(128).map(|(rgba, size)| {
            let size = size as usize;
            ctx.load_texture("app-icon", egui::ColorImage::from_rgba_unmultiplied([size, size], &rgba), egui::TextureOptions::LINEAR)
        }),
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

/// What a key in a stored shortcut is called on this platform (`Control` → `Ctrl`, `KeyA` → `A`).
fn key_label(part: &str) -> &str {
    match part {
        "Control" => "Ctrl",
        "Alt" if cfg!(target_os = "macos") => "Option",
        "Super" if cfg!(target_os = "macos") => "Cmd",
        "Super" if cfg!(windows) => "Win",
        _ => ["Key", "Digit", "Arrow"].iter().find_map(|prefix| part.strip_prefix(prefix)).unwrap_or(part),
    }
}

fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(fonts::MEDIUM_FAMILY.into()))
}

fn help_font() -> FontId {
    FontId::proportional(12.0)
}

/// A titled box of rows, like the grouped lists of the platform settings apps.
fn group(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Rows<'_>)) {
    ui.add_space(18.0);
    if !title.is_empty() {
        ui.label(RichText::new(title).font(medium(13.0)).color(ui.visuals().weak_text_color()));
        ui.add_space(6.0);
    }
    let fill = if ui.visuals().dark_mode { Color32::from_gray(36) } else { Color32::from_gray(252) };
    Frame::new().fill(fill).stroke(ui.visuals().widgets.noninteractive.bg_stroke).corner_radius(10).inner_margin(Margin::symmetric(14, 0)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        add(&mut Rows { ui, first: true });
    });
}

/// Small print under a group.
fn note(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).font(help_font()).color(ui.visuals().weak_text_color()));
}

/// A tinted message box across the page (save errors, platform limits).
fn banner(ui: &mut Ui, glyph: &str, color: Color32, text: &str) {
    ui.add_space(14.0);
    Frame::new()
        .fill(color.gamma_multiply(0.14))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.5)))
        .corner_radius(8)
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.label(RichText::new(glyph).size(16.0).color(color));
                ui.add(egui::Label::new(text).wrap());
            });
        });
}

/// The rows of one group, separated by hairlines.
struct Rows<'a> {
    ui: &'a mut Ui,
    first: bool,
}

impl Rows<'_> {
    fn divider(&mut self) {
        if !self.first {
            self.ui.add(egui::Separator::default().spacing(0.0));
        }
        self.first = false;
    }

    /// Label and optional help line on the left, `control` on the right. The control side lays out right to left,
    /// so its first widget ends up rightmost.
    fn row<R>(&mut self, label: &str, help: Option<&str>, control: impl FnOnce(&mut Ui) -> R) -> R {
        self.divider();
        let ui = &mut *self.ui;
        let label_height = ui.text_style_height(&egui::TextStyle::Body);
        let help_height = if help.is_some() { HELP_GAP + ui.ctx().fonts_mut(|f| f.row_height(&help_font())) } else { 0.0 };
        let height = (label_height + help_height + 2.0 * ROW_PAD).max(ROW_MIN);
        let weak = ui.visuals().weak_text_color();
        let (_, inner) = egui::Sides::new().height(height).shrink_left().truncate().show(
            ui,
            |ui| {
                ui.vertical(|ui| {
                    // The row's full height, so short controls don't shrink it. Sides centres single widgets, but a
                    // label with a help line has to be centred by hand.
                    ui.set_min_height(height);
                    ui.add_space((height - label_height - help_height) / 2.0);
                    ui.spacing_mut().item_spacing.y = HELP_GAP;
                    ui.label(label);
                    if let Some(help) = help {
                        ui.label(RichText::new(help).font(help_font()).color(weak));
                    }
                });
            },
            control,
        );
        inner
    }

    fn switch(&mut self, label: &str, help: Option<&str>, on: &mut bool) -> Response {
        self.row(label, help, |ui| switch(ui, on, label))
    }

    /// Label and help above a control that takes the full width.
    fn block(&mut self, label: &str, help: &str, control: impl FnOnce(&mut Ui)) {
        self.divider();
        let ui = &mut *self.ui;
        ui.add_space(ROW_PAD);
        ui.label(label);
        ui.add_space(HELP_GAP);
        ui.label(RichText::new(help).font(help_font()).color(ui.visuals().weak_text_color()));
        ui.add_space(8.0);
        control(ui);
        ui.add_space(ROW_PAD + 2.0);
    }
}

/// On/off switch; `label` names it for screen readers.
fn switch(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *on, label));
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let off = ui.visuals().text_color().gamma_multiply(if response.hovered() { 0.4 } else { 0.28 });
        let radius = rect.height() / 2.0;
        let painter = ui.painter();
        painter.rect_filled(rect, radius, off.lerp_to_gamma(theme::ACCENT, t));
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        painter.circle_filled(egui::pos2(x, rect.center().y), radius - 3.0, Color32::from_gray(250));
        if response.has_focus() {
            painter.rect_stroke(rect.expand(2.0), radius + 2.0, Stroke::new(1.5, theme::ACCENT), StrokeKind::Outside);
        }
    }
    response
}

/// Two or three exclusive options in one strip. Rows lay controls out right to left, hence the reversed order.
fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) {
    // Painted behind the buttons afterwards: a Frame here would stretch to the row's full height. The leading space
    // keeps the painted edge in line with the other controls.
    ui.add_space(2.0);
    let background = ui.painter().add(egui::Shape::Noop);
    let strip = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for &(option, label) in options.iter().rev() {
            ui.selectable_value(value, option, label);
        }
    });
    let fill = ui.visuals().widgets.inactive.bg_fill;
    ui.painter().set(background, egui::epaint::RectShape::filled(strip.response.rect.expand(2.0), 8, fill));
}

fn copy_button(ui: &mut Ui, text: &str, what: &str) {
    if ui.button(icon::COPY).on_hover_text(format!("Copy {what}")).clicked() {
        ui.ctx().copy_text(text.to_owned());
    }
}

/// A stored shortcut drawn as key caps; clicking it records a new one.
fn shortcut_keys(ui: &mut Ui, binding: &str, name: &str) -> Response {
    const PAD: f32 = 7.0;
    const GAP: f32 = 4.0;
    const CAP_HEIGHT: f32 = 21.0;
    let font = FontId::proportional(12.5);
    let text = ui.visuals().text_color();
    let keys: Vec<&str> = binding.split('+').map(key_label).collect();
    let galleys: Vec<_> = keys.iter().map(|k| ui.painter().layout_no_wrap((*k).to_owned(), font.clone(), text)).collect();
    let width = galleys.iter().map(|g| g.size().x + 2.0 * PAD).sum::<f32>() + GAP * galleys.len().saturating_sub(1) as f32 + 8.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), format!("{name} shortcut: {}", keys.join(" "))));
    let visuals = ui.visuals();
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 7, visuals.widgets.hovered.weak_bg_fill);
    }
    if response.has_focus() {
        painter.rect_stroke(rect, 7, Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
    }
    let mut x = rect.left() + 4.0;
    for galley in galleys {
        let cap = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - CAP_HEIGHT / 2.0), vec2(galley.size().x + 2.0 * PAD, CAP_HEIGHT));
        painter.rect(cap, 5, visuals.widgets.inactive.bg_fill, visuals.widgets.noninteractive.bg_stroke, StrokeKind::Inside);
        painter.galley(cap.center() - galley.size() / 2.0, galley, text);
        x = cap.right() + GAP;
    }
    response
}

/// Sidebar entry: icon and title, left-aligned, tinted when selected.
fn nav_item(ui: &mut Ui, glyph: &str, label: &str, selected: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    response.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, ui.is_enabled(), selected, label));
    let visuals = ui.visuals();
    let fill = if selected {
        theme::ACCENT.gamma_multiply(if visuals.dark_mode { 0.3 } else { 0.15 })
    } else if response.hovered() {
        visuals.widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let text = if selected { visuals.strong_text_color() } else { visuals.text_color() };
    let painter = ui.painter();
    painter.rect_filled(rect, 7, fill);
    if response.has_focus() {
        painter.rect_stroke(rect, 7, Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
    }
    painter.text(rect.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER, glyph, FontId::proportional(16.0), if selected { theme::ACCENT } else { text });
    painter.text(rect.left_center() + vec2(38.0, 0.0), Align2::LEFT_CENTER, label, if selected { medium(13.5) } else { FontId::proportional(13.5) }, text);
    response
}

impl SettingsApp {
    fn general(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        group(ui, "After capture", |rows| {
            rows.switch("Show Quick Access", Some("A floating preview to copy, save, annotate, pin or drag the capture"), &mut s.show_quick_access);
            rows.switch("Copy to clipboard", None, &mut s.copy_after_capture);
            rows.switch("Save to folder", None, &mut s.save_after_capture);
        });
        group(ui, "Saving", |rows| {
            let dir = s.save_dir.display().to_string();
            rows.row("Folder", Some(&dir), |ui| {
                if ui.button("Change…").clicked()
                    && let Some(dir) = rfd::FileDialog::new().set_directory(&s.save_dir).pick_folder()
                {
                    s.save_dir = dir;
                }
            });
            rows.row("Format", None, |ui| segmented(ui, &mut s.export_format, &ExportFormat::ALL.map(|f| (f, f.label()))));
            if s.export_format == ExportFormat::Jpeg {
                rows.row("JPEG quality", None, |ui| {
                    ui.add(DragValue::new(&mut s.jpeg_quality).range(40..=100));
                    ui.add(egui::Slider::new(&mut s.jpeg_quality, 40..=100).show_value(false));
                });
            }
        });
    }

    fn shortcuts(&mut self, ui: &mut Ui) {
        if cfg!(target_os = "linux") && std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland")) {
            banner(
                ui,
                icon::INFO,
                theme::ACCENT,
                "These shortcuts need an X11 session. On Wayland, bind your desktop's own shortcuts to commands such as shotcraft capture-area.",
            );
        }
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
                            self.shortcut_error = None;
                        } else {
                            self.shortcut_error = Some("Use at least one modifier: Ctrl, Alt, Shift or Super.".into());
                        }
                    }
                }
            }
        }
        let recording = self.recording;
        let mut clicked = None;
        let mut errors = Vec::new();
        group(ui, "", |rows| {
            for (row, (label, _, key)) in self.settings.hotkeys.entries_mut().into_iter().enumerate() {
                if !key.is_empty() && HotKey::from_str(key).is_err() {
                    errors.push(format!("{label}: not a valid shortcut."));
                }
                rows.row(label, None, |ui| {
                    // A fixed slot for the clear button keeps every row's keys aligned.
                    if key.is_empty() || recording == Some(row) {
                        ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                    } else if ui.add_sized([24.0, 24.0], egui::Button::new(icon::X).frame(false)).on_hover_text("Remove shortcut").clicked() {
                        key.clear();
                    }
                    let response = if recording == Some(row) {
                        ui.add(
                            egui::Button::new(RichText::new("Press a shortcut…").color(theme::ACCENT))
                                .fill(Color32::TRANSPARENT)
                                .stroke(Stroke::new(1.0, theme::ACCENT)),
                        )
                    } else if key.is_empty() {
                        ui.button("Record shortcut")
                    } else {
                        shortcut_keys(ui, key, label).on_hover_text("Click to change")
                    };
                    if response.clicked() {
                        clicked = Some(row);
                    }
                });
            }
        });
        if let Some(row) = clicked {
            // Clicking the row being recorded cancels; any other row starts recording.
            self.recording = if recording == Some(row) { None } else { Some(row) };
            self.shortcut_error = None;
        }
        note(ui, "Click a shortcut, then press the new key combination. Backspace removes it; Esc cancels.");
        for e in errors.iter().chain(self.shortcut_error.iter()) {
            ui.add_space(4.0);
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
    }

    fn quick_access(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        if !s.show_quick_access {
            banner(ui, icon::INFO, theme::ACCENT, "Quick Access is off. Turn on Show Quick Access in General to use it after each capture.");
        }
        group(ui, "", |rows| {
            rows.row("Position", None, |ui| {
                segmented(ui, &mut s.quick_access_side, &[(QuickAccessSide::Left, "Bottom left"), (QuickAccessSide::Right, "Bottom right")])
            });
            rows.switch("Close after an action", Some("After copying, saving or dragging the capture"), &mut s.close_quick_access_after_action);
            let mut auto = s.quick_access_auto_close > 0;
            if rows.switch("Close automatically", Some("When the overlay is left alone"), &mut auto).changed() {
                s.quick_access_auto_close = if auto { 10 } else { 0 };
            }
            if auto {
                rows.row("Close after", None, |ui| ui.add(DragValue::new(&mut s.quick_access_auto_close).range(1..=600).suffix(" s")));
            }
        });
    }

    fn capture(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        group(ui, "Selection", |rows| {
            rows.switch("Crosshair", Some("Guide lines that follow the pointer"), &mut s.show_crosshair);
            rows.switch("Magnifier", Some("A zoomed view for pixel-exact edges"), &mut s.show_magnifier);
        });
        group(ui, "Window capture", |rows| {
            rows.row("Style", None, |ui| {
                let current = WINDOW_STYLES.iter().find(|(style, _)| *style == s.window_style).map_or("", |(_, label)| label);
                ComboBox::from_id_salt("window-style").selected_text(current).width(210.0).show_ui(ui, |ui| {
                    for (style, label) in WINDOW_STYLES {
                        ui.selectable_value(&mut s.window_style, style, label);
                    }
                });
            });
        });
        group(ui, "Self-timer", |rows| {
            rows.row("Countdown", None, |ui| ui.add(DragValue::new(&mut s.timer_seconds).range(1..=60).suffix(" s")));
        });
        group(ui, "Text recognition", |rows| {
            rows.switch("Keep line breaks", Some("Copy recognized text with its original line breaks"), &mut s.ocr_keep_linebreaks);
        });
    }

    fn upload(&mut self, ui: &mut Ui) {
        let u = &mut self.settings.upload;
        let headers_text = &mut self.headers_text;
        group(ui, "Endpoint", |rows| {
            rows.row("URL", Some("https, or http to this computer only"), |ui| {
                ui.add(TextEdit::singleline(&mut u.url).hint_text("https://example.com/upload").desired_width(280.0))
            });
            rows.row("File field", Some("Form field that carries the image"), |ui| ui.add(TextEdit::singleline(&mut u.file_field).desired_width(140.0)));
            rows.row("Link in response", Some("JSON path such as data.link; empty uses the whole response"), |ui| {
                ui.add(TextEdit::singleline(&mut u.response_path).hint_text("data.link").desired_width(140.0))
            });
            rows.block("Headers", "One per line, such as Authorization: Bearer followed by your token", |ui| {
                if ui.add(TextEdit::multiline(headers_text).hint_text("Authorization: Bearer …").desired_rows(3).desired_width(f32::INFINITY)).changed() {
                    u.headers = headers_text.lines().map(str::trim).filter(|l| l.contains(':')).map(str::to_owned).collect();
                }
            });
        });
        note(ui, "Uploads send the capture as a multipart/form-data POST and copy the link from the response.");
    }

    fn advanced(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        let history = config::history_dir().display().to_string();
        let models = config::models_dir().display().to_string();
        group(ui, "History", |rows| {
            rows.row("Keep captures for", None, |ui| ui.add(DragValue::new(&mut s.history_days).range(1..=365).suffix(" days")));
            rows.row("Folder", Some(&history), |ui| copy_button(ui, &history, "path"));
        });
        group(ui, "Text recognition", |rows| {
            rows.row("Model folder", Some(&models), |ui| copy_button(ui, &models, "path"));
        });
        note(ui, "The text recognition models (about 12 MB) download into the model folder the first time you use it.");
    }

    fn about(&mut self, ui: &mut Ui) {
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            if let Some(icon) = &self.icon {
                ui.add(egui::Image::new((icon.id(), vec2(64.0, 64.0))));
            }
            ui.vertical(|ui| {
                ui.add_space(6.0);
                ui.label(RichText::new("ShotCraft").font(medium(18.0)).color(ui.visuals().strong_text_color()));
                ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(ui.visuals().weak_text_color()));
                ui.label("Capture, annotate, redact and share screenshots.");
            });
        });
        group(ui, "", |rows| {
            rows.row("Command line", Some("Every capture mode is also a command"), |ui| ui.label(RichText::new("shotcraft --help").monospace()));
            rows.row("Source code", Some(env!("CARGO_PKG_REPOSITORY")), |ui| copy_button(ui, env!("CARGO_PKG_REPOSITORY"), "link"));
            rows.row("License", None, |ui| ui.label(env!("CARGO_PKG_LICENSE")));
        });
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, root: &mut Ui, _frame: &mut eframe::Frame) {
        let sidebar = Frame::side_top_panel(root.style()).inner_margin(Margin::symmetric(10, 14));
        egui::Panel::left("tabs").exact_size(188.0).resizable(false).frame(sidebar).show(root, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for (tab, _, glyph, label) in TABS {
                if nav_item(ui, glyph, label, self.tab == tab).clicked() {
                    self.tab = tab;
                    self.recording = None;
                    self.shortcut_error = None;
                }
            }
        });
        let page = Frame::central_panel(root.style()).inner_margin(Margin { left: 28, right: 16, top: 18, bottom: 0 });
        egui::CentralPanel::default().frame(page).show(root, |ui| {
            let title = TABS.iter().find(|t| t.0 == self.tab).map_or("", |t| t.3);
            ui.label(RichText::new(title).font(medium(22.0)).color(ui.visuals().strong_text_color()));
            if let Some(error) = &self.error {
                banner(ui, icon::WARNING, theme::WARNING, error);
            }
            egui::ScrollArea::vertical().id_salt(title).auto_shrink(false).show(ui, |ui| {
                // Room for the scroll bar, so it never covers a control.
                Frame::new().inner_margin(Margin { right: 12, ..Margin::ZERO }).show(ui, |ui| {
                    match self.tab {
                        Tab::General => self.general(ui),
                        Tab::Shortcuts => self.shortcuts(ui),
                        Tab::QuickAccess => self.quick_access(ui),
                        Tab::Screenshots => self.capture(ui),
                        Tab::Cloud => self.upload(ui),
                        Tab::Advanced => self.advanced(ui),
                        Tab::About => self.about(ui),
                    }
                    ui.add_space(24.0);
                });
            });
        });
        if self.settings != self.saved {
            // Only what changed here goes to disk, over the newest settings, so an editor's palette or preset edits survive.
            match self.settings.save_changes(&self.saved) {
                Ok(merged) => {
                    self.settings = merged.clone();
                    self.saved = merged;
                    self.error = None;
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
