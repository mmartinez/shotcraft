//! Capture history browser: filter, restore, copy, annotate, pin or delete past captures.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Color32, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, vec2};
use egui_phosphor::regular as icon;
use image::RgbaImage;

use shotcraft_engine::config::Settings;
use shotcraft_engine::history::{self, Entry};
use shotcraft_engine::share;

use crate::theme;

const THUMB: u32 = 360;
const FILTERS: [(&str, &[&str]); 5] =
    [("All", &[]), ("Area", &["area"]), ("Window", &["window"]), ("Fullscreen", &["fullscreen"]), ("Scrolling", &["scrolling"])];

struct HistoryApp {
    entries: Vec<Entry>,
    filter: usize,
    selected: Option<String>,
    thumbs: HashMap<String, TextureHandle>,
    requested: std::collections::HashSet<String>,
    tx: Sender<(String, Option<RgbaImage>)>,
    rx: Receiver<(String, Option<RgbaImage>)>,
    settings: Settings,
    status: Option<String>,
}

pub fn run() -> anyhow::Result<()> {
    let settings = Settings::load();
    history::prune(settings.history_days);
    let builder = egui::ViewportBuilder::default().with_title("Capture History").with_inner_size(vec2(1000.0, 640.0)).with_min_inner_size(vec2(640.0, 400.0));
    theme::run("shotcraft-history", builder, move |_| {
        let (tx, rx) = channel();
        HistoryApp {
            entries: history::list(),
            filter: 0,
            selected: None,
            thumbs: HashMap::new(),
            requested: Default::default(),
            tx,
            rx,
            settings,
            status: None,
        }
    })
}

impl HistoryApp {
    fn visible(&self) -> Vec<Entry> {
        let kinds = FILTERS[self.filter].1;
        self.entries.iter().filter(|e| kinds.is_empty() || kinds.contains(&e.kind.as_str())).cloned().collect()
    }

    fn request_thumb(&mut self, ctx: &egui::Context, id: &str) {
        if self.requested.insert(id.to_owned()) {
            let (tx, id, ctx) = (self.tx.clone(), id.to_owned(), ctx.clone());
            std::thread::spawn(move || {
                let img = history::thumbnail(&id, THUMB).ok();
                let _ = tx.send((id, img));
                ctx.request_repaint();
            });
        }
    }

