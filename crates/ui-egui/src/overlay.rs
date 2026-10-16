//! Frozen-screen selection overlay: area, window, fullscreen and the All-In-One bar.
//!
//! Every monitor is captured first, then covered by a borderless window showing that
//! frozen image, so what you select is exactly what gets saved.

use std::cell::RefCell;
use std::rc::Rc;

use egui::{
    Align2, Color32, ColorImage, CursorIcon, FontId, Id, Key, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, ViewportBuilder,
    ViewportCommand, ViewportId, WindowLevel, pos2, vec2,
};
use egui_phosphor::regular as icon;
use image::RgbaImage;

use shotcraft_engine::capture::{self, MonitorShot, WinInfo};
use shotcraft_engine::config::Settings;
use shotcraft_engine::doc::effects::IRect;
use shotcraft_engine::doc::model::constrain_aspect;

use crate::{platform, theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    AllInOne,
    Area,
    Window,
    Scrolling,
    Timer,
    Text,
}

pub enum Outcome {
    Image { image: RgbaImage, kind: &'static str, area: Option<(u32, IRect)> },
    Window { id: u32, fallback: RgbaImage },
    Scrolling { monitor_id: u32, rect: IRect },
    Timer { monitor_id: u32, rect: IRect },
    Text { image: RgbaImage },
}

/// Initial selection, in monitor image pixels.
pub struct Preset {
    pub display: Option<usize>,
    pub rect: IRect,
}

#[derive(Clone, Copy)]
struct Selection {
    monitor: usize,
    /// Image pixels of that monitor.
    rect: Rect,
}

#[derive(Clone, Copy)]
enum Drag {
    New { anchor: Pos2 },
    Move { grab: egui::Vec2 },
    Handle { index: usize, opposite: Pos2 },
}

const ASPECTS: [(Option<(u32, u32)>, &str); 7] = [
    (None, "Freeform"),
    (Some((1, 1)), "1:1"),
    (Some((4, 3)), "4:3"),
    (Some((3, 2)), "3:2"),
    (Some((16, 9)), "16:9"),
    (Some((9, 16)), "9:16"),
    (Some((21, 9)), "21:9"),
];

struct OverlayApp {
    monitors: Vec<MonitorShot>,
    textures: Vec<Option<TextureHandle>>,
    root: usize,
    mode: Mode,
    /// All-In-One: picking a window instead of drawing an area.
    window_pick: bool,
    windows: Vec<WinInfo>,
    sel: Option<Selection>,
    drag: Option<(usize, Drag)>,
    aspect: Option<(u32, u32)>,
    size_text: (String, String),
    crosshair: bool,
    magnifier: bool,
    frames: u32,
    outcome: Rc<RefCell<Option<Outcome>>>,
    finished: bool,
}

fn monitor_builder(m: &MonitorShot, title: &str) -> ViewportBuilder {
    let r = m.logical_rect();
    let b = ViewportBuilder::default()
        .with_title(title)
        .with_decorations(false)
        .with_taskbar(false)
        .with_window_level(WindowLevel::AlwaysOnTop)
        .with_position(r.min)
        .with_inner_size(r.size())
        .with_app_id(theme::APP_ID);
    // Borderless fullscreen pins the window exactly onto its monitor, even when winit's scale
    // factor differs from xcap's. Mutter-family WMs refuse it for fixed-size windows, so only
    // macOS sets non-resizable: it would animate into a separate Space, so there the sized
    // window is raised above the menu bar instead.
    if cfg!(target_os = "macos") { b.with_resizable(false) } else { b.with_fullscreen(true) }
}

pub fn run(mode: Mode, preset: Option<Preset>) -> anyhow::Result<Option<Outcome>> {
    let monitors = capture::capture_monitors()?;
    let mut root = capture::cursor_monitor(&monitors);
    let settings = Settings::load();
    let mut sel = None;
    if let Some(p) = preset {
        if let Some(i) = p.display.and_then(|d| capture::index_of_display(&monitors, d)) {
            root = i;
        }
        let m = &monitors[root];
        if let Some(r) = IRect::clipped(
            Rect::from_min_size(pos2(p.rect.x as f32, p.rect.y as f32), vec2(p.rect.w as f32, p.rect.h as f32)),
            m.image.width(),
            m.image.height(),
        ) {
            sel = Some(Selection { monitor: root, rect: Rect::from_min_size(pos2(r.x as f32, r.y as f32), vec2(r.w as f32, r.h as f32)) });
        }
    }
    let windows = if matches!(mode, Mode::Window | Mode::AllInOne) { capture::list_windows() } else { Vec::new() };
    let outcome = Rc::new(RefCell::new(None));
    let n = monitors.len();
    let app = OverlayApp {
        root,
        textures: (0..n).map(|_| None).collect(),
        mode,
        window_pick: mode == Mode::Window,
        windows,
        sel,
        drag: None,
        aspect: None,
        size_text: (String::new(), String::new()),
        crosshair: settings.show_crosshair,
        magnifier: settings.show_magnifier,
        frames: 0,
        outcome: outcome.clone(),
        finished: false,
        monitors,
    };
    let options =
        eframe::NativeOptions { viewport: monitor_builder(&app.monitors[root], "ShotCraft capture"), renderer: eframe::Renderer::Glow, ..Default::default() };
    eframe::run_native(
        "shotcraft-overlay",
        options,
        Box::new(|cc| {
            theme::setup(&cc.egui_ctx);
            cc.egui_ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::Dark);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("overlay failed: {e}"))?;
    Ok(outcome.borrow_mut().take())
}

impl eframe::App for OverlayApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.frames += 1;
        if self.frames < 4 {
            platform::raise_all_windows();
        }
        self.monitor_ui(root, self.root);
        for i in 0..self.monitors.len() {
            if i == self.root {
                continue;
            }
            let builder = monitor_builder(&self.monitors[i], "ShotCraft capture");
            ctx.show_viewport_immediate(ViewportId::from_hash_of(("monitor", i)), builder, |ui, _| self.monitor_ui(ui, i));
        }
        if self.finished {
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }
}

impl OverlayApp {
    fn texture(&mut self, ctx: &egui::Context, i: usize) -> TextureHandle {
        self.textures[i]
            .get_or_insert_with(|| {
                let img = &self.monitors[i].image;
                let ci = ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                ctx.load_texture(format!("monitor-{i}"), ci, TextureOptions::LINEAR)
            })
            .clone()
    }

