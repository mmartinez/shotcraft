//! Scrolling capture session and the self-timer countdown.
//!
//! The selected region is re-captured several times a second while the user (or
//! auto-scroll) scrolls the content underneath; frames are stitched as they arrive.
//! A control panel sits beside the region and a thin frame marks it, both outside it
//! so they never end up in the capture.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use egui::{Color32, Key, Rect, RichText, TextureHandle, TextureOptions, ViewportBuilder, ViewportId, WindowLevel, pos2, vec2};
use egui_phosphor::regular as icon;
use image::RgbaImage;

use shotcraft_engine::capture::{self, MonitorShot};
use shotcraft_engine::doc::effects::IRect;
use shotcraft_engine::doc::stitch::{Push, Stitcher};

use crate::theme;

const MAX_HEIGHT: u32 = 40_000;
const PANEL: egui::Vec2 = vec2(270.0, 330.0);

/// Region in logical coordinates (for placing windows around it).
fn region_logical(shot: &MonitorShot, r: IRect) -> Rect {
    let l = shot.logical_rect();
    let k = l.width() / shot.image.width() as f32;
    Rect::from_min_size(l.min + vec2(r.x as f32, r.y as f32) * k, vec2(r.w as f32, r.h as f32) * k)
}

/// A spot for a `size` window next to `region`, staying on the monitor when possible.
fn beside(monitor: Rect, region: Rect, size: egui::Vec2) -> egui::Pos2 {
    let top = monitor.top() + 8.0;
    let y = region.top().clamp(top, (monitor.bottom() - size.y - 8.0).max(top));
    if region.right() + 16.0 + size.x <= monitor.right() {
        pos2(region.right() + 16.0, y)
    } else if region.left() - 16.0 - size.x >= monitor.left() {
        pos2(region.left() - 16.0 - size.x, y)
    } else if region.bottom() + 16.0 + size.y <= monitor.bottom() {
        pos2(region.center().x - size.x / 2.0, region.bottom() + 16.0)
    } else {
        // No room outside: bottom-right corner, accepting overlap.
        monitor.right_bottom() - size - vec2(16.0, 16.0)
    }
}

#[derive(Default)]
struct Shared {
    stitcher: Option<Stitcher>,
    running: bool,
    autoscroll: bool,
    stop: bool,
    unchanged: u32,
    reached_end: bool,
    preview: Option<RgbaImage>,
    error: Option<String>,
}

/// The worker keeps no invariants across a panic, so a poisoned lock is still usable.
fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Session {
    shared: Arc<Mutex<Shared>>,
    worker: Option<std::thread::JoinHandle<()>>,
    region: Rect,
    preview_tex: Option<TextureHandle>,
    result: Rc<RefCell<Option<RgbaImage>>>,
    closing: bool,
}

pub fn run(monitor_id: Option<u32>, display: Option<usize>, rect: IRect, start: bool, autoscroll: bool) -> anyhow::Result<Option<RgbaImage>> {
    let shot = capture::capture_monitor(monitor_id, display)?;
    let rect =
        IRect::clipped(Rect::from_min_size(pos2(rect.x as f32, rect.y as f32), vec2(rect.w as f32, rect.h as f32)), shot.image.width(), shot.image.height())
            .ok_or_else(|| anyhow::anyhow!("area is outside the display"))?;
    let region = region_logical(&shot, rect);
    let pos = beside(shot.logical_rect(), region, PANEL);
    let shared = Arc::new(Mutex::new(Shared { stitcher: Some(Stitcher::new(MAX_HEIGHT)), running: start, autoscroll, ..Default::default() }));
    let worker = {
        let shared = shared.clone();
        let id = shot.id;
        let center = (shot.x as f32 + (rect.x + rect.w / 2) as f32 / shot.px_per_unit(), shot.y as f32 + (rect.y + rect.h / 2) as f32 / shot.px_per_unit());
        std::thread::spawn(move || capture_loop(shared, id, rect, (center.0 as i32, center.1 as i32)))
    };
    let result = Rc::new(RefCell::new(None));
    let session = Session { shared, worker: Some(worker), region, preview_tex: None, result: result.clone(), closing: false };
    let builder = ViewportBuilder::default()
        .with_title("Scrolling Capture")
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_window_level(WindowLevel::AlwaysOnTop)
        .with_position(pos)
        .with_inner_size(PANEL);
    theme::run("shotcraft-scrolling", builder, move |ctx| {
        ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::Dark);
        session
    })?;
    Ok(result.borrow_mut().take())
}

