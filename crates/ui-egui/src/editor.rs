//! Annotate window: markup tools, redaction, crop, background tool, presets and export.
//!
//! The canvas shows the document rendered by [`render`] at screen resolution, so the
//! preview is pixel-for-pixel what gets saved. Only handles and the live text editor are
//! drawn by egui on top.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use egui::{
    Align2, Color32, CornerRadius, FontFamily, FontId, Key, Mesh, Pos2, Rect, RichText, Sense, Shape as EShape, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Vec2, pos2, vec2,
};
use egui_phosphor::regular as icon;
use image::RgbaImage;

use shotcraft_engine::config::{ExportFormat, Preset, Settings};
use shotcraft_engine::doc::fonts;
use shotcraft_engine::doc::model::{
    ArrowStyle, Aspect, Background, BgFill, Doc, GRADIENTS, Item, Orient, PROJECT_EXT, RedactMode, Shape, TextStyle, constrain_aspect,
};
use shotcraft_engine::doc::render::{self, RenderOpts};
use shotcraft_engine::{capture, history, ocr, share};

use crate::{dialogs, theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Select,
    Rect,
    FilledRect,
    Ellipse,
    Line,
    Arrow,
    Text,
    Counter,
    Pen,
    Highlighter,
    Redact,
    Spotlight,
}

const TOOLS: [(Tool, &str, &str, Key); 12] = [
    (Tool::Select, icon::CURSOR, "Select (V)", Key::V),
    (Tool::Rect, icon::SQUARE, "Rectangle (R)", Key::R),
    (Tool::FilledRect, icon::RECTANGLE, "Filled rectangle (F)", Key::F),
    (Tool::Ellipse, icon::CIRCLE, "Ellipse (O)", Key::O),
    (Tool::Line, icon::LINE_SEGMENT, "Line (L)", Key::L),
    (Tool::Arrow, icon::ARROW_UP_RIGHT, "Arrow (A)", Key::A),
    (Tool::Text, icon::TEXT_T, "Text (T)", Key::T),
    (Tool::Redact, icon::CHECKERBOARD, "Pixelate / Blur (P)", Key::P),
    (Tool::Spotlight, icon::FLASHLIGHT, "Spotlight (S)", Key::S),
    (Tool::Counter, icon::NUMBER_CIRCLE_ONE, "Counter (N)", Key::N),
    (Tool::Pen, icon::PENCIL_SIMPLE, "Pencil (D)", Key::D),
    (Tool::Highlighter, icon::HIGHLIGHTER, "Highlighter (H)", Key::H),
];

#[derive(Clone)]
struct Snapshot {
    base: Arc<RgbaImage>,
    items: Vec<Item>,
    crop: Option<Rect>,
    background: Option<Background>,
}

#[derive(Clone, Copy)]
enum EditDrag {
    Create { index: usize, start: Pos2 },
    Move { index: usize, last: Pos2 },
    Handle { index: usize, handle: usize },
    Pan { last: Pos2 },
}

enum CropAction {
    Done,
    Cancel,
    Reset,
    Aspect(Option<(u32, u32)>),
}

#[derive(Clone, Copy)]
enum CropDrag {
    New(Pos2),
    Move(Pos2),
    Corner(usize),
}

struct CropState {
    rect: Rect,
    aspect: Option<(u32, u32)>,
    drag: Option<CropDrag>,
}

/// Mapping between screen points and base pixels for the current frame.
#[derive(Clone, Copy)]
struct View {
    img_rect: Rect,
    region: Rect,
    offset: Vec2,
    out_scale: f32,
    out_size: Vec2,
}

impl Default for View {
    fn default() -> Self {
        Self { img_rect: Rect::ZERO, region: Rect::ZERO, offset: Vec2::ZERO, out_scale: 1.0, out_size: Vec2::splat(1.0) }
    }
}

impl View {
    fn to_base(self, p: Pos2) -> Pos2 {
        let out = (p - self.img_rect.min) * (self.out_size.x / self.img_rect.width());
        self.region.min + (out - self.offset) / self.out_scale
    }

    fn to_screen(self, b: Pos2) -> Pos2 {
        let out = self.offset + (b - self.region.min) * self.out_scale;
        self.img_rect.min + out * (self.img_rect.width() / self.out_size.x)
    }

    /// Screen points per base pixel.
    fn ppb(self) -> f32 {
        self.out_scale * self.img_rect.width() / self.out_size.x
    }
}

struct Editor {
    doc: Doc,
    history_id: Option<String>,
    project_path: Option<PathBuf>,
    settings: Settings,
    tool: Tool,
    color: Color32,
    width: f32,
    arrow_style: ArrowStyle,
    text_style: TextStyle,
    text_size: f32,
    redact_mode: RedactMode,
    spotlight_ellipse: bool,
    selected: Option<usize>,
    drag: Option<EditDrag>,
    editing: Option<(usize, String)>,
    focus_text: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    zoom: f32,
    pan: Vec2,
    show_background: bool,
    crop: Option<CropState>,
    picking_color: bool,
    export_percent: u32,
    view: View,
    tex: Option<TextureHandle>,
    render_key: Option<(u64, u32)>,
    base_cache: Option<(u32, RgbaImage)>,
    revision: u64,
    saved_revision: u64,
    status: Option<(String, Instant)>,
    task: Option<(&'static str, Receiver<anyhow::Result<String>>)>,
    confirm_close: bool,
    allow_close: bool,
    preset_name: String,
}

pub fn run(doc: Doc, history_id: Option<String>, path: Option<PathBuf>) -> anyhow::Result<()> {
    let screen = capture::primary_logical_rect();
    let scale = capture::primary_scale();
    let natural = vec2(doc.base.width() as f32, doc.base.height() as f32) / scale + vec2(80.0, 140.0);
    // Not `clamp`: it panics when a small screen makes the upper bound less than the minimum.
    let size = natural.max(vec2(980.0, 640.0)).min(screen.size() * 0.88);
    let project_path = path.filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(PROJECT_EXT)));
    let title = match (&project_path, &history_id) {
        (Some(p), _) => format!("{} — Annotate", p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()),
        _ => "Annotate".to_owned(),
    };
    let builder = egui::ViewportBuilder::default().with_title(title).with_inner_size(size).with_min_inner_size(vec2(760.0, 480.0)).with_drag_and_drop(true);
    let settings = Settings::load();
    let color = settings.palette.first().map_or(Color32::from_rgb(255, 59, 48), |c| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]));
    let show_background = doc.background.is_some();
    theme::run("shotcraft-editor", builder, move |_| Editor {
        doc,
        history_id,
        project_path,
        settings,
        tool: Tool::Arrow,
        color,
        width: 6.0,
        arrow_style: ArrowStyle::Tapered,
        text_style: TextStyle::Plain,
        text_size: 32.0,
        redact_mode: RedactMode::Pixelate,
        spotlight_ellipse: false,
        selected: None,
        drag: None,
        editing: None,
        focus_text: false,
        undo: Vec::new(),
        redo: Vec::new(),
        zoom: 1.0,
        pan: Vec2::ZERO,
        show_background,
        crop: None,
        picking_color: false,
        export_percent: 100,
        view: View::default(),
        tex: None,
        render_key: None,
        base_cache: None,
        revision: 0,
        saved_revision: 0,
        status: None,
        task: None,
        confirm_close: false,
        allow_close: false,
        preset_name: String::new(),
    })
}