    fn finish(&mut self, outcome: Option<Outcome>) {
        *self.outcome.borrow_mut() = outcome;
        self.finished = true;
    }

    fn selection_px(&self) -> Option<(usize, IRect)> {
        let s = self.sel?;
        let img = &self.monitors[s.monitor].image;
        Some((s.monitor, IRect::clipped(snap_noise(s.rect), img.width(), img.height())?))
    }

    /// Complete the capture of the current selection in `mode`.
    fn confirm(&mut self, mode: Mode) {
        let Some((m, r)) = self.selection_px() else { return };
        if r.w < 2 || r.h < 2 {
            return;
        }
        let shot = &self.monitors[m];
        let outcome = match mode {
            Mode::Scrolling => Outcome::Scrolling { monitor_id: shot.id, rect: r },
            Mode::Timer => Outcome::Timer { monitor_id: shot.id, rect: r },
            Mode::Text => Outcome::Text { image: capture::crop(&shot.image, r) },
            _ => Outcome::Image { image: capture::crop(&shot.image, r), kind: "area", area: Some((shot.id, r)) },
        };
        self.finish(Some(outcome));
    }

    fn capture_fullscreen(&mut self, m: usize) {
        let image = self.monitors[m].image.clone();
        self.finish(Some(Outcome::Image { image, kind: "fullscreen", area: None }));
    }

    fn window_at(&self, m: usize, px: Pos2) -> Option<usize> {
        let shot = &self.monitors[m];
        let k = shot.px_per_unit();
        let unit = pos2(shot.x as f32 + px.x / k, shot.y as f32 + px.y / k);
        self.windows.iter().position(|w| w.rect.contains(unit))
    }