fn capture_loop(shared: Arc<Mutex<Shared>>, monitor_id: u32, rect: IRect, center: (i32, i32)) {
    use enigo::{Axis, Coordinate, Mouse};
    let Some(monitor) = capture::Monitor::by_id(monitor_id) else {
        lock(&shared).error = Some("display disappeared".into());
        return;
    };
    // Created on first auto-scroll: on macOS it needs the Accessibility permission, plain capture does not.
    let mut enigo: Option<enigo::Enigo> = None;
    loop {
        let (stop, running, auto) = {
            let s = lock(&shared);
            (s.stop, s.running, s.autoscroll)
        };
        if stop {
            return;
        }
        if !running {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        let frame = monitor.capture().map(|img| capture::crop(&img, rect));
        let mut s = lock(&shared);
        match frame {
            Ok(frame) => {
                // The session took the stitcher: it is finishing.
                let Some(stitcher) = s.stitcher.as_mut() else { return };
                let pushed = stitcher.push(frame);
                let full = stitcher.is_full();
                s.unchanged = if pushed == Push::Unchanged { s.unchanged + 1 } else { 0 };
                if matches!(pushed, Push::Appended(_)) {
                    s.preview = s.stitcher.as_ref().and_then(|st| st.tail(1400));
                }
                if full || (s.autoscroll && s.unchanged >= 3) {
                    s.running = false;
                    s.reached_end = true;
                }
            }
            Err(e) => s.error = Some(e.to_string()),
        }
        drop(s);
        if auto {
            if enigo.is_none() {
                match enigo::Enigo::new(&enigo::Settings::default()) {
                    Ok(e) => enigo = Some(e),
                    Err(e) => {
                        let mut s = lock(&shared);
                        s.autoscroll = false;
                        s.error = Some(format!("auto-scroll needs permission to control the pointer (macOS: Privacy & Security → Accessibility): {e}"));
                    }
                }
            }
            if let Some(e) = enigo.as_mut() {
                let _ = e.move_mouse(center.0, center.1, Coordinate::Abs);
                let _ = e.scroll(3, Axis::Vertical);
            }
            std::thread::sleep(Duration::from_millis(350));
        } else {
            std::thread::sleep(Duration::from_millis(110));
        }
    }
}

impl Session {
    fn finish(&mut self, ctx: &egui::Context, keep: bool) {
        lock(&self.shared).stop = true;
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        if keep {
            let stitcher = lock(&self.shared).stitcher.take();
            *self.result.borrow_mut() = stitcher.and_then(Stitcher::finish);
        }
        self.closing = true;
        ctx.send_viewport_cmd_to(ViewportId::ROOT, egui::ViewportCommand::Close);
    }

    /// Thin always-on-top bars just outside the region, ignoring the mouse.
    fn frame_windows(&self, ctx: &egui::Context) {
        let r = self.region;
        let t = 3.0;
        let bars = [
            Rect::from_min_max(pos2(r.left() - t - 1.0, r.top() - t - 1.0), pos2(r.right() + t + 1.0, r.top() - 1.0)),
            Rect::from_min_max(pos2(r.left() - t - 1.0, r.bottom() + 1.0), pos2(r.right() + t + 1.0, r.bottom() + t + 1.0)),
            Rect::from_min_max(pos2(r.left() - t - 1.0, r.top() - 1.0), pos2(r.left() - 1.0, r.bottom() + 1.0)),
            Rect::from_min_max(pos2(r.right() + 1.0, r.top() - 1.0), pos2(r.right() + t + 1.0, r.bottom() + 1.0)),
        ];
        for (i, bar) in bars.into_iter().enumerate() {
            let builder = ViewportBuilder::default()
                .with_title("scroll frame")
                .with_decorations(false)
                .with_resizable(false)
                .with_taskbar(false)
                .with_mouse_passthrough(true)
                .with_active(false)
                .with_window_level(WindowLevel::AlwaysOnTop)
                .with_position(bar.min)
                .with_inner_size(bar.size());
            ctx.show_viewport_immediate(ViewportId::from_hash_of(("frame", i)), builder, |ui, _| {
                egui::CentralPanel::default().frame(egui::Frame::NONE.fill(theme::ACCENT)).show(ui, |_| {});
            });
        }
    }
}

impl eframe::App for Session {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        if self.closing {
            return;
        }
        self.frame_windows(ctx);
        let (running, auto, reached_end, height, error, preview) = {
            let mut s = lock(&self.shared);
            if s.error.is_none() && !s.stop && self.worker.as_ref().is_some_and(|w| w.is_finished()) {
                s.error = Some("capture stopped unexpectedly".into());
            }
            let height = s.stitcher.as_ref().map_or(0, Stitcher::height);
            (s.running, s.autoscroll, s.reached_end, height, s.error.clone(), s.preview.take())
        };
        if let Some(p) = preview {
            let ci = egui::ColorImage::from_rgba_unmultiplied([p.width() as usize, p.height() as usize], p.as_raw());
            self.preview_tex = Some(ctx.load_texture("scroll-preview", ci, TextureOptions::LINEAR));
        }
        let (enter, esc) = ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
        if esc {
            return self.finish(ctx, false);
        }
        if enter && height > 0 {
            return self.finish(ctx, true);
        }
        ctx.request_repaint_after(Duration::from_millis(200));
        egui::CentralPanel::default().frame(theme::hud_frame().inner_margin(14)).show(root, |ui| {
            ui.label(RichText::new(format!("{}  Scrolling Capture", icon::ARROW_LINE_DOWN)).size(16.0).color(theme::HUD_TEXT));
            let status = match (&error, running, reached_end, auto) {
                (Some(e), ..) => format!("Error: {e}"),
                (_, _, true, _) => "Reached the end. Press Done.".into(),
                (_, true, _, true) => "Auto-scrolling…".into(),
                (_, true, _, false) => "Scroll the content slowly.".into(),
                _ => "Press Start, then scroll.".into(),
            };
            ui.label(RichText::new(status).color(theme::HUD_MUTED));
            ui.label(RichText::new(format!("{height} px captured")).color(theme::HUD_MUTED).size(12.0));
            let avail = vec2(ui.available_width(), 170.0);
            let (rect, _) = ui.allocate_exact_size(avail, egui::Sense::hover());
            ui.painter().rect_filled(rect, 8, Color32::from_black_alpha(90));
            if let Some(tex) = &self.preview_tex {
                let s = tex.size_vec2();
                let k = (rect.width() / s.x).min(rect.height() / s.y);
                // Bottom-aligned: the newest rows stay visible.
                let r = Rect::from_min_size(pos2(rect.center().x - s.x * k / 2.0, rect.bottom() - s.y * k), s * k);
                ui.painter().image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if !running && height == 0 && ui.button(format!("{}  Start", icon::PLAY)).clicked() {
                    lock(&self.shared).running = true;
                }
                let mut a = auto;
                if ui.toggle_value(&mut a, format!("{}  Auto-scroll", icon::MOUSE_SCROLL)).changed() {
                    let mut s = lock(&self.shared);
                    s.autoscroll = a;
                    if a {
                        s.running = true;
                        s.reached_end = false;
                    }
                }
            });
            ui.horizontal(|ui| {
                if ui.add_enabled(height > 0, egui::Button::new(format!("{}  Done", icon::CHECK)).fill(theme::ACCENT)).clicked() {
                    self.finish(ctx, true);
                }
                if ui.button("Cancel").clicked() {
                    self.finish(ctx, false);
                }
            });
        });
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }
}