fn swatch_color(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

impl Editor {
    fn snapshot(&self) -> Snapshot {
        Snapshot { base: self.doc.base.clone(), items: self.doc.items.clone(), crop: self.doc.crop, background: self.doc.background.clone() }
    }

    fn restore(&mut self, s: Snapshot) {
        self.doc.base = s.base;
        self.doc.items = s.items;
        self.doc.crop = s.crop;
        self.doc.background = s.background;
        self.selected = self.selected.filter(|&i| i < self.doc.items.len());
        self.base_cache = None;
        self.touch();
    }

    /// Record the current state for undo, before a change.
    fn checkpoint(&mut self) {
        self.undo.push(self.snapshot());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn touch(&mut self) {
        self.revision += 1;
    }

    fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(s);
        }
    }

    fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(s);
        }
    }

    fn flash(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    fn report<T, E: std::fmt::Display>(&mut self, r: Result<T, E>) -> Option<T> {
        r.map_err(|e| self.flash(format!("{e:#}"))).ok()
    }

    fn export_image(&self) -> shotcraft_engine::Result<RgbaImage> {
        Ok(render::resize_percent(&render::flatten(&self.doc)?, self.export_percent))
    }

    /// False only when the history copy could not be written.
    fn sync_history(&mut self) -> bool {
        let Some(id) = self.history_id.clone() else { return true };
        let r = history::update(&id, &self.doc);
        self.report(r).is_some()
    }

    /// Apply one settings change to the newest settings on disk, so it can't undo another window's edits.
    fn update_settings(&mut self, f: impl FnOnce(&mut Settings)) -> bool {
        let r = Settings::update(f);
        let Some(settings) = self.report(r) else { return false };
        self.settings = settings;
        true
    }

    fn copy(&mut self) {
        let r = self.export_image().and_then(|img| share::copy_image(&img));
        if self.report(r).is_some() {
            self.sync_history();
            self.flash("Copied to clipboard");
        }
    }

    fn save(&mut self) {
        if let Some(path) = self.project_path.clone() {
            let r = self.doc.save_project(&path).map_err(|e| format!("Could not save {}: {e}", path.display()));
            if self.report(r).is_some() {
                self.saved_revision = self.revision;
                self.flash(format!("Saved {}", path.display()));
            }
            return;
        }
        let r = self.export_image().and_then(|img| share::save_default(&img, &self.settings));
        if let Some(path) = self.report(r) {
            self.saved_revision = self.revision;
            self.sync_history();
            self.flash(format!("Saved {}", path.display()));
        }
    }

    fn save_as(&mut self) {
        let r = self.export_image().and_then(|img| dialogs::save_as(&img, &self.settings));
        if let Some(Some(path)) = self.report(r) {
            self.saved_revision = self.revision;
            self.sync_history();
            self.flash(format!("Saved {}", path.display()));
        }
    }

    fn save_project_as(&mut self) {
        let name = format!("{}.{PROJECT_EXT}", share::default_file_name(ExportFormat::Png).trim_end_matches(".png"));
        let Some(chosen) =
            rfd::FileDialog::new().set_directory(&self.settings.save_dir).set_file_name(name).add_filter("ShotCraft project", &[PROJECT_EXT]).save_file()
        else {
            return;
        };
        let path = if chosen.extension().is_some_and(|e| e.eq_ignore_ascii_case(PROJECT_EXT)) {
            chosen
        } else {
            // Append rather than replace: "Shot 18.25.31" must not become "Shot 18.25.shotcraft".
            let mut name = chosen.into_os_string();
            name.push(".");
            name.push(PROJECT_EXT);
            let path = PathBuf::from(name);
            // The dialog only confirmed overwriting the name it showed.
            if path.exists() {
                self.flash(format!("{} already exists; choose another name", path.display()));
                return;
            }
            path
        };
        let r = self.doc.save_project(&path).map_err(|e| format!("Could not save {}: {e}", path.display()));
        if self.report(r).is_some() {
            self.saved_revision = self.revision;
            self.flash(format!("Saved project {}", path.display()));
            self.project_path = Some(path);
        }
    }

    fn ensure_history(&mut self) -> Option<String> {
        if self.history_id.is_none() {
            let r = history::add(&self.doc, "edit");
            self.history_id = self.report(r).map(|e| e.id);
        } else {
            self.sync_history();
        }
        self.history_id.clone()
    }

    fn pin(&mut self) {
        if let Some(id) = self.ensure_history() {
            share::spawn(["pin", "--history", &id]);
        }
    }

    fn background_task(&mut self, label: &'static str, ctx: &egui::Context, job: impl FnOnce(RgbaImage, Settings) -> anyhow::Result<String> + Send + 'static) {
        if self.task.is_some() {
            return;
        }
        let r = self.export_image();
        let Some(img) = self.report(r) else { return };
        let (tx, rx) = channel();
        let (settings, ctx2) = (self.settings.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(job(img, settings));
            ctx2.request_repaint();
        });
        self.task = Some((label, rx));
    }

    fn upload(&mut self, ctx: &egui::Context) {
        self.sync_history();
        self.background_task("Uploading…", ctx, |img, settings| {
            let link = share::upload(&img, &settings)?;
            share::copy_text(&link)?;
            Ok(format!("Link copied: {link}"))
        });
    }

    fn copy_text(&mut self, ctx: &egui::Context) {
        self.background_task("Recognizing text…", ctx, |img, settings| {
            let text = ocr::extract(&img, settings.ocr_keep_linebreaks)?;
            anyhow::ensure!(!text.trim().is_empty(), "no text found");
            share::copy_text(&text)?;
            Ok("Text copied".into())
        });
    }

    fn new_item(&self, shape: Shape) -> Item {
        Item { shape, color: self.color, width: self.width }
    }

    /// The shape a drag with the current tool starts at `b`; `None` for tools that don't draw by dragging.
    fn new_shape(&self, b: Pos2) -> Option<Shape> {
        let r = Rect::from_two_pos(b, b);
        Some(match self.tool {
            Tool::Rect => Shape::Rect { rect: r, filled: false },
            Tool::FilledRect => Shape::Rect { rect: r, filled: true },
            Tool::Ellipse => Shape::Ellipse { rect: r, filled: false },
            Tool::Line => Shape::Line { a: b, b },
            Tool::Arrow => Shape::Arrow { a: b, b, style: self.arrow_style, bend: if self.arrow_style == ArrowStyle::Curved { 0.25 } else { 0.0 } },
            Tool::Pen => Shape::Pen { pts: vec![b] },
            Tool::Highlighter => Shape::Highlighter { pts: vec![b] },
            Tool::Redact => Shape::Redact { rect: r, mode: self.redact_mode, seed: rand_seed() },
            Tool::Spotlight => Shape::Spotlight { rect: r, ellipse: self.spotlight_ellipse },
            Tool::Select | Tool::Text | Tool::Counter => return None,
        })
    }

    fn add_image_item(&mut self, img: RgbaImage) {
        let content = self.doc.content_rect();
        let size = vec2(img.width() as f32, img.height() as f32);
        let k = (content.width() * 0.5 / size.x).min(content.height() * 0.5 / size.y).min(1.0);
        let rect = Rect::from_center_size(content.center(), size * k);
        self.checkpoint();
        self.doc.items.push(Item { shape: Shape::Image { rect, image: Arc::new(img) }, color: Color32::WHITE, width: 0.0 });
        self.selected = Some(self.doc.items.len() - 1);
        self.tool = Tool::Select;
        self.touch();
    }

    fn reorient(&mut self, op: Orient) {
        self.commit_text();
        self.checkpoint();
        self.doc.reorient(op);
        self.base_cache = None;
        self.touch();
    }

    fn commit_text(&mut self) {
        let Some((i, text)) = self.editing.take() else { return };
        if i >= self.doc.items.len() {
            return;
        }
        if text.trim().is_empty() {
            self.doc.items.remove(i);
            self.selected = None;
        } else if let Shape::Text { text: t, .. } = &mut self.doc.items[i].shape {
            *t = text;
        }
        self.touch();
    }

    fn start_text_edit(&mut self, i: usize) {
        if let Shape::Text { text, .. } = &self.doc.items[i].shape {
            self.editing = Some((i, text.clone()));
            self.selected = Some(i);
            self.focus_text = true;
            self.touch();
        }
    }

    /// Apply the toolbar's style to the selected item.
    fn restyle_selected(&mut self, f: impl FnOnce(&mut Item)) {
        if let Some(i) = self.selected.filter(|&i| i < self.doc.items.len()) {
            self.checkpoint();
            f(&mut self.doc.items[i]);
            self.touch();
        }
    }

    fn select_tool(&mut self, tool: Tool) {
        self.commit_text();
        self.tool = tool;
        if tool != Tool::Select {
            self.selected = None;
        }
        self.crop = None;
    }

    fn render_if_needed(&mut self, ctx: &egui::Context, canvas: Rect) {
        let crop_mode = self.crop.is_some();
        let opts = |scale: f32| RenderOpts {
            scale,
            hide: self.editing.as_ref().map(|e| e.0),
            ignore_crop: crop_mode,
            background: self.show_background || self.doc.background.is_some(),
        };
        let full = render::output_size(&self.doc, &opts(1.0));
        let ppp = ctx.pixels_per_point();
        let fit = ((canvas.width() - 48.0) / full.x).min((canvas.height() - 48.0) / full.y).min(1.0 / ppp).max(0.01);
        let points_per_out1 = fit * self.zoom;
        // Render at physical-pixel resolution, never above the base image's own resolution.
        let scale = (points_per_out1 * ppp).min(1.0);
        let scale_key = (scale * 1000.0).round() as u32;
        let key = (self.revision, scale_key);
        if self.render_key != Some(key) || self.tex.is_none() {
            let base = match &self.base_cache {
                Some((k, scaled)) if *k == scale_key => scaled,
                _ => {
                    let base = &self.doc.base;
                    let scaled = if scale >= 0.999 {
                        (**base).clone()
                    } else {
                        let w = ((base.width() as f32 * scale).round() as u32).max(1);
                        let h = ((base.height() as f32 * scale).round() as u32).max(1);
                        image::imageops::resize(&**base, w, h, image::imageops::FilterType::Triangle)
                    };
                    &self.base_cache.insert((scale_key, scaled)).1
                }
            };
            match render::render(&self.doc, base, &opts(scale)) {
                Ok(out) => {
                    let size = [out.pixmap.width() as usize, out.pixmap.height() as usize];
                    let ci = egui::ColorImage::from_rgba_premultiplied(size, out.pixmap.data());
                    let filter = if self.zoom > 1.5 { TextureOptions::NEAREST } else { TextureOptions::LINEAR };
                    match &mut self.tex {
                        Some(t) => t.set(ci, filter),
                        None => self.tex = Some(ctx.load_texture("canvas", ci, filter)),
                    }
                    self.view.region = out.region;
                    self.view.offset = out.offset;
                    self.view.out_scale = out.scale;
                    self.view.out_size = vec2(size[0] as f32, size[1] as f32);
                }
                Err(e) => self.flash(e.to_string()),
            }
            self.render_key = Some(key);
        }
        let display = full * points_per_out1;
        self.view.img_rect = Rect::from_center_size(canvas.center() + self.pan, display);
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.editing.is_some() {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.commit_text();
            }
            return;
        }
        if ctx.text_edit_focused() {
            return;
        }
        let (cmd, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        if cmd {
            if pressed(Key::Z) {
                if shift { self.redo() } else { self.undo() }
            } else if pressed(Key::Y) {
                self.redo();
            } else if pressed(Key::C) {
                self.copy();
            } else if pressed(Key::S) {
                if shift { self.save_as() } else { self.save() }
            } else if pressed(Key::Num0) {
                self.zoom = 1.0;
                self.pan = Vec2::ZERO;
            } else if pressed(Key::Plus) || pressed(Key::Equals) {
                self.zoom = (self.zoom * 1.25).min(16.0);
            } else if pressed(Key::Minus) {
                self.zoom = (self.zoom / 1.25).max(0.1);
            }
            return;
        }
        if (pressed(Key::Delete) || pressed(Key::Backspace))
            && let Some(i) = self.selected.take().filter(|&i| i < self.doc.items.len())
        {
            self.checkpoint();
            self.doc.items.remove(i);
            self.touch();
        }
        if pressed(Key::Escape) {
            if self.crop.is_some() {
                self.crop = None;
                self.touch();
            } else {
                self.selected = None;
            }
        }
        if pressed(Key::Enter) && self.crop.is_some() {
            self.apply_crop();
        }
        for (tool, _, _, key) in TOOLS {
            if pressed(key) {
                self.select_tool(tool);
            }
        }
        if pressed(Key::C) {
            self.enter_crop();
        }
        if pressed(Key::B) {
            self.toggle_background();
        }
    }

    fn toggle_background(&mut self) {
        self.show_background = !self.show_background;
        if self.show_background && self.doc.background.is_none() {
            self.checkpoint();
            self.doc.background = Some(Background::default());
        }
        self.touch();
    }

    fn enter_crop(&mut self) {
        self.commit_text();
        self.crop = Some(CropState { rect: self.doc.content_rect(), aspect: None, drag: None });
        self.selected = None;
        self.touch();
    }

    fn apply_crop(&mut self) {
        if let Some(c) = self.crop.take() {
            let full = self.doc.full_rect();
            let r = Rect::from_min_max(c.rect.min.round(), c.rect.max.round()).intersect(full);
            if r.width() >= 2.0 && r.height() >= 2.0 {
                self.checkpoint();
                self.doc.crop = (r != full).then_some(r);
            }
            self.touch();
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal_centered(|ui| {
            if theme::tool_button(ui, icon::CROP, None, self.crop.is_some(), false).on_hover_text("Crop (C)").clicked() {
                if self.crop.is_some() { self.apply_crop() } else { self.enter_crop() }
            }
            if theme::tool_button(ui, icon::IMAGE_SQUARE, None, self.show_background, false).on_hover_text("Background tool (B)").clicked() {
                self.toggle_background();
            }
            ui.separator();
            for (tool, ic, tip, _) in TOOLS {
                if theme::tool_button(ui, ic, None, self.tool == tool && self.crop.is_none(), false).on_hover_text(tip).clicked() {
                    self.select_tool(tool);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(RichText::new(icon::DOTS_THREE).size(18.0), |ui| {
                    if ui.button(format!("{}  Save As…", icon::DOWNLOAD_SIMPLE)).clicked() {
                        self.save_as();
                    }
                    if ui.button(format!("{}  Save Project…", icon::FILE_IMAGE)).clicked() {
                        self.save_project_as();
                    }
                    ui.separator();
                    if ui.button(format!("{}  Pin to Screen", icon::PUSH_PIN)).clicked() {
                        self.pin();
                    }
                    if ui.button(format!("{}  Upload", icon::CLOUD_ARROW_UP)).clicked() {
                        self.upload(&ctx);
                    }
                    if ui.button(format!("{}  Copy Text (OCR)", icon::TEXT_AA)).clicked() {
                        self.copy_text(&ctx);
                    }
                    ui.separator();
                    for (op, ic, label) in [
                        (Orient::RotateCcw, icon::ARROW_COUNTER_CLOCKWISE, "Rotate Left"),
                        (Orient::RotateCw, icon::ARROW_CLOCKWISE, "Rotate Right"),
                        (Orient::FlipH, icon::FLIP_HORIZONTAL, "Flip Horizontal"),
                        (Orient::FlipV, icon::FLIP_VERTICAL, "Flip Vertical"),
                    ] {
                        if ui.button(format!("{ic}  {label}")).clicked() {
                            self.reorient(op);
                        }
                    }
                    ui.separator();
                    if ui.button(format!("{}  Paste Image", icon::CLIPBOARD)).clicked() {
                        let r = share::clipboard_image();
                        if let Some(img) = self.report(r) {
                            self.add_image_item(img);
                        }
                    }
                });
                let save = egui::Button::new(RichText::new(format!("{}  Save", icon::FLOPPY_DISK)).color(Color32::WHITE)).fill(theme::ACCENT);
                if ui.add(save).on_hover_text("Save (Ctrl/Cmd+S)").clicked() {
                    self.save();
                }
                if ui.button(format!("{}  Copy", icon::COPY)).on_hover_text("Copy (Ctrl/Cmd+C)").clicked() {
                    self.copy();
                }
                ui.add_enabled_ui(!self.redo.is_empty(), |ui| {
                    if theme::tool_button(ui, icon::ARROW_U_UP_RIGHT, None, false, false).on_hover_text("Redo").clicked() {
                        self.redo();
                    }
                });
                ui.add_enabled_ui(!self.undo.is_empty(), |ui| {
                    if theme::tool_button(ui, icon::ARROW_U_UP_LEFT, None, false, false).on_hover_text("Undo").clicked() {
                        self.undo();
                    }
                });
            });
        });
    }

    fn style_controls(&mut self, ui: &mut egui::Ui) {
        // Colour swatch with palette popover.
        let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
        ui.painter().circle_filled(rect.center(), 11.0, self.color);
        ui.painter().circle_stroke(rect.center(), 11.0, Stroke::new(2.0, ui.visuals().widgets.inactive.bg_fill));
        let popup_id = ui.id().with("palette");
        egui::Popup::from_toggle_button_response(&resp).id(popup_id).show(|ui| {
            ui.set_min_width(220.0);
            ui.horizontal_wrapped(|ui| {
                let palette = self.settings.palette.clone();
                for c in palette {
                    let col = swatch_color(c);
                    let (r, sresp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                    ui.painter().circle_filled(r.center(), 10.0, col);
                    if col == self.color {
                        ui.painter().circle_stroke(r.center(), 11.5, Stroke::new(2.0, theme::ACCENT));
                    }
                    if sresp.clicked() {
                        self.color = col;
                        self.restyle_selected(|it| it.color = col);
                    }
                }
            });
            ui.separator();
            let mut c = self.color;
            if ui.color_edit_button_srgba(&mut c).changed() {
                self.color = c;
                self.restyle_selected(|it| it.color = c);
            }
            ui.horizontal(|ui| {
                if ui.button(format!("{} Add to palette", icon::PLUS)).clicked() {
                    let c = self.color.to_srgba_unmultiplied();
                    self.update_settings(|s| s.palette.push(c));
                }
                if ui.button(format!("{} Pick from image", icon::EYEDROPPER)).clicked() {
                    self.picking_color = true;
                }
            });
        });
        ui.add_space(4.0);
        let mut w = self.width;
        if ui.add(egui::Slider::new(&mut w, 1.0..=30.0).show_value(false)).on_hover_text("Stroke width").changed() {
            self.width = w;
            if let Some(i) = self.selected.filter(|&i| i < self.doc.items.len()) {
                // One undo step per slider drag, not per frame.
                if !ui.ctx().input(|inp| inp.pointer.any_down()) || self.undo.is_empty() {
                    self.checkpoint();
                }
                self.doc.items[i].width = w;
                self.touch();
            }
        }
        let selected_shape = self.selected.and_then(|i| self.doc.items.get(i)).map(|it| it.shape.clone());
        let tool = match &selected_shape {
            Some(Shape::Arrow { .. }) => Tool::Arrow,
            Some(Shape::Text { .. }) => Tool::Text,
            Some(Shape::Redact { .. }) => Tool::Redact,
            Some(Shape::Spotlight { .. }) => Tool::Spotlight,
            _ => self.tool,
        };
        match tool {
            Tool::Arrow => {
                let styles =
                    [(ArrowStyle::Tapered, "Tapered"), (ArrowStyle::Classic, "Classic"), (ArrowStyle::Double, "Double"), (ArrowStyle::Curved, "Curved")];
                let current = styles.iter().find(|s| s.0 == self.arrow_style).map_or("Tapered", |s| s.1);
                egui::ComboBox::from_id_salt("arrow-style").selected_text(current).width(90.0).show_ui(ui, |ui| {
                    for (s, l) in styles {
                        if ui.selectable_label(self.arrow_style == s, l).clicked() {
                            self.arrow_style = s;
                            self.restyle_selected(|it| {
                                if let Shape::Arrow { style, bend, .. } = &mut it.shape {
                                    *style = s;
                                    if s == ArrowStyle::Curved && *bend == 0.0 {
                                        *bend = 0.25;
                                    }
                                }
                            });
                        }
                    }
                });
            }
            Tool::Text => {
                egui::ComboBox::from_id_salt("text-style").selected_text(format!("{:?}", self.text_style)).width(90.0).show_ui(ui, |ui| {
                    for s in TextStyle::ALL {
                        if ui.selectable_label(self.text_style == s, format!("{s:?}")).clicked() {
                            self.text_style = s;
                            self.restyle_selected(|it| {
                                if let Shape::Text { style, .. } = &mut it.shape {
                                    *style = s;
                                }
                            });
                        }
                    }
                });
                let mut size = self.text_size;
                if ui.add(egui::DragValue::new(&mut size).range(8.0..=400.0).suffix(" pt")).changed() {
                    self.text_size = size;
                    self.restyle_selected(|it| {
                        if let Shape::Text { size: s, .. } = &mut it.shape {
                            *s = size;
                        }
                    });
                }
            }
            Tool::Redact => {
                let modes = [
                    (RedactMode::Pixelate, "Pixelate"),
                    (RedactMode::SecureBlur, "Secure blur"),
                    (RedactMode::Blur, "Smooth blur"),
                    (RedactMode::Solid, "Solid"),
                ];
                let current = modes.iter().find(|m| m.0 == self.redact_mode).map_or("Pixelate", |m| m.1);
                egui::ComboBox::from_id_salt("redact-mode").selected_text(current).width(100.0).show_ui(ui, |ui| {
                    for (m, l) in modes {
                        if ui.selectable_label(self.redact_mode == m, l).clicked() {
                            self.redact_mode = m;
                            self.restyle_selected(|it| {
                                if let Shape::Redact { mode, .. } = &mut it.shape {
                                    *mode = m;
                                }
                            });
                        }
                    }
                });
            }
            Tool::Spotlight => {
                let mut e = self.spotlight_ellipse;
                if ui.toggle_value(&mut e, "Ellipse").changed() {
                    self.spotlight_ellipse = e;
                    self.restyle_selected(|it| {
                        if let Shape::Spotlight { ellipse, .. } = &mut it.shape {
                            *ellipse = e;
                        }
                    });
                }
            }
            _ => {}
        }
    }

    fn background_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        // Brand presets.
        ui.horizontal(|ui| {
            let names: Vec<String> = self.settings.presets.iter().map(|p| p.name.clone()).collect();
            egui::ComboBox::from_id_salt("preset").selected_text("Brand preset").width(150.0).show_ui(ui, |ui| {
                for (i, name) in names.iter().enumerate() {
                    if ui.selectable_label(false, name).clicked() {
                        let bg = self.settings.presets[i].background.clone();
                        self.checkpoint();
                        self.doc.background = Some(bg);
                        self.touch();
                    }
                }
                if names.is_empty() {
                    ui.label(RichText::new("No presets yet").weak());
                }
            });
            ui.menu_button(icon::TRASH, |ui| {
                let mut remove = None;
                for p in &self.settings.presets {
                    if ui.button(&p.name).clicked() {
                        remove = Some(p.name.clone());
                    }
                }
                if let Some(name) = remove {
                    self.update_settings(|s| s.presets.retain(|p| p.name != name));
                }
            })
            .response
            .on_hover_text("Delete a preset");
        });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.preset_name).hint_text("Preset name").desired_width(150.0));
            let can = self.doc.background.is_some() && !self.preset_name.trim().is_empty();
            if ui.add_enabled(can, egui::Button::new(icon::PLUS)).on_hover_text("Save current background as a preset").clicked()
                && let Some(background) = self.doc.background.clone()
            {
                let name = self.preset_name.trim().to_owned();
                let saved = self.update_settings(|s| {
                    s.presets.retain(|p| p.name != name);
                    s.presets.push(Preset { name, background });
                });
                if saved {
                    self.preset_name.clear();
                    self.flash("Preset saved");
                }
            }
        });
        ui.add_space(4.0);
        if ui.add_sized([ui.available_width(), 28.0], egui::Button::selectable(self.doc.background.is_none(), "None")).clicked() {
            self.checkpoint();
            self.doc.background = None;
            self.touch();
        }
        let fill = self.doc.background.as_ref().map(|b| b.fill.clone());
        ui.add_space(4.0);
        ui.label(RichText::new("Gradients").strong());
        egui::Grid::new("gradients").spacing(vec2(6.0, 6.0)).show(ui, |ui| {
            for (i, g) in GRADIENTS.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
                paint_gradient(ui.painter(), r, i);
                if fill == Some(BgFill::Gradient(i)) {
                    ui.painter().rect_stroke(r.expand(2.0), 10, Stroke::new(2.5, theme::ACCENT), StrokeKind::Outside);
                }
                if resp.on_hover_text(g.name).clicked() {
                    self.set_fill(BgFill::Gradient(i));
                }
                if i % 4 == 3 {
                    ui.end_row();
                }
            }
        });
        ui.add_space(6.0);
        ui.label(RichText::new("Plain color").strong());
        ui.horizontal_wrapped(|ui| {
            for c in [
                [255, 255, 255, 255],
                [242, 242, 247, 255],
                [28, 28, 30, 255],
                [10, 132, 255, 255],
                [52, 199, 89, 255],
                [255, 149, 0, 255],
                [255, 59, 48, 255],
                [175, 82, 222, 255],
            ] {
                let col = swatch_color(c);
                let (r, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                ui.painter().circle_filled(r.center(), 11.0, col);
                ui.painter().circle_stroke(r.center(), 11.0, ui.visuals().widgets.noninteractive.bg_stroke);
                if resp.clicked() {
                    self.set_fill(BgFill::Solid(col));
                }
            }
            let mut custom = match &fill {
                Some(BgFill::Solid(c)) => *c,
                _ => Color32::WHITE,
            };
            if ui.color_edit_button_srgba(&mut custom).changed() {
                self.set_fill(BgFill::Solid(custom));
            }
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(fill == Some(BgFill::Blurred), format!("{} Blurred", icon::DROP)).clicked() {
                self.set_fill(BgFill::Blurred);
            }
            if ui.selectable_label(fill == Some(BgFill::Transparent), format!("{} Clear", icon::CHECKERBOARD)).clicked() {
                self.set_fill(BgFill::Transparent);
            }
            if ui.button(format!("{} Image…", icon::IMAGE)).clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp"]).pick_file()
            {
                let r = image::open(&path).map_err(anyhow::Error::from);
                if let Some(img) = self.report(r) {
                    self.set_fill(BgFill::Image(Arc::new(img.into_rgba8())));
                }
            }
        });
        let Some(bg) = self.doc.background.clone() else { return };
        ui.add_space(8.0);
        let mut b = bg.clone();
        let mut changed = false;
        egui::Grid::new("bg-settings").num_columns(2).spacing(vec2(8.0, 8.0)).show(ui, |ui| {
            ui.label("Padding");
            changed |= ui.add(egui::Slider::new(&mut b.padding, 0.0..=400.0).show_value(false)).changed();
            ui.end_row();
            ui.label("Corners");
            changed |= ui.add(egui::Slider::new(&mut b.radius, 0.0..=80.0).show_value(false)).changed();
            ui.end_row();
            ui.label("Shadow");
            changed |= ui.add(egui::Slider::new(&mut b.shadow, 0.0..=1.0).show_value(false)).changed();
            ui.end_row();
            ui.label("Ratio");
            egui::ComboBox::from_id_salt("aspect").selected_text(b.aspect.label()).width(120.0).show_ui(ui, |ui| {
                for (a, l) in Aspect::PRESETS {
                    changed |= ui.selectable_value(&mut b.aspect, a, l).changed();
                }
            });
            ui.end_row();
            ui.label("Align");
            egui::Grid::new("align").spacing(vec2(2.0, 2.0)).show(ui, |ui| {
                for row in [egui::Align::Min, egui::Align::Center, egui::Align::Max] {
                    for col in [egui::Align::Min, egui::Align::Center, egui::Align::Max] {
                        let a = egui::Align2([col, row]);
                        let on = b.align == a;
                        let glyph = RichText::new(if on { icon::RADIO_BUTTON } else { icon::CIRCLE }).color(if on {
                            theme::ACCENT
                        } else {
                            ui.visuals().weak_text_color()
                        });
                        if ui.add(egui::Button::new(glyph).small().frame(false)).clicked() {
                            b.align = a;
                            changed = true;
                        }
                    }
                    ui.end_row();
                }
            });
            ui.end_row();
        });
        changed |= ui.checkbox(&mut b.auto_balance, "Auto-balance").on_hover_text("Even out uneven margins around the content").changed();
        if changed {
            if !ui.ctx().input(|i| i.pointer.any_down()) || self.undo.last().is_none_or(|s| s.background.as_ref() != Some(&bg)) {
                self.checkpoint();
            }
            self.doc.background = Some(b);
            self.touch();
        }
    }

    fn set_fill(&mut self, fill: BgFill) {
        self.checkpoint();
        let mut bg = self.doc.background.clone().unwrap_or_default();
        bg.fill = fill;
        self.doc.background = Some(bg);
        self.touch();
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let canvas = ui.max_rect();
        let checker = ui.visuals().extreme_bg_color;
        ui.painter().rect_filled(canvas, 0, checker);
        self.render_if_needed(&ctx, canvas);
        let resp = ui.allocate_rect(canvas, Sense::click_and_drag());
        let view = self.view;
        if let Some(tex) = &self.tex {
            let shadow = egui::epaint::Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(50) };
            ui.painter().add(shadow.as_shape(view.img_rect, CornerRadius::ZERO));
            ui.painter().image(tex.id(), view.img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            // Keeps dark screenshots distinguishable from the dark canvas.
            ui.painter().rect_stroke(view.img_rect, 0, ui.visuals().widgets.noninteractive.bg_stroke, StrokeKind::Outside);
        }

        // Zoom with Ctrl/Cmd + wheel or pinch; pan with the wheel when zoomed in.
        if resp.hovered() {
            let (zoom_delta, scroll, cmd) = ctx.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.modifiers.command));
            if zoom_delta != 1.0 {
                self.zoom = (self.zoom * zoom_delta).clamp(0.1, 16.0);
            } else if !cmd && self.zoom > 1.0 && scroll != Vec2::ZERO {
                self.pan += scroll;
            }
        }
        for f in ctx.input(|i| i.raw.dropped_files.clone()) {
            let r = image::open(f.path()).map_err(anyhow::Error::from);
            if let Some(img) = self.report(r) {
                self.add_image_item(img.into_rgba8());
            }
        }

        if self.crop.is_some() {
            self.crop_interaction(ui, &resp);
            return;
        }
        let pointer = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
        let tol = 6.0 / view.ppb();
        let shift = ctx.input(|i| i.modifiers.shift);
        let middle = ctx.input(|i| i.pointer.middle_down());

        if self.picking_color {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
            if resp.clicked()
                && let Some(p) = pointer
            {
                let b = view.to_base(p);
                if b.x >= 0.0 && b.y >= 0.0 && (b.x as u32) < self.doc.base.width() && (b.y as u32) < self.doc.base.height() {
                    let [r, g, bb, _] = self.doc.base.get_pixel(b.x as u32, b.y as u32).0;
                    self.color = Color32::from_rgb(r, g, bb);
                    self.restyle_selected(|it| it.color = Color32::from_rgb(r, g, bb));
                }
                self.picking_color = false;
            }
            return;
        }

        if resp.drag_started() {
            self.commit_text();
            let start = ctx.input(|i| i.pointer.press_origin()).unwrap_or_default();
            let b = view.to_base(start);
            let handle = self.selected.and_then(|i| {
                let it = self.doc.items.get(i)?;
                it.handles().iter().position(|h| h.distance(b) <= tol * 1.5).map(|h| (i, h))
            });
            self.drag = if middle {
                Some(EditDrag::Pan { last: start })
            } else if let Some((index, handle)) = handle {
                self.checkpoint();
                Some(EditDrag::Handle { index, handle })
            } else if self.tool == Tool::Select {
                let hit = self.doc.items.iter().rposition(|it| it.hit(b, tol));
                self.selected = hit;
                hit.map(|index| {
                    self.checkpoint();
                    EditDrag::Move { index, last: b }
                })
            } else if let Some(shape) = self.new_shape(b) {
                self.checkpoint();
                let mut item = self.new_item(shape);
                if self.tool == Tool::Redact && self.redact_mode == RedactMode::Solid {
                    item.color = Color32::BLACK;
                }
                if self.tool == Tool::Highlighter && self.color == Color32::from_rgb(255, 59, 48) {
                    item.color = Color32::from_rgb(255, 214, 10);
                }
                self.doc.items.push(item);
                self.selected = None;
                Some(EditDrag::Create { index: self.doc.items.len() - 1, start: b })
            } else {
                None
            };
        }

        if let (Some(drag), Some(p)) = (self.drag, resp.interact_pointer_pos()) {
            let b = view.to_base(p);
            match drag {
                EditDrag::Pan { last } => {
                    self.pan += p - last;
                    self.drag = Some(EditDrag::Pan { last: p });
                }
                EditDrag::Move { index, last } => {
                    if let Some(it) = self.doc.items.get_mut(index) {
                        it.translate(b - last);
                        self.drag = Some(EditDrag::Move { index, last: b });
                        self.touch();
                    }
                }
                EditDrag::Handle { index, handle } => {
                    if let Some(it) = self.doc.items.get_mut(index) {
                        it.set_handle(handle, b, shift);
                        self.touch();
                    }
                }
                EditDrag::Create { index, start } => {
                    if let Some(it) = self.doc.items.get_mut(index) {
                        update_created(it, start, b, shift);
                        self.touch();
                    }
                }
            }
        }

        if resp.drag_stopped() {
            if let Some(EditDrag::Create { index, .. }) = self.drag
                && let Some(it) = self.doc.items.get_mut(index)
            {
                if let Shape::Pen { pts } = &mut it.shape {
                    *pts = smooth(pts);
                }
                let r = it.bounds();
                let tiny = match &it.shape {
                    Shape::Pen { .. } | Shape::Highlighter { .. } => false,
                    _ => r.width().max(r.height()) < 4.0 + it.width,
                };
                if tiny {
                    self.doc.items.remove(index);
                    self.undo.pop();
                } else {
                    self.selected = Some(index);
                }
                self.touch();
            }
            self.drag = None;
        }

        if resp.clicked()
            && let Some(p) = pointer
        {
            let b = view.to_base(p);
            let hit = self.doc.items.iter().rposition(|it| it.hit(b, tol));
            match self.tool {
                Tool::Text => {
                    self.commit_text();
                    match hit.filter(|&i| matches!(self.doc.items[i].shape, Shape::Text { .. })) {
                        Some(i) => self.start_text_edit(i),
                        None => {
                            self.checkpoint();
                            let size = self.text_size;
                            let pos = b - vec2(0.0, size * 0.6);
                            let item = self.new_item(Shape::Text { pos, text: String::new(), size, style: self.text_style });
                            self.doc.items.push(item);
                            self.start_text_edit(self.doc.items.len() - 1);
                        }
                    }
                }
                Tool::Counter => {
                    self.checkpoint();
                    let n = self.doc.next_counter();
                    let item = self.new_item(Shape::Counter { center: b, n });
                    self.doc.items.push(item);
                    self.selected = Some(self.doc.items.len() - 1);
                    self.touch();
                }
                _ => {
                    self.commit_text();
                    self.selected = hit;
                }
            }
        }
        if resp.double_clicked()
            && let Some(i) = self.selected
            && matches!(self.doc.items.get(i).map(|it| &it.shape), Some(Shape::Text { .. }))
        {
            self.start_text_edit(i);
        }

        // Hover cursor feedback.
        if let Some(p) = resp.hover_pos() {
            let b = view.to_base(p);
            let over_handle = self.selected.and_then(|i| self.doc.items.get(i)).is_some_and(|it| it.handles().iter().any(|h| h.distance(b) <= tol * 1.5));
            let icon = if over_handle {
                egui::CursorIcon::Grab
            } else if self.tool == Tool::Select {
                if self.doc.items.iter().any(|it| it.hit(b, tol)) { egui::CursorIcon::Move } else { egui::CursorIcon::Default }
            } else if self.tool == Tool::Text {
                egui::CursorIcon::Text
            } else {
                egui::CursorIcon::Crosshair
            };
            ctx.set_cursor_icon(icon);
        }

        self.draw_selection(ui);
        self.text_editor(ui);
    }

    fn draw_selection(&self, ui: &egui::Ui) {
        let Some(it) = self.selected.and_then(|i| self.doc.items.get(i)) else { return };
        if self.editing.is_some() {
            return;
        }
        let view = self.view;
        let b = it.bounds();
        let r = Rect::from_min_max(view.to_screen(b.min), view.to_screen(b.max)).expand(3.0);
        let p = ui.painter();
        p.rect_stroke(r, 4, Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.8)), StrokeKind::Outside);
        for h in it.handles() {
            let s = view.to_screen(h);
            p.circle_filled(s, 5.0, Color32::WHITE);
            p.circle_stroke(s, 5.0, Stroke::new(1.5, theme::ACCENT));
        }
    }

    fn text_editor(&mut self, ui: &mut egui::Ui) {
        let Some((i, mut text)) = self.editing.clone() else { return };
        let Some(Item { shape: Shape::Text { pos, size, .. }, color, .. }) = self.doc.items.get(i).cloned() else {
            self.editing = None;
            return;
        };
        let view = self.view;
        let at = view.to_screen(pos);
        let font = FontId::new(size * view.ppb(), FontFamily::Name(fonts::ANNOTATION_FAMILY.into()));
        let id = egui::Id::new("text-edit");
        let out = egui::Area::new(egui::Id::new("text-area")).fixed_pos(at).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
            let edit = egui::TextEdit::multiline(&mut text)
                .id(id)
                .font(font)
                .text_color(color)
                .frame(egui::Frame::NONE)
                .desired_rows(1)
                .desired_width(f32::INFINITY)
                .margin(Vec2::ZERO);
            ui.add(edit)
        });
        let resp = out.inner;
        if self.focus_text {
            resp.request_focus();
            self.focus_text = false;
        }
        ui.painter().rect_stroke(resp.rect.expand(4.0), 4, Stroke::new(1.0, theme::ACCENT), StrokeKind::Outside);
        self.editing = Some((i, text));
        if resp.lost_focus() {
            self.commit_text();
        }
    }

    fn crop_interaction(&mut self, ui: &mut egui::Ui, resp: &egui::Response) {
        let ctx = ui.ctx().clone();
        let view = self.view;
        let full = self.doc.full_rect();
        let tol = 10.0 / view.ppb();
        let snap = 8.0 / view.ppb();
        let Some(crop) = &mut self.crop else { return };
        let snap1 = |v: f32, lo: f32, hi: f32| {
            let v = v.clamp(lo, hi);
            if (v - lo).abs() < snap {
                lo
            } else if (v - hi).abs() < snap {
                hi
            } else {
                v
            }
        };
        let snap_pt = |p: Pos2| pos2(snap1(p.x, full.min.x, full.max.x), snap1(p.y, full.min.y, full.max.y));
        let corners = |r: Rect| [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
        if resp.drag_started() {
            let b = view.to_base(ctx.input(|i| i.pointer.press_origin()).unwrap_or_default());
            crop.drag = Some(if let Some(i) = corners(crop.rect).iter().position(|c| c.distance(b) < tol) {
                CropDrag::Corner(i)
            } else if crop.rect.contains(b) && crop.rect != full {
                // A full-image crop can't move; dragging inside it draws a new one instead.
                CropDrag::Move(b)
            } else {
                CropDrag::New(snap_pt(b))
            });
        }
        if let (Some(d), Some(p)) = (crop.drag, resp.interact_pointer_pos()) {
            let b = view.to_base(p);
            let ratio = crop.aspect.map(|(w, h)| w as f32 / h as f32);
            let fix = |anchor: Pos2, q: Pos2| ratio.map_or(q, |r| constrain_aspect(anchor, q, r));
            match d {
                CropDrag::New(a) => crop.rect = Rect::from_two_pos(a, fix(a, snap_pt(b))).intersect(full),
                CropDrag::Corner(i) => {
                    let anchor = corners(crop.rect)[(i + 2) % 4];
                    crop.rect = Rect::from_two_pos(anchor, fix(anchor, snap_pt(b))).intersect(full);
                    crop.drag = Some(CropDrag::Corner(match corners(crop.rect).iter().position(|c| *c == anchor) {
                        Some(ai) => (ai + 2) % 4,
                        None => i,
                    }));
                }
                CropDrag::Move(last) => {
                    let size = crop.rect.size();
                    let max_min = (full.max - size).max(full.min);
                    let min = (crop.rect.min + (b - last)).clamp(full.min, max_min);
                    crop.rect = Rect::from_min_size(snap_pt(min), size);
                    crop.drag = Some(CropDrag::Move(b));
                }
            }
        }
        if resp.drag_stopped() {
            crop.drag = None;
        }
        let r = Rect::from_min_max(view.to_screen(crop.rect.min), view.to_screen(crop.rect.max));
        let p = ui.painter();
        let outer = view.img_rect;
        let dim = Color32::from_black_alpha(120);
        p.rect_filled(Rect::from_min_max(outer.min, pos2(outer.max.x, r.min.y)), 0, dim);
        p.rect_filled(Rect::from_min_max(pos2(outer.min.x, r.max.y), outer.max), 0, dim);
        p.rect_filled(Rect::from_min_max(pos2(outer.min.x, r.min.y), pos2(r.min.x, r.max.y)), 0, dim);
        p.rect_filled(Rect::from_min_max(pos2(r.max.x, r.min.y), pos2(outer.max.x, r.max.y)), 0, dim);
        p.rect_stroke(r, 0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside);
        for t in [1.0 / 3.0, 2.0 / 3.0] {
            let thin = Stroke::new(0.5, Color32::from_white_alpha(120));
            p.line_segment([pos2(r.left() + r.width() * t, r.top()), pos2(r.left() + r.width() * t, r.bottom())], thin);
            p.line_segment([pos2(r.left(), r.top() + r.height() * t), pos2(r.right(), r.top() + r.height() * t)], thin);
        }
        for c in corners(r) {
            p.circle_filled(c, 6.0, Color32::WHITE);
            p.circle_stroke(c, 6.0, Stroke::new(1.5, theme::ACCENT));
        }
        let label = format!("{} × {}", crop.rect.width().round() as u32, crop.rect.height().round() as u32);
        p.text(r.center_top() - vec2(0.0, 12.0), Align2::CENTER_CENTER, label, FontId::proportional(12.0), Color32::WHITE);
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
    }

    fn crop_bar(&mut self, ui: &mut egui::Ui) {
        let full = self.doc.full_rect();
        let Some(aspect) = self.crop.as_ref().map(|c| c.aspect) else { return };
        let mut action = None;
        ui.horizontal_centered(|ui| {
            ui.label(RichText::new(format!("{}  Crop", icon::CROP)).strong());
            let presets: [(Option<(u32, u32)>, &str); 6] =
                [(None, "Freeform"), (Some((1, 1)), "1:1"), (Some((4, 3)), "4:3"), (Some((3, 2)), "3:2"), (Some((16, 9)), "16:9"), (Some((9, 16)), "9:16")];
            for (a, l) in presets {
                if ui.selectable_label(aspect == a, l).clicked() {
                    action = Some(CropAction::Aspect(a));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("Done").color(Color32::WHITE)).fill(theme::ACCENT)).clicked() {
                    action = Some(CropAction::Done);
                }
                if ui.button("Cancel").clicked() {
                    action = Some(CropAction::Cancel);
                }
                if ui.button("Reset").clicked() {
                    action = Some(CropAction::Reset);
                }
            });
        });
        match action {
            Some(CropAction::Done) => self.apply_crop(),
            Some(CropAction::Cancel) => {
                self.crop = None;
                self.touch();
            }
            Some(CropAction::Reset) => {
                if let Some(c) = &mut self.crop {
                    c.rect = full;
                }
            }
            Some(CropAction::Aspect(a)) => {
                if let Some(c) = &mut self.crop {
                    c.aspect = a;
                    if let Some((w, h)) = a {
                        let end = constrain_aspect(c.rect.min, c.rect.max, w as f32 / h as f32);
                        c.rect = Rect::from_two_pos(c.rect.min, end).intersect(full);
                    }
                }
            }
            None => {}
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            let size = render::output_size(&self.doc, &RenderOpts::default());
            let k = self.export_percent as f32 / 100.0;
            ui.label(RichText::new(format!("{} × {} px", (size.x * k).round(), (size.y * k).round())).weak());
            ui.separator();
            ui.label(RichText::new(format!("{:.0}%", self.zoom * 100.0)).weak()).on_hover_text("Ctrl/Cmd + wheel to zoom, Ctrl/Cmd+0 to fit");
            if let Some((label, _)) = &self.task {
                ui.separator();
                ui.spinner();
                ui.label(*label);
            }
            if let Some((msg, at)) = &self.status {
                if at.elapsed() < Duration::from_secs(5) {
                    ui.separator();
                    ui.label(msg);
                } else {
                    self.status = None;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut fmt = self.settings.export_format;
                egui::ComboBox::from_id_salt("format").selected_text(fmt.label()).width(70.0).show_ui(ui, |ui| {
                    for f in ExportFormat::ALL {
                        ui.selectable_value(&mut fmt, f, f.label());
                    }
                });
                if fmt != self.settings.export_format {
                    self.update_settings(|s| s.export_format = fmt);
                }
                ui.label("Format");
                egui::ComboBox::from_id_salt("scale").selected_text(format!("{}%", self.export_percent)).width(70.0).show_ui(ui, |ui| {
                    for p in [25, 50, 75, 100, 150, 200] {
                        ui.selectable_value(&mut self.export_percent, p, format!("{p}%"));
                    }
                });
                ui.label("Export size");
            });
        });
    }
}