    fn pick_window(&mut self, m: usize, wi: usize) {
        let w = &self.windows[wi];
        let shot = &self.monitors[m];
        let fallback = capture::unit_rect_to_px(shot, w.rect).map(|r| capture::crop(&shot.image, r)).unwrap_or_else(|| shot.image.clone());
        self.finish(Some(Outcome::Window { id: w.id, fallback }));
    }

    fn apply_aspect(&self, anchor: Pos2, p: Pos2, shift: bool) -> Pos2 {
        let ratio = if shift { Some((1, 1)) } else { self.aspect };
        match ratio {
            Some((w, h)) => constrain_aspect(anchor, p, w as f32 / h as f32),
            None => p,
        }
    }

    fn monitor_ui(&mut self, root: &mut egui::Ui, m: usize) {
        let ctx = &root.ctx().clone();
        let tex = self.texture(ctx, m);
        let img_size = vec2(self.monitors[m].image.width() as f32, self.monitors[m].image.height() as f32);
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(root, |ui| {
            let screen = ui.max_rect();
            let k = img_size.x / screen.width();
            let to_px = |p: Pos2| pos2(((p.x - screen.min.x) * k).clamp(0.0, img_size.x), ((p.y - screen.min.y) * k).clamp(0.0, img_size.y));
            let to_pt = |p: Pos2| screen.min + p.to_vec2() / k;
            let resp = ui.allocate_rect(screen, Sense::click_and_drag());
            let painter = ui.painter_at(screen);
            painter.image(tex.id(), screen, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
            let pointer = ctx.input(|i| i.pointer.latest_pos()).filter(|p| screen.contains(*p));
            let shift = ctx.input(|i| i.modifiers.shift);
            ctx.set_cursor_icon(if self.window_pick { CursorIcon::PointingHand } else { CursorIcon::Crosshair });

            self.handle_keys(ctx, m);

            if self.window_pick {
                let hovered = pointer.and_then(|p| self.window_at(m, to_px(p)));
                let shot = &self.monitors[m];
                let hrect = hovered
                    .and_then(|wi| capture::unit_rect_to_px(shot, self.windows[wi].rect))
                    .map(|r| Rect::from_min_size(to_pt(pos2(r.x as f32, r.y as f32)), vec2(r.w as f32, r.h as f32) / k));
                dim_outside(&painter, screen, hrect);
                if let (Some(r), Some(wi)) = (hrect, hovered) {
                    painter.rect_filled(r, 0, theme::ACCENT.gamma_multiply(0.18));
                    painter.rect_stroke(r, 0, Stroke::new(2.0, theme::ACCENT), StrokeKind::Inside);
                    let w = &self.windows[wi];
                    let label = if w.app.is_empty() { w.title.clone() } else { w.app.clone() };
                    label_pill(&painter, r.center(), &label);
                    if resp.clicked() {
                        self.pick_window(m, wi);
                    }
                }
            } else {
                self.area_interaction(ctx, &resp, m, k, screen, &to_px, shift);
                let sel = self.sel.filter(|s| s.monitor == m).map(|s| (Rect::from_min_max(to_pt(s.rect.min), to_pt(s.rect.max)), s.rect));
                match sel {
                    Some((r, s)) => {
                        dim_outside(&painter, screen, Some(r));
                        painter.rect_stroke(r, 0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Outside);
                        if self.mode == Mode::AllInOne && self.drag.is_none() {
                            for h in handle_points(r) {
                                painter.circle_filled(h, 4.5, Color32::WHITE);
                                painter.circle_stroke(h, 4.5, Stroke::new(1.0, theme::ACCENT));
                            }
                        }
                        let text = format!("{} × {}", s.width().round() as u32, s.height().round() as u32);
                        let at = if r.top() > 30.0 { r.left_top() + vec2(0.0, -16.0) } else { r.left_top() + vec2(0.0, 16.0) };
                        size_pill(&painter, at, &text);
                    }
                    None => {
                        if self.crosshair
                            && let Some(p) = pointer
                        {
                            let line = Stroke::new(1.0, Color32::from_white_alpha(170));
                            painter.line_segment([pos2(screen.left(), p.y), pos2(screen.right(), p.y)], line);
                            painter.line_segment([pos2(p.x, screen.top()), pos2(p.x, screen.bottom())], line);
                        }
                    }
                }
                let dragging_new = matches!(self.drag, Some((dm, Drag::New { .. })) if dm == m);
                if self.magnifier
                    && (self.sel.is_none() || dragging_new)
                    && let Some(p) = pointer
                {
                    self.draw_magnifier(ctx, &painter, m, p, to_px(p), screen);
                }
            }
            if self.mode == Mode::AllInOne && (self.sel.map_or(m == self.root, |s| s.monitor == m)) {
                self.all_in_one_bar(ctx, m, screen);
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn area_interaction(&mut self, ctx: &egui::Context, resp: &egui::Response, m: usize, k: f32, screen: Rect, to_px: &dyn Fn(Pos2) -> Pos2, shift: bool) {
        let to_pt = |p: Pos2| screen.min + p.to_vec2() / k;
        if resp.drag_started_by(egui::PointerButton::Primary) {
            let Some(start) = ctx.input(|i| i.pointer.press_origin()) else { return };
            let px = to_px(start);
            let existing = self.sel.filter(|s| s.monitor == m && self.mode == Mode::AllInOne);
            let drag = match existing {
                Some(s) => {
                    let pts = handle_points(Rect::from_min_max(to_pt(s.rect.min), to_pt(s.rect.max)));
                    if let Some(index) = pts.iter().position(|h| h.distance(start) < 10.0) {
                        let opp = handle_points(s.rect)[(index + 4) % 8];
                        Drag::Handle { index, opposite: opp }
                    } else if s.rect.contains(px) {
                        Drag::Move { grab: px - s.rect.min }
                    } else {
                        Drag::New { anchor: px }
                    }
                }
                None => Drag::New { anchor: px },
            };
            self.drag = Some((m, drag));
        }
        if let (Some((dm, drag)), Some(p)) = (self.drag, resp.interact_pointer_pos())
            && dm == m
        {
            let px = to_px(p);
            let bounds = Rect::from_min_size(Pos2::ZERO, screen.size() * k);
            match drag {
                Drag::New { anchor } => {
                    let end = self.apply_aspect(anchor, px, shift);
                    self.sel = Some(Selection { monitor: m, rect: Rect::from_two_pos(anchor, end).intersect(bounds) });
                }
                Drag::Move { grab } => {
                    if let Some(s) = &mut self.sel {
                        let size = s.rect.size();
                        // `max` can dip below zero by rounding when the selection spans the whole display; `clamp` would panic.
                        let min = (px - grab).clamp(Pos2::ZERO, (bounds.max - size).max(Pos2::ZERO));
                        s.rect = Rect::from_min_size(min, size);
                    }
                }
                Drag::Handle { index, opposite } => {
                    let corner = self.apply_aspect(opposite, px, shift);
                    if let Some(s) = &mut self.sel {
                        let r = s.rect;
                        // Edge handles move one axis; `opposite` is the fixed edge's midpoint.
                        let (a, b) = match index {
                            1 => (pos2(r.min.x, opposite.y), pos2(r.max.x, px.y)),
                            5 => (pos2(r.min.x, px.y), pos2(r.max.x, opposite.y)),
                            3 => (pos2(opposite.x, r.min.y), pos2(px.x, r.max.y)),
                            7 => (pos2(px.x, r.min.y), pos2(opposite.x, r.max.y)),
                            _ => (opposite, corner),
                        };
                        s.rect = Rect::from_two_pos(a, b).intersect(bounds);
                    }
                }
            }
            self.sync_size_text();
        }
        if resp.drag_stopped() && self.drag.is_some_and(|(dm, _)| dm == m) {
            self.drag = None;
            let big_enough = self.sel.is_some_and(|s| s.rect.width() >= 4.0 && s.rect.height() >= 4.0);
            if !big_enough {
                self.sel = None;
            } else if self.mode != Mode::AllInOne {
                self.confirm(self.mode);
            }
        }
        if resp.double_clicked() && self.mode == Mode::AllInOne && self.sel.is_some() {
            self.confirm(Mode::Area);
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context, m: usize) {
        let (esc, enter, space) = ctx.input(|i| (i.key_pressed(Key::Escape), i.key_pressed(Key::Enter), i.key_pressed(Key::Space)));
        if esc {
            if self.mode == Mode::AllInOne && (self.sel.is_some() || self.window_pick) {
                self.sel = None;
                self.window_pick = false;
            } else {
                self.finish(None);
            }
        }
        if enter && self.sel.is_some() && !ctx.text_edit_focused() {
            self.confirm(if self.mode == Mode::AllInOne { Mode::Area } else { self.mode });
        }
        if space && matches!(self.mode, Mode::Area | Mode::AllInOne | Mode::Window) && !ctx.text_edit_focused() {
            self.window_pick = !self.window_pick;
            if self.window_pick && self.windows.is_empty() {
                self.windows = capture::list_windows();
            }
        }
        // Arrow keys nudge the selection (Shift resizes) in All-In-One mode.
        if self.mode == Mode::AllInOne
            && !ctx.text_edit_focused()
            && let Some(s) = &mut self.sel
        {
            let (shift, l, r, u, d) = ctx.input(|i| {
                (i.modifiers.shift, i.key_pressed(Key::ArrowLeft), i.key_pressed(Key::ArrowRight), i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown))
            });
            let d = vec2((r as i32 - l as i32) as f32, (d as i32 - u as i32) as f32);
            if d != egui::Vec2::ZERO && s.monitor == m {
                if shift {
                    s.rect.max += d;
                } else {
                    s.rect = s.rect.translate(d);
                }
                self.sync_size_text();
            }
        }
    }

    fn sync_size_text(&mut self) {
        if let Some(s) = self.sel {
            self.size_text = ((s.rect.width().round() as u32).to_string(), (s.rect.height().round() as u32).to_string());
        }
    }

    fn draw_magnifier(&self, ctx: &egui::Context, painter: &egui::Painter, m: usize, p: Pos2, px: Pos2, screen: Rect) {
        const CELLS: i32 = 15;
        const SIZE: f32 = 120.0;
        let img = &self.monitors[m].image;
        let (cx, cy) = (px.x.floor() as i32, px.y.floor() as i32);
        let mut ci = ColorImage::new([CELLS as usize, CELLS as usize], vec![Color32::BLACK; (CELLS * CELLS) as usize]);
        for dy in 0..CELLS {
            for dx in 0..CELLS {
                let (x, y) = (cx + dx - CELLS / 2, cy + dy - CELLS / 2);
                if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
                    let [r, g, b, _] = img.get_pixel(x as u32, y as u32).0;
                    ci.pixels[(dy * CELLS + dx) as usize] = Color32::from_rgb(r, g, b);
                }
            }
        }
        let center = ci.pixels[(CELLS / 2 * CELLS + CELLS / 2) as usize];
        let tex = ctx.load_texture("magnifier", ci, TextureOptions::NEAREST);
        let mut origin = p + vec2(24.0, 24.0);
        if origin.x + SIZE > screen.right() {
            origin.x = p.x - 24.0 - SIZE;
        }
        if origin.y + SIZE + 40.0 > screen.bottom() {
            origin.y = p.y - 24.0 - SIZE - 40.0;
        }
        let r = Rect::from_min_size(origin, vec2(SIZE, SIZE));
        painter.rect_filled(r.expand(3.0), 10, theme::HUD_FILL);
        painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        let cell = SIZE / CELLS as f32;
        let grid = Stroke::new(0.5, Color32::from_black_alpha(50));
        for i in 1..CELLS {
            let o = i as f32 * cell;
            painter.line_segment([pos2(r.left() + o, r.top()), pos2(r.left() + o, r.bottom())], grid);
            painter.line_segment([pos2(r.left(), r.top() + o), pos2(r.right(), r.top() + o)], grid);
        }
        let c = Rect::from_min_size(r.min + vec2(CELLS as f32 / 2.0 - 0.5, CELLS as f32 / 2.0 - 0.5) * cell, vec2(cell, cell));
        painter.rect_stroke(c, 0, Stroke::new(1.5, theme::ACCENT), StrokeKind::Middle);
        let info = format!("{}, {}   #{:02X}{:02X}{:02X}", cx, cy, center.r(), center.g(), center.b());
        let info_rect = Rect::from_min_size(r.left_bottom() + vec2(-3.0, 8.0), vec2(SIZE + 6.0, 26.0));
        painter.rect_filled(info_rect, 8, theme::HUD_FILL);
        painter.text(info_rect.center(), Align2::CENTER_CENTER, info, FontId::monospace(11.0), theme::HUD_TEXT);
    }

    fn all_in_one_bar(&mut self, ctx: &egui::Context, m: usize, screen: Rect) {
        let anchor = pos2(screen.center().x, screen.bottom() - 110.0);
        egui::Area::new(Id::new(("all-in-one", m))).fixed_pos(anchor).pivot(Align2::CENTER_CENTER).order(egui::Order::Foreground).show(ctx, |ui| {
            theme::hud_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    let has_sel = self.sel.is_some();
                    if theme::tool_button(ui, icon::SELECTION, Some("Area"), !self.window_pick && has_sel, true)
                        .on_hover_text("Capture selection (Enter)")
                        .clicked()
                    {
                        if has_sel { self.confirm(Mode::Area) } else { self.window_pick = false }
                    }
                    if theme::tool_button(ui, icon::MONITOR, Some("Fullscreen"), false, true).clicked() {
                        self.capture_fullscreen(m);
                    }
                    if theme::tool_button(ui, icon::APP_WINDOW, Some("Window"), self.window_pick, true).on_hover_text("Pick a window (Space)").clicked() {
                        self.window_pick = !self.window_pick;
                        if self.windows.is_empty() {
                            self.windows = capture::list_windows();
                        }
                    }
                    for (mode, ic, label, tip) in [
                        (Mode::Scrolling, icon::ARROW_LINE_DOWN, "Scrolling", "Scrolling capture of the selection"),
                        (Mode::Timer, icon::TIMER, "Timer", "Capture the selection after a delay"),
                        (Mode::Text, icon::TEXT_AA, "OCR", "Copy text from the selection"),
                    ] {
                        if theme::tool_button(ui, ic, Some(label), false, true).on_hover_text(tip).clicked() && has_sel {
                            self.confirm(mode);
                        }
                    }
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let (mut w, mut h) = self.size_text.clone();
                            let wr = ui.add(egui::TextEdit::singleline(&mut w).desired_width(52.0).horizontal_align(egui::Align::Center));
                            ui.label(egui::RichText::new("×").color(theme::HUD_MUTED));
                            let hr = ui.add(egui::TextEdit::singleline(&mut h).desired_width(52.0).horizontal_align(egui::Align::Center));
                            if wr.changed() || hr.changed() {
                                self.size_text = (w.clone(), h.clone());
                                if let (Ok(w), Ok(h), Some(s)) = (w.parse::<f32>(), h.parse::<f32>(), &mut self.sel) {
                                    let img = &self.monitors[s.monitor].image;
                                    let max = vec2(img.width() as f32, img.height() as f32);
                                    let size = vec2(w.clamp(2.0, max.x), h.clamp(2.0, max.y));
                                    let min = s.rect.min.min((max - size).to_pos2());
                                    s.rect = Rect::from_min_size(min, size);
                                } else if let (Ok(w), Ok(h), None) = (w.parse::<f32>(), h.parse::<f32>(), self.sel) {
                                    let img = &self.monitors[m].image;
                                    let size = vec2(w.min(img.width() as f32), h.min(img.height() as f32));
                                    let min = ((vec2(img.width() as f32, img.height() as f32) - size) / 2.0).to_pos2();
                                    self.sel = Some(Selection { monitor: m, rect: Rect::from_min_size(min, size) });
                                }
                            }
                            let label = ASPECTS.iter().find(|(a, _)| *a == self.aspect).map_or("Freeform", |(_, l)| l);
                            egui::ComboBox::from_id_salt(("aspect", m)).selected_text(label).width(90.0).show_ui(ui, |ui| {
                                for (a, l) in ASPECTS {
                                    if ui.selectable_label(self.aspect == a, l).clicked() {
                                        self.aspect = a;
                                        if let (Some((aw, ah)), Some(s)) = (a, &mut self.sel) {
                                            let end = constrain_aspect(s.rect.min, s.rect.max, aw as f32 / ah as f32);
                                            s.rect = Rect::from_two_pos(s.rect.min, end);
                                        }
                                        self.sync_size_text();
                                    }
                                }
                            });
                        });
                    });
                    ui.separator();
                    if theme::tool_button(ui, icon::X, Some("Cancel"), false, true).clicked() {
                        self.finish(None);
                    }
                });
            });
        });
    }
}