    fn act(&mut self, id: &str, what: &str) {
        let result: anyhow::Result<()> = (|| {
            match what {
                "restore" => share::spawn(["add-quick-access-overlay", "--history", id]),
                "annotate" => share::spawn(["open-annotate", "--history", id]),
                "pin" => share::spawn(["pin", "--history", id]),
                "copy" => {
                    share::copy_image(&history::load_image(id)?)?;
                    self.status = Some("Copied to clipboard".into());
                }
                "save" => {
                    let path = share::save_default(&history::load_image(id)?, &self.settings)?;
                    self.status = Some(format!("Saved {}", path.display()));
                }
                "delete" => {
                    history::delete(id)?;
                    self.entries.retain(|e| e.id != id);
                    self.thumbs.remove(id);
                    self.selected = None;
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.status = Some(format!("{e:#}"));
        }
    }
}

impl eframe::App for HistoryApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        while let Ok((id, img)) = self.rx.try_recv() {
            if let Some(img) = img {
                let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                self.thumbs.insert(id.clone(), ctx.load_texture(format!("thumb-{id}"), ci, TextureOptions::LINEAR));
            }
        }
        egui::Panel::top("filters").exact_size(56.0).show(root, |ui| {
            ui.horizontal_centered(|ui| {
                ui.heading(format!("{}  History", icon::CLOCK_COUNTER_CLOCKWISE));
                ui.add_space(16.0);
                for (i, (label, _)) in FILTERS.iter().enumerate() {
                    let on = self.filter == i;
                    let text = RichText::new(*label).color(if on { Color32::WHITE } else { ui.visuals().text_color() });
                    if ui
                        .add(egui::Button::new(text).fill(if on { theme::ACCENT } else { ui.visuals().widgets.inactive.weak_bg_fill }).corner_radius(14))
                        .clicked()
                    {
                        self.filter = i;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("Kept for {} days", self.settings.history_days)).weak());
                });
            });
        });
        let selected = self.selected.clone();
        egui::Panel::bottom("actions").exact_size(52.0).show(root, |ui| {
            ui.horizontal_centered(|ui| {
                ui.add_enabled_ui(selected.is_some(), |ui| {
                    let id = selected.clone().unwrap_or_default();
                    let restore = egui::Button::new(RichText::new(format!("{}  Restore", icon::ARROW_U_UP_LEFT)).color(Color32::WHITE)).fill(theme::ACCENT);
                    if ui.add(restore).on_hover_text("Open in a Quick Access Overlay").clicked() {
                        self.act(&id, "restore");
                    }
                    for (what, ic, label) in [
                        ("copy", icon::COPY, "Copy"),
                        ("save", icon::FLOPPY_DISK, "Save"),
                        ("annotate", icon::PENCIL_SIMPLE, "Annotate"),
                        ("pin", icon::PUSH_PIN, "Pin"),
                        ("delete", icon::TRASH, "Delete"),
                    ] {
                        if ui.button(format!("{ic}  {label}")).clicked() {
                            self.act(&id, what);
                        }
                    }
                });
                if let Some(s) = &self.status {
                    ui.separator();
                    ui.label(s);
                }
            });
        });
        egui::CentralPanel::default().show(root, |ui| {
            let visible = self.visible();
            if visible.is_empty() {
                ui.centered_and_justified(|ui| ui.label(RichText::new("No captures yet. Take a screenshot to see it here.").weak()));
                return;
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                let card = vec2(220.0, 170.0);
                let cols = ((ui.available_width() + 12.0) / (card.x + 12.0)).floor().max(1.0) as usize;
                egui::Grid::new("history-grid").spacing(vec2(12.0, 12.0)).show(ui, |ui| {
                    for (n, e) in visible.iter().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(card, Sense::click());
                        if ui.is_rect_visible(rect) {
                            self.request_thumb(ctx, &e.id);
                        }
                        let selected = self.selected.as_deref() == Some(e.id.as_str());
                        let p = ui.painter();
                        let img_area = egui::Rect::from_min_size(rect.min, vec2(card.x, card.y - 36.0));
                        p.rect_filled(img_area, 12, ui.visuals().extreme_bg_color);
                        if let Some(tex) = self.thumbs.get(&e.id) {
                            let s = tex.size_vec2();
                            let k = ((img_area.width() - 12.0) / s.x).min((img_area.height() - 12.0) / s.y).min(1.0);
                            let r = egui::Rect::from_center_size(img_area.center(), s * k);
                            egui::Image::new(tex).corner_radius(6).paint_at(ui, r);
                        } else {
                            ui.put(img_area, egui::Spinner::new());
                        }
                        let p = ui.painter();
                        if selected {
                            p.rect_stroke(img_area, 12, Stroke::new(3.0, theme::ACCENT), StrokeKind::Inside);
                        } else if resp.hovered() {
                            p.rect_stroke(img_area, 12, Stroke::new(1.0, ui.visuals().widgets.hovered.bg_stroke.color), StrokeKind::Inside);
                        }
                        let when = chrono::DateTime::from_timestamp(e.created, 0)
                            .map(|t| t.with_timezone(&chrono::Local).format("%b %e, %H:%M").to_string())
                            .unwrap_or_default();
                        p.text(
                            img_area.left_bottom() + vec2(4.0, 10.0),
                            egui::Align2::LEFT_TOP,
                            format!("{}  ·  {} × {}  ·  {}", e.kind, e.width, e.height, when),
                            egui::FontId::proportional(11.5),
                            ui.visuals().weak_text_color(),
                        );
                        if resp.clicked() {
                            self.selected = Some(e.id.clone());
                        }
                        if resp.double_clicked() {
                            self.act(&e.id.clone(), "restore");
                        }
                        let id = e.id.clone();
                        resp.context_menu(|ui| {
                            for (what, label) in
                                [("restore", "Restore"), ("copy", "Copy"), ("save", "Save"), ("annotate", "Annotate"), ("pin", "Pin"), ("delete", "Delete")]
                            {
                                if ui.button(label).clicked() {
                                    self.act(&id, what);
                                }
                            }
                        });
                        if (n + 1) % cols == 0 {
                            ui.end_row();
                        }
                    }
                });
            });
        });
    }
}
