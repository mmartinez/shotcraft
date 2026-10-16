//! Look and feel shared by all windows.

use std::fs::File;
use std::sync::Arc;

use egui::{
    Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin, Response, Sense, Stroke, Theme, ThemePreference, Ui, Vec2,
    Visuals, vec2,
};
use shotcraft_engine::config::run_dir;
use shotcraft_engine::doc::fonts;

/// Desktop app id: the window class on X11, the app id on Wayland, and the name of the
/// desktop entry and theme icons in packaging/linux.
pub const APP_ID: &str = "io.github.mmartinez.shotcraft";

pub const ACCENT: Color32 = Color32::from_rgb(10, 132, 255);
/// Errors and warnings (toasts).
pub const WARNING: Color32 = Color32::from_rgb(232, 120, 20);
/// Dark translucent panel used over screenshots (overlay bars, toasts, quick access).
pub const HUD_FILL: Color32 = Color32::from_rgba_premultiplied(24, 24, 27, 236);
pub const HUD_TEXT: Color32 = Color32::from_rgb(235, 235, 240);
pub const HUD_MUTED: Color32 = Color32::from_rgb(160, 160, 168);

pub fn setup(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_visuals_of(Theme::Light, tuned(Visuals::light()));
    ctx.set_visuals_of(Theme::Dark, tuned(Visuals::dark()));
    ctx.options_mut(|o| o.theme_preference = ThemePreference::System);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(10.0, 5.0);
        s.spacing.interact_size.y = 26.0;
        s.interaction.selectable_labels = false;
    });
}

/// Inter for the UI plus the families annotations and headings use, then phosphor icons.
fn install_fonts(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let fallbacks = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for (name, bytes) in [("inter", fonts::INTER_400), ("inter-medium", fonts::INTER_500), ("inter-bold", fonts::INTER_700)] {
        defs.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    defs.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());
    for (family, font) in [(fonts::MEDIUM_FAMILY, "inter-medium"), (fonts::ANNOTATION_FAMILY, "inter-bold")] {
        let mut list = vec![font.to_owned()];
        list.extend(fallbacks.iter().cloned());
        defs.families.insert(FontFamily::Name(family.into()), list);
    }
    egui_phosphor::add_to_fonts(&mut defs, egui_phosphor::Variant::Regular);
    ctx.set_fonts(defs);
}

fn tuned(mut v: Visuals) -> Visuals {
    let r = CornerRadius::same(7);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = r;
    }
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.hyperlink_color = ACCENT;
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    v
}

pub fn hud_frame() -> Frame {
    Frame::new().fill(HUD_FILL).corner_radius(14).inner_margin(Margin::same(8)).stroke(Stroke::new(1.0, Color32::from_white_alpha(28)))
}

/// Square-ish tool button: phosphor icon with an optional caption underneath.
pub fn tool_button(ui: &mut Ui, icon: &str, caption: Option<&str>, selected: bool, hud: bool) -> Response {
    let size = if caption.is_some() { vec2(78.0, 54.0) } else { vec2(32.0, 30.0) };
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let visuals = ui.visuals();
    let (fg, hover_bg) =
        if hud { (HUD_TEXT, Color32::from_white_alpha(22)) } else { (visuals.widgets.inactive.fg_stroke.color, visuals.widgets.hovered.weak_bg_fill) };
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 8, ACCENT);
    } else if resp.hovered() {
        painter.rect_filled(rect, 8, hover_bg);
    }
    let color = if selected { Color32::WHITE } else { fg };
    match caption {
        Some(text) => {
            painter.text(rect.center_top() + vec2(0.0, 17.0), Align2::CENTER_CENTER, icon, FontId::proportional(20.0), color);
            painter.text(rect.center_bottom() - vec2(0.0, 13.0), Align2::CENTER_CENTER, text, FontId::proportional(12.5), color);
        }
        None => {
            painter.text(rect.center(), Align2::CENTER_CENTER, icon, FontId::proportional(18.0), color);
        }
    }
    resp
}

/// Round icon button used on top of screenshots (quick access corners).
pub fn round_button(ui: &mut Ui, center: egui::Pos2, icon: &str, id: &str) -> Response {
    let rect = egui::Rect::from_center_size(center, Vec2::splat(28.0));
    let resp = ui.interact(rect, ui.id().with(id), Sense::click());
    let fill = if resp.hovered() { Color32::from_rgb(60, 60, 66) } else { Color32::from_rgba_premultiplied(30, 30, 34, 230) };
    ui.painter().circle_filled(rect.center(), 14.0, fill);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, FontId::proportional(15.0), Color32::WHITE);
    resp
}

/// Pill-shaped text button for dark surfaces.
pub fn pill_button(ui: &mut Ui, text: &str) -> Response {
    let button = egui::Button::new(egui::RichText::new(text).color(Color32::WHITE).size(14.0))
        .fill(Color32::from_rgba_premultiplied(40, 40, 46, 235))
        .stroke(Stroke::new(1.0, Color32::from_white_alpha(40)))
        .corner_radius(16)
        .min_size(vec2(96.0, 30.0));
    ui.add(button)
}

/// Run one eframe window for this process with the shared theme, renderer and icon.
pub fn run<A: eframe::App + 'static>(name: &str, viewport: egui::ViewportBuilder, make: impl FnOnce(&egui::Context) -> A + 'static) -> anyhow::Result<()> {
    let mut viewport = viewport.with_app_id(APP_ID);
    if let Some((rgba, size)) = app_icon(64) {
        viewport = viewport.with_icon(egui::IconData { rgba, width: size, height: size });
    }
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Glow, ..Default::default() };
    eframe::run_native(
        name,
        options,
        Box::new(|cc| {
            setup(&cc.egui_ctx);
            Ok(Box::new(make(&cc.egui_ctx)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("window failed: {e}"))
}

/// Lowest free stacking slot for a kind of floating window, so several stack instead of overlapping.
/// The lock is held for the window's lifetime and released when its process exits.
pub fn claim_slot(kind: &str) -> anyhow::Result<(usize, File)> {
    let dir = run_dir();
    std::fs::create_dir_all(&dir)?;
    for n in 0..32 {
        let file = File::options().create(true).truncate(false).write(true).open(dir.join(format!("{kind}-slot-{n}.lock")))?;
        if file.try_lock().is_ok() {
            return Ok((n, file));
        }
    }
    anyhow::bail!("too many {kind} windows open")
}

/// Builder for small floating HUD windows (toasts, quick access, pins).
pub fn floating(title: &str, pos: egui::Pos2, size: Vec2) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(title)
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_taskbar(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
        .with_position(pos)
        .with_inner_size(size)
}

/// App/tray icon (assets/app-icon, regenerated by packaging/icons.sh) as RGBA at `size` px.
pub fn app_icon(size: u32) -> Option<(Vec<u8>, u32)> {
    const PNG: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/io.github.mmartinez.shotcraft.png");
    let img = image::load_from_memory_with_format(PNG, image::ImageFormat::Png).ok()?.into_rgba8();
    let img = if img.width() == size { img } else { image::imageops::resize(&img, size, size, image::imageops::FilterType::Lanczos3) };
    Some((img.into_raw(), size))
}
