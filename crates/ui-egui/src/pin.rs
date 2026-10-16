//! Pinned screenshot: a borderless, always-on-top image you can move, zoom and fade.
//!
//! Drag to move, scroll to zoom, Ctrl/Cmd+scroll or 1–9/0 to change opacity,
//! arrow keys to nudge (Shift = 10px), Esc or double-click to close.

use std::path::PathBuf;

use egui::{Color32, Key, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2, ViewportCommand, vec2};
use egui_phosphor::regular as icon;
use image::RgbaImage;

use shotcraft_engine::config::Settings;
use shotcraft_engine::doc::model::Doc;
use shotcraft_engine::doc::render;
use shotcraft_engine::{capture, history, share};

use crate::{dialogs, theme};

/// What to pin: a history entry or an image/project file.
pub enum ImageSource {
    History(String),
    File(PathBuf),
}

struct Pin {
    image: RgbaImage,
    history_id: Option<String>,
    tex: Option<TextureHandle>,
    /// Natural size in points (image pixels at the display scale).
    natural: Vec2,
    zoom: f32,
    opacity: f32,
    settings: Settings,
}

pub fn run(source: ImageSource) -> anyhow::Result<()> {
    let (image, history_id) = match source {
        ImageSource::History(id) => (history::load_image(&id)?, Some(id)),
        ImageSource::File(path) => (render::flatten(&Doc::open(&path)?)?, None),
    };
    let screen = capture::primary_logical_rect();
    let scale = capture::primary_scale();
    let natural = vec2(image.width() as f32, image.height() as f32) / scale;
    let fit = (screen.width() * 0.7 / natural.x).min(screen.height() * 0.7 / natural.y).min(1.0);
    let size = (natural * fit).round();
    let pos = screen.center() - size / 2.0;
    let builder = theme::floating("ShotCraft pin", pos, size).with_resizable(false);
    theme::run("shotcraft-pin", builder, move |_| Pin { image, history_id, tex: None, natural, zoom: fit, opacity: 1.0, settings: Settings::load() })
}

impl Pin {
    fn set_zoom(&mut self, ctx: &egui::Context, zoom: f32) {
        self.zoom = zoom.clamp(0.1, 4.0);
        let size = (self.natural * self.zoom).round().max(vec2(40.0, 30.0));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
    }

    fn nudge(&self, ctx: &egui::Context, d: Vec2) {
        if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(outer.min + d));
        }
    }

    fn menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if ui.button(format!("{}  Copy", icon::COPY)).clicked() {
            let _ = share::copy_image(&self.image);
        }
        if ui.button(format!("{}  Save As…", icon::DOWNLOAD_SIMPLE)).clicked() {
            let _ = dialogs::save_as(&self.image, &self.settings);
        }
        if ui.button(format!("{}  Annotate", icon::PENCIL_SIMPLE)).clicked() {
            match &self.history_id {
                Some(id) => share::spawn(["open-annotate", "--history", id]),
                None => {
                    if let Ok(entry) = history::add(&Doc::new(self.image.clone()), "pin") {
                        share::spawn(["open-annotate", "--history", &entry.id]);
                    }
                }
            }
        }
        ui.separator();
        ui.menu_button(format!("{}  Opacity", icon::DROP_HALF), |ui| {
            for pct in [100, 75, 50, 25] {
                if ui.selectable_label((self.opacity * 100.0).round() as i32 == pct, format!("{pct}%")).clicked() {
                    self.opacity = pct as f32 / 100.0;
                }
            }
        });
        if ui.button(format!("{}  Actual Size", icon::CORNERS_OUT)).clicked() {
            self.set_zoom(&ctx, 1.0);
        }
        ui.separator();
        if ui.button(format!("{}  Close", icon::X)).clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }
}

impl eframe::App for Pin {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        let tex = self
            .tex
            .get_or_insert_with(|| {
                let ci = egui::ColorImage::from_rgba_unmultiplied([self.image.width() as usize, self.image.height() as usize], self.image.as_raw());
                ctx.load_texture("pin", ci, TextureOptions::LINEAR)
            })
            .clone();

        let (scroll, command, shift) = ctx.input(|i| (i.smooth_scroll_delta.y, i.modifiers.command, i.modifiers.shift));
        // Smooth scrolling spreads one wheel notch (~50 points) over several frames.
        if scroll != 0.0 {
            if command {
                self.opacity = (self.opacity + scroll * 0.002).clamp(0.1, 1.0);
            } else {
                self.set_zoom(ctx, self.zoom * (1.0 + scroll * 0.002));
            }
        }
        let step = if shift { 10.0 } else { 1.0 };
        // Collect first: calling back into `ctx` inside `ctx.input` deadlocks its lock.
        let arrows =
            [(Key::ArrowLeft, vec2(-step, 0.0)), (Key::ArrowRight, vec2(step, 0.0)), (Key::ArrowUp, vec2(0.0, -step)), (Key::ArrowDown, vec2(0.0, step))];
        let nudge = ctx.input(|i| arrows.iter().filter(|(k, _)| i.key_pressed(*k)).fold(Vec2::ZERO, |acc, (_, d)| acc + *d));
        if nudge != Vec2::ZERO {
            self.nudge(ctx, nudge);
        }
        let digit = ctx.input(|i| {
            [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9, Key::Num0]
                .iter()
                .position(|k| i.key_pressed(*k))
        });
        if let Some(n) = digit {
            self.opacity = (n + 1) as f32 / 10.0;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::C)) {
            let _ = share::copy_image(&self.image);
        }
        if ctx.input(|i| i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals)) {
            self.set_zoom(ctx, self.zoom * 1.1);
        }
        if ctx.input(|i| i.key_pressed(Key::Minus)) {
            self.set_zoom(ctx, self.zoom / 1.1);
        }

        egui::CentralPanel::default().frame(egui::Frame::NONE).show(root, |ui| {
            let rect = ui.max_rect();
            let resp = ui.allocate_rect(rect, Sense::click_and_drag());
            let tint = Color32::from_white_alpha((self.opacity * 255.0) as u8);
            egui::Image::new(&tex).tint(tint).paint_at(ui, rect);
            if ui.rect_contains_pointer(rect) {
                ui.painter().rect_stroke(rect, 0, Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
                if self.opacity < 1.0 || self.zoom != 1.0 {
                    let label = format!("{:.0}%  ·  opacity {:.0}%", self.zoom * 100.0, self.opacity * 100.0);
                    let r = Rect::from_min_size(rect.left_top() + vec2(6.0, 6.0), vec2(150.0, 20.0));
                    ui.painter().rect_filled(r, 6, theme::HUD_FILL);
                    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(11.0), Color32::WHITE);
                }
            }
            if resp.drag_started() {
                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if resp.double_clicked() {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            resp.context_menu(|ui| self.menu(ui));
        });
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }
}