/// Snaps edges within 0.01 px of a whole pixel onto it. Point-to-pixel math at fractional
/// scales lands a hair off exact edges, and outward rounding would then grow the capture.
fn snap_noise(r: Rect) -> Rect {
    let snap = |v: f32| if (v - v.round()).abs() < 0.01 { v.round() } else { v };
    Rect::from_min_max(pos2(snap(r.min.x), snap(r.min.y)), pos2(snap(r.max.x), snap(r.max.y)))
}

/// Corner and edge midpoints, clockwise from top-left.
fn handle_points(r: Rect) -> [Pos2; 8] {
    [r.left_top(), r.center_top(), r.right_top(), r.right_center(), r.right_bottom(), r.center_bottom(), r.left_bottom(), r.left_center()]
}

fn dim_outside(painter: &egui::Painter, screen: Rect, hole: Option<Rect>) {
    let dim = Color32::from_black_alpha(110);
    let Some(h) = hole.map(|h| h.intersect(screen)) else {
        painter.rect_filled(screen, 0, Color32::from_black_alpha(60));
        return;
    };
    painter.rect_filled(Rect::from_min_max(screen.min, pos2(screen.max.x, h.min.y)), 0, dim);
    painter.rect_filled(Rect::from_min_max(pos2(screen.min.x, h.max.y), screen.max), 0, dim);
    painter.rect_filled(Rect::from_min_max(pos2(screen.min.x, h.min.y), pos2(h.min.x, h.max.y)), 0, dim);
    painter.rect_filled(Rect::from_min_max(pos2(h.max.x, h.min.y), pos2(screen.max.x, h.max.y)), 0, dim);
}