struct Countdown {
    until: Instant,
    seconds: u32,
    fired: Rc<RefCell<bool>>,
}

/// Count down beside the region; returns false if cancelled with Esc.
pub fn countdown(monitor_id: u32, rect: IRect, seconds: u32) -> anyhow::Result<bool> {
    let shot = capture::capture_monitor(Some(monitor_id), None)?;
    let size = vec2(150.0, 150.0);
    let pos = beside(shot.logical_rect(), region_logical(&shot, rect), size);
    let fired = Rc::new(RefCell::new(false));
    let app = Countdown { until: Instant::now() + Duration::from_secs(seconds.max(1) as u64), seconds: seconds.max(1), fired: fired.clone() };
    theme::run("shotcraft-timer", theme::floating("Self-Timer", pos, size), move |_| app)?;
    Ok(*fired.borrow())
}

impl eframe::App for Countdown {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            *self.fired.borrow_mut() = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(50));
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(root, |ui| {
            let rect = ui.max_rect().shrink(4.0);
            let p = ui.painter();
            p.circle_filled(rect.center(), rect.width() / 2.0, theme::HUD_FILL);
            let frac = left.as_secs_f32() / self.seconds as f32;
            let pts: Vec<_> = (0..=64)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * frac * i as f32 / 64.0;
                    rect.center() + vec2(a.cos(), a.sin()) * (rect.width() / 2.0 - 6.0)
                })
                .collect();
            p.add(egui::Shape::line(pts, egui::Stroke::new(5.0, theme::ACCENT)));
            let n = left.as_secs_f32().ceil() as u32;
            p.text(rect.center(), egui::Align2::CENTER_CENTER, n.to_string(), egui::FontId::proportional(56.0), Color32::WHITE);
        });
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_placement_survives_a_screen_shorter_than_the_panel() {
        let monitor = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 300.0));
        let region = Rect::from_min_size(pos2(100.0, 50.0), vec2(200.0, 100.0));
        let p = beside(monitor, region, PANEL);
        assert!(p.y >= monitor.top() + 8.0, "{p:?}");
    }
}
