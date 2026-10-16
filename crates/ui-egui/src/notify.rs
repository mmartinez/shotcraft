//! Small self-dismissing toast in the top-right corner of the primary display.

use std::fs::File;
use std::time::{Duration, Instant};

use egui::{Align2, Color32, FontFamily, FontId, RichText, Sense, pos2, vec2};

use shotcraft_engine::capture;
use shotcraft_engine::doc::fonts;

use crate::theme;

const LIFETIME: Duration = Duration::from_millis(3500);
const SIZE: egui::Vec2 = vec2(360.0, 92.0);

struct Toast {
    title: String,
    body: String,
    warning: bool,
    shown: Instant,
    _slot: File,
}

pub fn run(title: String, body: String, warning: bool) -> anyhow::Result<()> {
    let (slot, lock) = theme::claim_slot("toast")?;
    let screen = capture::primary_logical_rect();
    let pos = pos2(screen.right() - SIZE.x - 16.0, screen.top() + 40.0 + slot as f32 * (SIZE.y + 10.0));
    let builder = theme::floating("ShotCraft notification", pos, SIZE).with_active(false);
    theme::run("shotcraft-notify", builder, move |_| Toast { title, body, warning, shown: Instant::now(), _slot: lock })
}

impl eframe::App for Toast {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        let elapsed = self.shown.elapsed();
        if elapsed >= LIFETIME {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(LIFETIME.saturating_sub(elapsed));
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(root, |ui| {
            let rect = ui.max_rect().shrink(2.0);
            let resp = ui.allocate_rect(rect, Sense::click());
            if resp.clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ui.painter().rect_filled(rect, 14, theme::HUD_FILL);
            let (fill, glyph) = if self.warning { (theme::WARNING, egui_phosphor::regular::WARNING) } else { (theme::ACCENT, egui_phosphor::regular::CHECK) };
            ui.painter().circle_filled(rect.left_center() + vec2(30.0, 0.0), 16.0, fill);
            ui.painter().text(rect.left_center() + vec2(30.0, 0.0), Align2::CENTER_CENTER, glyph, FontId::proportional(18.0), Color32::WHITE);
            let text_rect = egui::Rect::from_min_max(rect.min + vec2(58.0, 14.0), rect.max - vec2(12.0, 10.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(text_rect), |ui| {
                ui.label(RichText::new(&self.title).family(FontFamily::Name(fonts::MEDIUM_FAMILY.into())).size(15.0).color(theme::HUD_TEXT));
                ui.add(egui::Label::new(RichText::new(&self.body).size(12.5).color(theme::HUD_MUTED)).truncate());
            });
        });
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }
}
