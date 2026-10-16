//! Quick Access Overlay: the floating thumbnail shown after each capture.
//! Copy, save, annotate, pin, upload, or drag it straight into another app.

use std::fs::File;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use egui::{Color32, Key, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2, vec2};
use egui_phosphor::regular as icon;
use image::RgbaImage;

use shotcraft_engine::config::{ExportFormat, QuickAccessSide, Settings, State};
use shotcraft_engine::{capture, history, ocr, share};

use crate::{dialogs, dragout, theme};

const MAX_CARD: Vec2 = vec2(280.0, 180.0);
const GAP: f32 = 12.0;

struct QuickAccess {
    id: String,
    settings: Settings,
    image: RgbaImage,
    tex: Option<TextureHandle>,
    _slot: File,
    shown: Instant,
    last_hover: Instant,
    task: Option<(&'static str, Receiver<anyhow::Result<String>>)>,
    status: Option<(String, Instant)>,
    dropped: Option<Receiver<bool>>,
}

pub fn run(id: String) -> anyhow::Result<()> {
    let settings = Settings::load();
    let image = history::load_image(&id)?;
    let (w, h) = (image.width() as f32, image.height() as f32);
    let k = (MAX_CARD.x / w).min(MAX_CARD.y / h).min(1.0);
    let card = vec2((w * k).max(140.0), (h * k).max(90.0)).round();
    let (slot, lock) = theme::claim_slot("quick")?;
    let screen = capture::primary_logical_rect();
    let x = match settings.quick_access_side {
        QuickAccessSide::Left => screen.left() + 20.0,
        QuickAccessSide::Right => screen.right() - 20.0 - card.x,
    };
    let y = screen.bottom() - 70.0 - (card.y + GAP) * (slot as f32 + 1.0);
    let builder = theme::floating("ShotCraft quick access", egui::pos2(x, y.max(screen.top())), card).with_active(false);
    theme::run("shotcraft-quick", builder, move |_| QuickAccess {
        id,
        settings,
        image,
        tex: None,
        _slot: lock,
        shown: Instant::now(),
        last_hover: Instant::now(),
        task: None,
        status: None,
        dropped: None,
    })
}

impl QuickAccess {
    fn close(&self, ctx: &egui::Context, recently_closed: bool) {
        if recently_closed {
            let id = self.id.clone();
            State::update(|s| {
                s.recently_closed.retain(|x| *x != id);
                s.recently_closed.push(id);
                let excess = s.recently_closed.len().saturating_sub(20);
                s.recently_closed.drain(..excess);
            });
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn done(&mut self, ctx: &egui::Context, message: &str) {
        if self.settings.close_quick_access_after_action {
            self.close(ctx, false);
        } else {
            self.status = Some((message.to_owned(), Instant::now()));
        }
    }

    fn fail(&mut self, err: impl Into<anyhow::Error>) {
        self.status = Some((format!("{:#}", err.into()), Instant::now()));
    }

    fn copy(&mut self, ctx: &egui::Context) {
        match share::copy_image(&self.image) {
            Ok(()) => self.done(ctx, "Copied"),
            Err(e) => self.fail(e),
        }
    }

    fn save(&mut self, ctx: &egui::Context) {
        match share::save_default(&self.image, &self.settings) {
            Ok(path) => {
                share::notify("Screenshot saved", &path.display().to_string());
                self.done(ctx, "Saved");
            }
            Err(e) => self.fail(e),
        }
    }

    fn save_as(&mut self, ctx: &egui::Context) {
        match dialogs::save_as(&self.image, &self.settings) {
            Ok(Some(_)) => self.done(ctx, "Saved"),
            Ok(None) => {}
            Err(e) => self.fail(e),
        }
    }

    fn open(&mut self, ctx: &egui::Context, cmd: &str) {
        share::spawn([cmd, "--history", &self.id]);
        self.close(ctx, false);
    }

    fn background(&mut self, label: &'static str, ctx: &egui::Context, job: impl FnOnce(RgbaImage, Settings) -> anyhow::Result<String> + Send + 'static) {
        if self.task.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let (img, settings, ctx2) = (self.image.clone(), self.settings.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(job(img, settings));
            ctx2.request_repaint();
        });
        self.task = Some((label, rx));
    }

    fn upload(&mut self, ctx: &egui::Context) {
        self.background("Uploading…", ctx, |img, settings| {
            let link = share::upload(&img, &settings)?;
            share::copy_text(&link)?;
            share::notify("Link copied", &link);
            Ok("Link copied".into())
        });
    }

    fn copy_text(&mut self, ctx: &egui::Context) {
        self.background("Recognizing text…", ctx, |img, settings| {
            let text = ocr::extract(&img, settings.ocr_keep_linebreaks)?;
            anyhow::ensure!(!text.trim().is_empty(), "no text found");
            share::copy_text(&text)?;
            share::notify("Text copied", &text);
            Ok("Text copied".into())
        });
    }

    fn drag_out(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let (tx, rx) = channel();
        let repaint = ctx.clone();
        let result = (|| {
            let dir = history::drag_dir(&self.id);
            std::fs::create_dir_all(&dir)?;
            let file = dir.join(share::default_file_name(ExportFormat::Png));
            std::fs::copy(history::image_path(&self.id), &file)?;
            let preview = dir.join("preview.png");
            history::thumbnail(&self.id, 160)?.save(&preview)?;
            dragout::start(frame, &file, &preview, move |dropped| {
                let _ = tx.send(dropped);
                repaint.request_repaint();
            })
        })();
        match result {
            Ok(()) => self.dropped = Some(rx),
            Err(e) => self.fail(e),
        }
    }

    fn context_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let item = |ui: &mut egui::Ui, ic: &str, text: &str| ui.button(format!("{ic}  {text}")).clicked();
        if item(ui, icon::COPY, "Copy") {
            self.copy(&ctx);
        }
        if item(ui, icon::FLOPPY_DISK, "Save") {
            self.save(&ctx);
        }
        if item(ui, icon::DOWNLOAD_SIMPLE, "Save As…") {
            self.save_as(&ctx);
        }
        ui.separator();
        if item(ui, icon::PENCIL_SIMPLE, "Annotate") {
            self.open(&ctx, "open-annotate");
        }
        if item(ui, icon::PUSH_PIN, "Pin to Screen") {
            self.open(&ctx, "pin");
        }
        if item(ui, icon::CLOUD_ARROW_UP, "Upload") {
            self.upload(&ctx);
        }
        if item(ui, icon::TEXT_AA, "Copy Text (OCR)") {
            self.copy_text(&ctx);
        }
        ui.separator();
        if item(ui, icon::TRASH, "Delete from History") {
            let _ = history::delete(&self.id);
            self.close(&ctx, false);
        }
        if item(ui, icon::X, "Close") {
            self.close(&ctx, true);
        }
    }
}

impl eframe::App for QuickAccess {
    fn ui(&mut self, root: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        if self.dropped.as_ref().is_some_and(|rx| rx.try_recv() == Ok(true)) {
            self.dropped = None;
            self.done(ctx, "Dropped");
        }
        if let Some((_, rx)) = &self.task
            && let Ok(result) = rx.try_recv()
        {
            self.task = None;
            match result {
                Ok(msg) => self.done(ctx, &msg),
                Err(e) => self.fail(e),
            }
        }
        let tex = self
            .tex
            .get_or_insert_with(|| {
                let img = image::imageops::thumbnail(&self.image, (MAX_CARD.x * 2.0) as u32, (MAX_CARD.y * 2.0) as u32);
                let (w, h) = img.dimensions();
                let k = ((MAX_CARD.x * 2.0) / w as f32).min((MAX_CARD.y * 2.0) / h as f32).min(1.0);
                let img = if k < 1.0 { image::imageops::thumbnail(&img, (w as f32 * k) as u32, (h as f32 * k) as u32) } else { img };
                let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                ctx.load_texture("quick-thumb", ci, TextureOptions::LINEAR)
            })
            .clone();

        let (esc, copy, save) =
            ctx.input(|i| (i.key_pressed(Key::Escape), i.modifiers.command && i.key_pressed(Key::C), i.modifiers.command && i.key_pressed(Key::S)));
        if esc {
            self.close(ctx, true);
        }
        if copy {
            self.copy(ctx);
        }
        if save {
            self.save(ctx);
        }

        egui::CentralPanel::default().frame(egui::Frame::NONE).show(root, |ui| {
            let rect = ui.max_rect().shrink(1.0);
            let card = ui.allocate_rect(rect, Sense::click_and_drag());
            let painter = ui.painter_at(rect.expand(1.0));
            painter.rect_filled(rect, 12, Color32::from_rgb(24, 24, 27));
            // Letterbox the thumbnail inside the card.
            let ts = tex.size_vec2();
            let k = (rect.width() / ts.x).min(rect.height() / ts.y);
            let img_rect = Rect::from_center_size(rect.center(), ts * k);
            egui::Image::new(&tex).corner_radius(12).paint_at(ui, img_rect);
            painter.rect_stroke(rect, 12, Stroke::new(1.0, Color32::from_white_alpha(40)), StrokeKind::Inside);

            let hovered = ui.rect_contains_pointer(rect);
            if hovered {
                self.last_hover = Instant::now();
            }
            if let Some((label, _)) = &self.task {
                painter.rect_filled(rect, 12, Color32::from_black_alpha(150));
                ui.put(Rect::from_center_size(rect.center(), vec2(rect.width(), 40.0)), egui::Spinner::new().size(22.0));
                painter.text(rect.center() + vec2(0.0, 28.0), egui::Align2::CENTER_CENTER, *label, egui::FontId::proportional(13.0), Color32::WHITE);
            } else if hovered {
                painter.rect_filled(rect, 12, Color32::from_black_alpha(120));
                let c = rect.center();
                if ui.put(Rect::from_center_size(c - vec2(0.0, 18.0), vec2(100.0, 30.0)), |ui: &mut egui::Ui| theme::pill_button(ui, "Copy")).clicked() {
                    self.copy(ctx);
                }
                if ui.put(Rect::from_center_size(c + vec2(0.0, 18.0), vec2(100.0, 30.0)), |ui: &mut egui::Ui| theme::pill_button(ui, "Save")).clicked() {
                    self.save(ctx);
                }
                let inset = 20.0;
                if theme::round_button(ui, rect.left_top() + vec2(inset, inset), icon::X, "close").on_hover_text("Close").clicked() {
                    self.close(ctx, true);
                }
                if theme::round_button(ui, rect.right_top() + vec2(-inset, inset), icon::PUSH_PIN, "pin").on_hover_text("Pin to screen").clicked() {
                    self.open(ctx, "pin");
                }
                if theme::round_button(ui, rect.left_bottom() + vec2(inset, -inset), icon::PENCIL_SIMPLE, "edit").on_hover_text("Annotate").clicked() {
                    self.open(ctx, "open-annotate");
                }
                if theme::round_button(ui, rect.right_bottom() + vec2(-inset, -inset), icon::CLOUD_ARROW_UP, "upload").on_hover_text("Upload").clicked() {
                    self.upload(ctx);
                }
            }
            if let Some((msg, at)) = &self.status {
                if at.elapsed() < Duration::from_secs(4) {
                    let r = Rect::from_min_size(rect.left_bottom() + vec2(8.0, -30.0), vec2(rect.width() - 16.0, 22.0));
                    painter.rect_filled(r, 8, theme::HUD_FILL);
                    painter.text(r.center(), egui::Align2::CENTER_CENTER, msg, egui::FontId::proportional(12.0), Color32::WHITE);
                    ctx.request_repaint_after(Duration::from_millis(500));
                } else {
                    self.status = None;
                }
            }
            if card.drag_started() && self.task.is_none() {
                self.drag_out(ctx, frame);
            }
            if card.double_clicked() {
                self.open(ctx, "open-annotate");
            }
            card.context_menu(|ui| self.context_menu(ui));
        });

        let auto = self.settings.quick_access_auto_close;
        if auto > 0 && self.task.is_none() {
            let idle = self.last_hover.max(self.shown).elapsed();
            let limit = Duration::from_secs(auto as u64);
            if idle >= limit {
                self.close(ctx, true);
            } else {
                ctx.request_repaint_after(limit - idle);
            }
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }
}