fn size_pill(painter: &egui::Painter, left_center: Pos2, text: &str) {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(12.0), Color32::WHITE);
    let r = Rect::from_min_size(left_center - vec2(0.0, galley.size().y / 2.0 + 3.0), galley.size() + vec2(14.0, 6.0));
    painter.rect_filled(r, 6, theme::HUD_FILL);
    painter.galley(r.min + vec2(7.0, 3.0), galley, Color32::WHITE);
}

fn label_pill(painter: &egui::Painter, center: Pos2, text: &str) {
    let text: String = text.chars().take(60).collect();
    let galley = painter.layout_no_wrap(text, FontId::proportional(14.0), Color32::WHITE);
    let r = Rect::from_center_size(center, galley.size() + vec2(20.0, 10.0));
    painter.rect_filled(r, 8, theme::HUD_FILL);
    painter.galley(r.min + vec2(10.0, 5.0), galley, Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_noise_fixes_float_noise_but_keeps_real_fractions() {
        let r = |x0, y0, x1, y1| Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
        assert_eq!(snap_noise(r(99.999_99, 119.999_99, 700.000_06, 520.000_06)), r(100.0, 120.0, 700.0, 520.0));
        assert_eq!(snap_noise(r(10.4, 10.6, 20.2, 20.7)), r(10.4, 10.6, 20.2, 20.7));
    }
}