fn rand_seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.as_nanos() as u64)
}

/// Update a shape being drawn from `start` to `b`; Shift squares boxes and snaps lines to 45°.
fn update_created(it: &mut Item, start: Pos2, b: Pos2, shift: bool) {
    let boxed = if shift { constrain_aspect(start, b, 1.0) } else { b };
    let snapped = if shift { snap45(start, b) } else { b };
    match &mut it.shape {
        Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } | Shape::Redact { rect, .. } | Shape::Spotlight { rect, .. } => {
            *rect = Rect::from_two_pos(start, boxed)
        }
        Shape::Line { b: end, .. } | Shape::Arrow { b: end, .. } => *end = snapped,
        Shape::Pen { pts } | Shape::Highlighter { pts } => {
            if shift && !pts.is_empty() {
                // Straight stroke from the first point.
                pts.truncate(1);
                pts.push(snap45(pts[0], b));
            } else if pts.last().is_none_or(|l| l.distance(b) > 1.0) {
                pts.push(b);
            }
        }
        _ => {}
    }
}

fn snap45(a: Pos2, b: Pos2) -> Pos2 {
    let d = b - a;
    let angle = (d.y.atan2(d.x) / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
    a + Vec2::angled(angle) * d.length()
}

/// Two rounds of Chaikin corner cutting: pleasant curves from shaky input.
fn smooth(pts: &[Pos2]) -> Vec<Pos2> {
    let mut cur = pts.to_vec();
    for _ in 0..2 {
        if cur.len() < 3 {
            break;
        }
        let mut next = vec![cur[0]];
        for w in cur.windows(2) {
            next.push(w[0] + (w[1] - w[0]) * 0.25);
            next.push(w[0] + (w[1] - w[0]) * 0.75);
        }
        next.extend(cur.last().copied());
        cur = next;
    }
    cur
}

fn paint_gradient(painter: &egui::Painter, r: Rect, index: usize) {
    let Some(g) = GRADIENTS.get(index) else { return };
    let c = |i: usize| {
        let [r, gg, b] = g.stops[i.min(g.stops.len() - 1)];
        Color32::from_rgb(r, gg, b)
    };
    let first = c(0);
    let last = c(g.stops.len() - 1);
    let mid = if g.stops.len() > 2 { c(1) } else { first.lerp_to_gamma(last, 0.5) };
    let mut mesh = Mesh::default();
    // Diagonal gradient: top-left = first stop, bottom-right = last.
    for (p, col) in [(r.left_top(), first), (r.right_top(), mid), (r.right_bottom(), last), (r.left_bottom(), mid)] {
        mesh.colored_vertex(p, col);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(EShape::mesh(mesh));
    painter.rect_stroke(r, 10, Stroke::new(1.0, Color32::from_black_alpha(25)), StrokeKind::Inside);
}

impl eframe::App for Editor {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        if let Some((_, rx)) = &self.task
            && let Ok(result) = rx.try_recv()
        {
            self.task = None;
            match result {
                Ok(msg) => self.flash(msg),
                Err(e) => self.flash(format!("{e:#}")),
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            self.commit_text();
            if self.revision != self.saved_revision && self.history_id.is_some() {
                // History captures autosave; if that fails, keep the window and offer the save dialog.
                if !self.sync_history() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    self.confirm_close = true;
                }
            } else if self.revision != self.saved_revision && self.project_path.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm_close = true;
            }
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::V))
            && self.editing.is_none()
            && let Ok(img) = share::clipboard_image()
        {
            self.add_image_item(img);
        }
        self.handle_shortcuts(ctx);

        egui::Panel::top("toolbar").exact_size(48.0).show(root, |ui| if self.crop.is_some() { self.crop_bar(ui) } else { self.toolbar(ui) });
        if self.crop.is_none() {
            // Per-tool options get their own row so the toolbar never overflows into the actions.
            egui::Panel::top("options").exact_size(36.0).show(root, |ui| {
                ui.horizontal_centered(|ui| self.style_controls(ui));
            });
        }
        egui::Panel::bottom("status").exact_size(32.0).show(root, |ui| self.status_bar(ui));
        if self.show_background && self.crop.is_none() {
            egui::Panel::left("background").exact_size(232.0).resizable(false).show(root, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.background_panel(ui));
            });
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(root, |ui| self.canvas(ui));

        if self.confirm_close {
            egui::Modal::new(egui::Id::new("confirm-close")).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.heading("Save changes?");
                ui.label("Your edits to this project are not saved.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Save").color(Color32::WHITE)).fill(theme::ACCENT)).clicked() {
                        self.save();
                        // A failed save has already shown its error; keep the window and the edits.
                        if self.saved_revision == self.revision {
                            self.allow_close = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                    if ui.button("Don't Save").clicked() {
                        self.allow_close = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_close = false;
                    }
                });
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_mapping_round_trips() {
        let v = View {
            img_rect: Rect::from_min_size(pos2(100.0, 50.0), vec2(400.0, 300.0)),
            region: Rect::from_min_size(pos2(20.0, 10.0), vec2(800.0, 600.0)),
            offset: vec2(0.0, 0.0),
            out_scale: 0.5,
            out_size: vec2(400.0, 300.0),
        };
        let b = pos2(321.0, 123.0);
        assert!(v.to_base(v.to_screen(b)).distance(b) < 1e-3);
        assert!((v.ppb() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn shift_snaps_lines_to_45_degrees() {
        let p = snap45(pos2(0.0, 0.0), pos2(10.0, 9.0));
        assert!((p.x - p.y).abs() < 1e-3);
    }

    #[test]
    fn smoothing_keeps_endpoints() {
        let pts = vec![pos2(0.0, 0.0), pos2(10.0, 0.0), pos2(10.0, 10.0), pos2(20.0, 10.0)];
        let s = smooth(&pts);
        assert_eq!(s.first(), pts.first());
        assert_eq!(s.last(), pts.last());
        assert!(s.len() > pts.len());
    }
}
