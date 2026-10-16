//! Rasterises a [`Doc`] with tiny-skia. The editor preview and exports share this path,
//! so what you see is what you save.

use ecolor::Color32;
use emath::{Align, Pos2, Rect, Vec2, pos2, vec2};
use image::RgbaImage;
use tiny_skia::{
    BlendMode, FillRule, FilterQuality, GradientStop, LineCap, LineJoin, LinearGradient, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Point,
    SpreadMode, Stroke, Transform,
};

use crate::effects::{self, IRect};
use crate::model::{
    ArrowStyle, Aspect, Background, BgFill, Doc, GRADIENTS, Item, RedactMode, Shape, TextStyle, arrow_head_len, curve_control, curve_point, highlighter_width,
    text_padding,
};
use crate::{Error, Result, fonts};

/// Largest canvas we allocate: 2 GiB of RGBA, enough for a 40 000-row scrolling capture of an 8K display.
/// tiny-skia aborts the process when an allocation fails, so absurd sizes from a project file stop here.
const MAX_PIXELS: u64 = 1 << 29;

/// A blank pixmap, or [`Error::TooLarge`] when the size can't be allocated.
fn pixmap(w: u32, h: u32) -> Result<Pixmap> {
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err(Error::TooLarge(w, h));
    }
    Pixmap::new(w, h).ok_or(Error::TooLarge(w, h))
}

pub struct RenderOpts {
    /// Output pixels per base pixel. `base` passed to [`render`] must already be scaled by this.
    pub scale: f32,
    /// Item index to leave out (the one being edited live).
    pub hide: Option<usize>,
    /// Show the whole base image (crop tool).
    pub ignore_crop: bool,
    pub background: bool,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self { scale: 1.0, hide: None, ignore_crop: false, background: true }
    }
}

pub struct Rendered {
    pub pixmap: Pixmap,
    /// Base-image rectangle shown as content.
    pub region: Rect,
    /// Output-pixel position of `region.min`.
    pub offset: Vec2,
    pub scale: f32,
}

/// Final image at full resolution.
pub fn flatten(doc: &Doc) -> Result<RgbaImage> {
    Ok(pixmap_to_rgba(&render(doc, &doc.base, &RenderOpts::default())?.pixmap))
}

pub fn render(doc: &Doc, base: &RgbaImage, o: &RenderOpts) -> Result<Rendered> {
    let s = o.scale;
    let region = content_region(doc, o);
    let cw = ((region.width() * s).round() as u32).max(1);
    let ch = ((region.height() * s).round() as u32).max(1);
    let mut content = pixmap(cw, ch)?;
    blit_base(&mut content, base, (region.min.x * s).round() as i64, (region.min.y * s).round() as i64);
    let ts = Transform::from_row(s, 0.0, 0.0, s, -region.min.x * s, -region.min.y * s);
    for (i, item) in doc.items.iter().enumerate() {
        if o.hide != Some(i) {
            draw_item(&mut content, item, ts, s, region);
        }
    }
    Ok(match (&doc.background, o.background && !o.ignore_crop) {
        (Some(bg), true) => {
            let (pixmap, offset) = compose_background(&content, bg, s)?;
            Rendered { pixmap, region, offset, scale: s }
        }
        _ => Rendered { pixmap: content, region, offset: Vec2::ZERO, scale: s },
    })
}

/// Size [`render`] would produce, without rendering.
pub fn output_size(doc: &Doc, o: &RenderOpts) -> Vec2 {
    let region = content_region(doc, o);
    let content = vec2((region.width() * o.scale).round().max(1.0), (region.height() * o.scale).round().max(1.0));
    match (&doc.background, o.background && !o.ignore_crop) {
        (Some(bg), true) => background_layout(content, bg, o.scale).0,
        _ => content,
    }
}

fn content_region(doc: &Doc, o: &RenderOpts) -> Rect {
    if o.ignore_crop {
        return doc.full_rect();
    }
    let region = doc.content_rect();
    let balance = o.background && doc.background.as_ref().is_some_and(|b| b.auto_balance);
    if balance && let Some(ir) = IRect::clipped(region, doc.base.width(), doc.base.height()) {
        let b = effects::balanced_rect(&doc.base, ir, 6);
        // Balancing looks at screenshot pixels only; annotations are content too.
        let kept = doc.items.iter().fold(Rect::from_min_size(pos2(b.x as f32, b.y as f32), vec2(b.w as f32, b.h as f32)), |r, it| r.union(it.bounds()));
        return kept.intersect(region);
    }
    region
}

/// Copy `src` into `dst` with `src` pixel (ox, oy) landing at dst (0, 0), premultiplying alpha.
fn blit_base(dst: &mut Pixmap, src: &RgbaImage, ox: i64, oy: i64) {
    let (dw, dh) = (dst.width() as i64, dst.height() as i64);
    let (sw, sh) = (src.width() as i64, src.height() as i64);
    let data = dst.data_mut();
    let raw = src.as_raw();
    for y in 0..dh {
        let sy = y + oy;
        if sy < 0 || sy >= sh {
            continue;
        }
        let x0 = (-ox).max(0);
        let x1 = (sw - ox).min(dw);
        if x1 <= x0 {
            continue;
        }
        let d = ((y * dw + x0) * 4) as usize;
        let s = ((sy * sw + x0 + ox) * 4) as usize;
        let n = ((x1 - x0) * 4) as usize;
        data[d..d + n].copy_from_slice(&raw[s..s + n]);
        premultiply(&mut data[d..d + n]);
    }
}

fn premultiply(px: &mut [u8]) {
    for p in px.as_chunks_mut::<4>().0 {
        let a = p[3] as u32;
        if a != 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
}

pub fn rgba_to_pixmap(img: &RgbaImage) -> Result<Pixmap> {
    let mut p = pixmap(img.width().max(1), img.height().max(1))?;
    blit_base(&mut p, img, 0, 0);
    Ok(p)
}

pub fn pixmap_to_rgba(p: &Pixmap) -> RgbaImage {
    let mut out = RgbaImage::new(p.width(), p.height());
    for (dst, px) in out.as_chunks_mut::<4>().0.iter_mut().zip(p.pixels()) {
        let c = px.demultiply();
        *dst = [c.red(), c.green(), c.blue(), c.alpha()];
    }
    out
}

fn paint(color: Color32) -> Paint<'static> {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    let mut p = Paint::default();
    p.set_color_rgba8(r, g, b, a);
    p.anti_alias = true;
    p
}

fn stroke(width: f32) -> Stroke {
    Stroke { width: width.max(0.5), line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() }
}

fn sk_rect(r: Rect) -> Option<tiny_skia::Rect> {
    tiny_skia::Rect::from_ltrb(r.min.x, r.min.y, r.max.x, r.max.y)
}

pub fn rounded_rect_path(r: Rect, radius: f32) -> Option<Path> {
    let rad = radius.min(r.width() / 2.0).min(r.height() / 2.0).max(0.0);
    if rad < 0.5 {
        return Some(PathBuilder::from_rect(sk_rect(r)?));
    }
    let k = rad * 0.447_715; // 1 - 0.5523 (cubic circle constant)
    let (l, t, rr, b) = (r.min.x, r.min.y, r.max.x, r.max.y);
    let mut pb = PathBuilder::new();
    pb.move_to(l + rad, t);
    pb.line_to(rr - rad, t);
    pb.cubic_to(rr - k, t, rr, t + k, rr, t + rad);
    pb.line_to(rr, b - rad);
    pb.cubic_to(rr, b - k, rr - k, b, rr - rad, b);
    pb.line_to(l + rad, b);
    pb.cubic_to(l + k, b, l, b - k, l, b - rad);
    pb.line_to(l, t + rad);
    pb.cubic_to(l, t + k, l + k, t, l + rad, t);
    pb.close();
    pb.finish()
}

fn polyline(pts: &[Pos2]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(pts.first()?.x, pts[0].y);
    if pts.len() == 1 {
        pb.line_to(pts[0].x + 0.01, pts[0].y);
    }
    for p in &pts[1..] {
        pb.line_to(p.x, p.y);
    }
    pb.finish()
}

fn polygon(pts: &[Pos2]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(pts.first()?.x, pts[0].y);
    for p in &pts[1..] {
        pb.line_to(p.x, p.y);
    }
    pb.close();
    pb.finish()
}

fn is_light(c: Color32) -> bool {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32 > 170.0
}

fn contrast_on(c: Color32) -> Color32 {
    if is_light(c) { Color32::from_rgb(28, 28, 30) } else { Color32::WHITE }
}

/// Head triangle for an arrow ending at `tip` travelling along unit `dir`.
fn head(tip: Pos2, dir: Vec2, len: f32) -> (Pos2, [Pos2; 3]) {
    let n = dir.rot90();
    let half = len * 0.55;
    let base = tip - dir * len;
    (base, [tip, base + n * half, base - n * half])
}

fn draw_arrow(pm: &mut Pixmap, item: &Item, ts: Transform) {
    let Shape::Arrow { a, b, style, bend } = item.shape else { return };
    let (w, color) = (item.width, item.color);
    let d = b - a;
    let len = d.length();
    if len < 1.0 {
        return;
    }
    let dir = d / len;
    let hl = arrow_head_len(w).min(len * 0.6);
    let p = paint(color);
    match style {
        ArrowStyle::Tapered => {
            let n = dir.rot90();
            let (base, tri) = head(b, dir, hl);
            let poly = [a + n * w * 0.15, base + n * w * 0.55, tri[1], tri[0], tri[2], base - n * w * 0.55, a - n * w * 0.15];
            if let Some(path) = polygon(&poly) {
                pm.fill_path(&path, &p, FillRule::Winding, ts, None);
            }
        }
        ArrowStyle::Classic | ArrowStyle::Double => {
            let (end, tri) = head(b, dir, hl);
            let (start, tri2) = head(a, -dir, hl);
            let from = if style == ArrowStyle::Double { start + dir * 1.0 } else { a };
            if let Some(path) = polyline(&[from, end + dir * 1.0]) {
                let mut s = stroke(w);
                s.line_cap = LineCap::Butt;
                pm.stroke_path(&path, &p, &s, ts, None);
            }
            for t in std::iter::once(tri).chain((style == ArrowStyle::Double).then_some(tri2)) {
                if let Some(path) = polygon(&t) {
                    pm.fill_path(&path, &p, FillRule::Winding, ts, None);
                }
            }
        }
        ArrowStyle::Curved => {
            let c = curve_control(a, b, bend);
            // Stop the stroke where the head starts so the round cap can't poke through the tip.
            let steps = 48;
            let mut t_end = 1.0;
            for i in (0..=steps).rev() {
                let t = i as f32 / steps as f32;
                if curve_point(a, c, b, t).distance(b) >= hl * 0.8 {
                    t_end = t;
                    break;
                }
            }
            let pts: Vec<Pos2> = (0..=steps).map(|i| curve_point(a, c, b, t_end * i as f32 / steps as f32)).collect();
            let tail = pts.last().copied().unwrap_or(a);
            if let Some(path) = polyline(&pts) {
                pm.stroke_path(&path, &p, &stroke(w), ts, None);
            }
            let hd = (b - tail).normalized();
            let (_, tri) = head(b, hd, hl);
            if let Some(path) = polygon(&tri) {
                pm.fill_path(&path, &p, FillRule::Winding, ts, None);
            }
        }
    }
}

fn draw_text(pm: &mut Pixmap, pos: Pos2, text: &str, size: f32, style: TextStyle, color: Color32, ts: Transform) {
    let pad = text_padding(size, style);
    if style.has_box() {
        let bg = match style {
            TextStyle::Filled => color,
            TextStyle::Light => Color32::WHITE,
            _ => Color32::from_rgb(28, 28, 30),
        };
        let r = Rect::from_min_size(pos, fonts::measure(text, size)).expand(pad);
        if let Some(path) = rounded_rect_path(r, size * 0.3) {
            pm.fill_path(&path, &paint(bg), FillRule::Winding, ts, None);
        }
    }
    let Some(path) = fonts::text_path(text, size, pos) else { return };
    let fill = match style {
        TextStyle::Filled => contrast_on(color),
        TextStyle::Dark if !is_light(color) => Color32::WHITE,
        _ => color,
    };
    match style {
        TextStyle::Outlined => {
            pm.stroke_path(&path, &paint(contrast_on(color)), &stroke(size * 0.16), ts, None);
        }
        TextStyle::Shadow => {
            let off = ts.pre_translate(size * 0.04, size * 0.07);
            pm.fill_path(&path, &paint(Color32::from_black_alpha(120)), FillRule::Winding, off, None);
        }
        TextStyle::Stroke => {
            pm.stroke_path(&path, &paint(color), &stroke(size * 0.06), ts, None);
            return;
        }
        _ => {}
    }
    pm.fill_path(&path, &paint(fill), FillRule::Winding, ts, None);
}

fn draw_item(pm: &mut Pixmap, item: &Item, ts: Transform, s: f32, region: Rect) {
    let w = item.width;
    let p = paint(item.color);
    match &item.shape {
        Shape::Rect { rect, filled } => {
            if let Some(path) = rounded_rect_path(*rect, w * 0.8) {
                if *filled {
                    pm.fill_path(&path, &p, FillRule::Winding, ts, None);
                } else {
                    pm.stroke_path(&path, &p, &stroke(w), ts, None);
                }
            }
        }
        Shape::Ellipse { rect, filled } => {
            if let Some(path) = sk_rect(*rect).and_then(PathBuilder::from_oval) {
                if *filled {
                    pm.fill_path(&path, &p, FillRule::Winding, ts, None);
                } else {
                    pm.stroke_path(&path, &p, &stroke(w), ts, None);
                }
            }
        }
        Shape::Line { a, b } => {
            if let Some(path) = polyline(&[*a, *b]) {
                pm.stroke_path(&path, &p, &stroke(w), ts, None);
            }
        }
        Shape::Arrow { .. } => draw_arrow(pm, item, ts),
        Shape::Pen { pts } => {
            if let Some(path) = polyline(pts) {
                pm.stroke_path(&path, &p, &stroke(w), ts, None);
            }
        }
        Shape::Highlighter { pts } => {
            if let Some(path) = polyline(pts) {
                let mut hp = p.clone();
                hp.blend_mode = BlendMode::Multiply;
                let mut st = stroke(highlighter_width(w));
                st.line_cap = LineCap::Square;
                pm.stroke_path(&path, &hp, &st, ts, None);
            }
        }
        Shape::Text { pos, text, size, style } => draw_text(pm, *pos, text, *size, *style, item.color, ts),
        Shape::Counter { center, n } => {
            let r = item.counter_radius();
            if let Some(path) = PathBuilder::from_circle(center.x, center.y, r) {
                pm.fill_path(&path, &p, FillRule::Winding, ts, None);
            }
            let label = n.to_string();
            let size = r * 1.15;
            let m = fonts::measure(&label, size);
            draw_text(pm, *center - m * 0.5, &label, size, TextStyle::Plain, contrast_on(item.color), ts);
        }
        Shape::Redact { rect, mode, seed } => {
            let out = Rect::from_min_max(pos2(0.0, 0.0) + (rect.min - region.min) * s, pos2(0.0, 0.0) + (rect.max - region.min) * s);
            let side = rect.width().min(rect.height());
            match (mode, IRect::clipped(out, pm.width(), pm.height())) {
                (RedactMode::Solid, Some(ir)) => {
                    // Every touched pixel, fully opaque and without antialiasing: nothing of the original may show through.
                    let [r, g, b, _] = item.color.to_srgba_unmultiplied();
                    let mut opaque = Paint::default();
                    opaque.set_color_rgba8(r, g, b, 255);
                    if let Some(rect) = tiny_skia::Rect::from_xywh(ir.x as f32, ir.y as f32, ir.w as f32, ir.h as f32) {
                        pm.fill_rect(rect, &opaque, Transform::identity(), None);
                    }
                }
                // Sizes are in base pixels times `s`, unrounded, so a scaled-down preview shows the export's effect, not a stronger one.
                (RedactMode::Pixelate, Some(ir)) => {
                    let block = (side / 8.0).clamp(6.0, 32.0) * s;
                    let width = pm.width();
                    effects::pixelate_region(pm.data_mut(), width, ir, block, *seed);
                }
                (RedactMode::SecureBlur, Some(ir)) => {
                    let block = (side / 14.0).clamp(4.0, 16.0) * s;
                    let width = pm.width();
                    effects::pixelate_region(pm.data_mut(), width, ir, block, *seed);
                    effects::blur_region(pm.data_mut(), width, ir, (block * 1.2) as u32);
                }
                (RedactMode::Blur, Some(ir)) => {
                    let radius = ((side / 6.0).clamp(4.0, 24.0) * s) as u32;
                    let width = pm.width();
                    effects::blur_region(pm.data_mut(), width, ir, radius);
                }
                _ => {}
            }
        }
        Shape::Spotlight { rect, ellipse } => {
            let mut pb = PathBuilder::new();
            if let Some(full) = sk_rect(region.expand(1.0)) {
                pb.push_rect(full);
            }
            if let Some(r) = sk_rect(*rect) {
                if *ellipse { pb.push_oval(r) } else { pb.push_rect(r) }
            }
            if let Some(path) = pb.finish() {
                pm.fill_path(&path, &paint(Color32::from_black_alpha(140)), FillRule::EvenOdd, ts, None);
            }
        }
        Shape::Image { rect, image } => {
            // The item's own buffer already holds this many pixels, so this only fails if memory just ran out.
            if let Ok(src) = rgba_to_pixmap(image) {
                let t = ts.pre_translate(rect.min.x, rect.min.y).pre_scale(rect.width() / src.width() as f32, rect.height() / src.height() as f32);
                pm.draw_pixmap(0, 0, src.as_ref(), &PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() }, t, None);
            }
        }
    }
}

fn gradient_paint(index: usize, w: f32, h: f32) -> Option<Paint<'static>> {
    let g = GRADIENTS.get(index)?;
    let (sin, cos) = g.angle.to_radians().sin_cos();
    let half = (w * cos.abs() + h * sin.abs()) / 2.0;
    let c = Point::from_xy(w / 2.0, h / 2.0);
    let start = Point::from_xy(c.x - cos * half, c.y - sin * half);
    let end = Point::from_xy(c.x + cos * half, c.y + sin * half);
    let n = g.stops.len().max(2) - 1;
    let stops =
        g.stops.iter().enumerate().map(|(i, [r, gg, b])| GradientStop::new(i as f32 / n as f32, tiny_skia::Color::from_rgba8(*r, *gg, *b, 255))).collect();
    let shader = LinearGradient::new(start, end, stops, SpreadMode::Pad, Transform::identity())?;
    Some(Paint { shader, anti_alias: true, ..Default::default() })
}

/// Canvas size and content offset for a background, in output pixels.
pub fn background_layout(content: Vec2, bg: &Background, s: f32) -> (Vec2, Vec2) {
    let pad = bg.padding * s;
    let mut size = content + Vec2::splat(2.0 * pad);
    if let Aspect::Ratio(rw, rh) = bg.aspect {
        let target = rw as f32 / rh.max(1) as f32;
        if size.x / size.y < target {
            size.x = size.y * target;
        } else {
            size.y = size.x / target;
        }
    }
    let place = |avail: f32, len: f32, align: Align| match align {
        Align::Min => pad,
        Align::Center => (avail - len) / 2.0,
        Align::Max => avail - len - pad,
    };
    let offset = vec2(place(size.x, content.x, bg.align.x()), place(size.y, content.y, bg.align.y()));
    (size.round(), offset.round())
}

fn compose_background(content: &Pixmap, bg: &Background, s: f32) -> Result<(Pixmap, Vec2)> {
    let csize = vec2(content.width() as f32, content.height() as f32);
    let (size, offset) = background_layout(csize, bg, s);
    let (w, h) = (size.x.max(1.0) as u32, size.y.max(1.0) as u32);
    let mut canvas = pixmap(w, h)?;
    let full = tiny_skia::Rect::from_xywh(0.0, 0.0, w as f32, h as f32);
    let cover = |pm: &mut Pixmap, src: &Pixmap| {
        let k = (w as f32 / src.width() as f32).max(h as f32 / src.height() as f32);
        let t = Transform::from_translate((w as f32 - src.width() as f32 * k) / 2.0, (h as f32 - src.height() as f32 * k) / 2.0).pre_scale(k, k);
        pm.draw_pixmap(0, 0, src.as_ref(), &PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() }, t, None);
    };
    match &bg.fill {
        BgFill::Transparent => {}
        BgFill::Gradient(i) => {
            if let (Some(p), Some(full)) = (gradient_paint(*i, w as f32, h as f32), full) {
                canvas.fill_rect(full, &p, Transform::identity(), None);
            }
        }
        BgFill::Solid(c) => {
            let [r, g, b, a] = c.to_srgba_unmultiplied();
            canvas.fill(tiny_skia::Color::from_rgba8(r, g, b, a));
        }
        BgFill::Image(img) => cover(&mut canvas, &rgba_to_pixmap(img)?),
        BgFill::Blurred => {
            cover(&mut canvas, content);
            let radius = (w.max(h) / 25).max(4);
            effects::blur_buffer(canvas.data_mut(), w, h, radius);
        }
    }
    let content_rect = Rect::from_min_size(pos2(offset.x, offset.y), csize);
    let radius = bg.radius * s;
    if bg.shadow > 0.0 {
        draw_shadow(&mut canvas, content_rect, radius, bg.shadow, s);
    }
    let mut mask = Mask::new(w, h).ok_or(Error::TooLarge(w, h))?;
    if let Some(path) = rounded_rect_path(content_rect, radius) {
        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
    }
    canvas.draw_pixmap(offset.x as i32, offset.y as i32, content.as_ref(), &PixmapPaint::default(), Transform::identity(), Some(&mask));
    Ok((canvas, offset))
}

/// Soft drop shadow, blurred at quarter resolution and scaled up.
fn draw_shadow(canvas: &mut Pixmap, rect: Rect, radius: f32, strength: f32, s: f32) {
    const DOWN: f32 = 4.0;
    let (w, h) = (canvas.width(), canvas.height());
    let (sw, sh) = (((w as f32) / DOWN).ceil() as u32, ((h as f32) / DOWN).ceil() as u32);
    let Some(mut small) = Pixmap::new(sw.max(1), sh.max(1)) else { return };
    let shifted = rect.translate(vec2(0.0, 10.0 * s * strength));
    let r = Rect::from_min_max(shifted.min / DOWN, shifted.max / DOWN);
    if let Some(path) = rounded_rect_path(r, radius / DOWN) {
        let alpha = (90.0 + 110.0 * strength).min(255.0) as u8;
        small.fill_path(&path, &paint(Color32::from_black_alpha(alpha)), FillRule::Winding, Transform::identity(), None);
    }
    let blur = ((6.0 + 30.0 * strength) * s / DOWN).max(1.0) as u32;
    effects::blur_buffer(small.data_mut(), sw, sh, blur);
    canvas.draw_pixmap(0, 0, small.as_ref(), &PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() }, Transform::from_scale(DOWN, DOWN), None);
}

/// Resize to `percent` of the original with a high-quality filter.
pub fn resize_percent(img: &RgbaImage, percent: u32) -> RgbaImage {
    if percent == 100 {
        return img.clone();
    }
    let w = (img.width() * percent / 100).max(1);
    let h = (img.height() * percent / 100).max(1);
    image::imageops::resize(img, w, h, image::imageops::FilterType::Lanczos3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Doc;

    fn white(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]))
    }

    #[test]
    fn plain_doc_renders_identical_pixels() {
        let mut img = white(30, 20);
        img.put_pixel(3, 4, image::Rgba([1, 2, 3, 255]));
        let doc = Doc::new(img.clone());
        assert_eq!(flatten(&doc).unwrap(), img);
    }

    #[test]
    fn crop_selects_region() {
        let mut img = white(30, 20);
        img.put_pixel(10, 5, image::Rgba([9, 9, 9, 255]));
        let mut doc = Doc::new(img);
        doc.crop = Some(Rect::from_min_size(pos2(10.0, 5.0), vec2(8.0, 6.0)));
        let out = flatten(&doc).unwrap();
        assert_eq!(out.dimensions(), (8, 6));
        assert_eq!(out.get_pixel(0, 0).0, [9, 9, 9, 255]);
    }

    #[test]
    fn solid_redaction_is_opaque_on_every_touched_pixel() {
        let mut doc = Doc::new(white(40, 40));
        doc.items.push(Item {
            shape: Shape::Redact { rect: Rect::from_min_max(pos2(10.3, 10.6), pos2(19.2, 19.5)), mode: RedactMode::Solid, seed: 1 },
            color: Color32::from_rgba_unmultiplied(0, 0, 0, 100),
            width: 4.0,
        });
        let out = flatten(&doc).unwrap();
        for y in 0..40 {
            for x in 0..40 {
                let inside = (10..20).contains(&x) && (10..20).contains(&y);
                let want = if inside { [0, 0, 0, 255] } else { [255, 255, 255, 255] };
                assert_eq!(out.get_pixel(x, y).0, want, "pixel ({x},{y})");
            }
        }
    }

    #[test]
    fn preview_pixelation_keeps_the_export_cell_size() {
        // A horizontal gradient, pixelated with 6 px cells, previewed at a quarter size: cells are 1.5 preview px.
        let base = RgbaImage::from_fn(400, 24, |x, _| image::Rgba([(x * 255 / 399) as u8, 0, 0, 255]));
        let mut doc = Doc::new(base.clone());
        doc.items.push(Item {
            shape: Shape::Redact { rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 24.0)), mode: RedactMode::Pixelate, seed: 3 },
            color: Color32::BLACK,
            width: 4.0,
        });
        let small = image::imageops::resize(&base, 100, 6, image::imageops::FilterType::Triangle);
        let preview = pixmap_to_rgba(&render(&doc, &small, &RenderOpts { scale: 0.25, ..Default::default() }).unwrap().pixmap);
        let row: Vec<u8> = (0..100).map(|x| preview.get_pixel(x, 2).0[0]).collect();
        let cells = 1 + row.windows(2).filter(|w| w[0] != w[1]).count();
        // 100 / 1.5 ≈ 67 cells; rounding the cell up to 2 preview px (8 base px) would give at most 50.
        assert!(cells > 55, "{cells} cells");
    }

    #[test]
    fn oversized_canvas_is_an_error_not_an_abort() {
        let mut doc = Doc::new(white(10, 10));
        doc.background = Some(Background { padding: 1.0e6, ..Default::default() });
        assert!(matches!(flatten(&doc), Err(Error::TooLarge(..))));
    }

    #[test]
    fn pixelate_destroys_detail_inside_rect() {
        let mut img = white(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                if (x + y) % 2 == 0 {
                    img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                }
            }
        }
        let mut doc = Doc::new(img);
        doc.items.push(Item {
            shape: Shape::Redact { rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(64.0, 64.0)), mode: RedactMode::Pixelate, seed: 7 },
            color: Color32::BLACK,
            width: 4.0,
        });
        let out = flatten(&doc).unwrap();
        // A checkerboard averages to mid-grey (±noise) everywhere.
        for (x, y) in [(0, 0), (1, 0), (33, 17)] {
            let v = out.get_pixel(x, y).0[0];
            assert!((100..=155).contains(&v), "pixel ({x},{y}) = {v}");
        }
    }

    #[test]
    fn background_layout_respects_aspect_and_alignment() {
        let bg = Background { padding: 10.0, aspect: Aspect::Ratio(2, 1), align: emath::Align2::LEFT_TOP, ..Default::default() };
        let (size, off) = background_layout(vec2(100.0, 100.0), &bg, 1.0);
        assert_eq!(size, vec2(240.0, 120.0));
        assert_eq!(off, vec2(10.0, 10.0));
        let centered = Background { align: emath::Align2::CENTER_CENTER, ..bg };
        assert_eq!(background_layout(vec2(100.0, 100.0), &centered, 1.0).1, vec2(70.0, 10.0));
    }

    #[test]
    fn auto_balance_keeps_annotations_in_the_margin() {
        // Dark content in the middle of a white page, plus a counter far out in the margin.
        let mut img = white(200, 100);
        for y in 40..60 {
            for x in 90..110 {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let mut doc = Doc::new(img);
        doc.background = Some(Background { auto_balance: true, ..Default::default() });
        let alone = render(&doc, &doc.base, &RenderOpts::default()).unwrap().region;
        assert_eq!(alone, Rect::from_min_max(pos2(50.0, 0.0), pos2(150.0, 100.0)));
        doc.items.push(Item { shape: Shape::Counter { center: pos2(15.0, 50.0), n: 1 }, color: Color32::RED, width: 4.0 });
        let with_item = render(&doc, &doc.base, &RenderOpts::default()).unwrap().region;
        assert!(with_item.contains(pos2(3.0, 50.0)), "counter stays visible: {with_item:?}");
    }

    #[test]
    fn background_wraps_content_and_keeps_its_pixels() {
        let mut doc = Doc::new(RgbaImage::from_pixel(50, 40, image::Rgba([10, 200, 30, 255])));
        doc.background = Some(Background { padding: 20.0, radius: 0.0, shadow: 0.0, auto_balance: false, ..Default::default() });
        let out = flatten(&doc).unwrap();
        assert_eq!(out.dimensions(), (90, 80));
        assert_eq!(out.get_pixel(45, 40).0, [10, 200, 30, 255]);
        assert_ne!(out.get_pixel(2, 2).0, [10, 200, 30, 255]);
    }

    #[test]
    fn transparent_background_with_shadow_has_soft_alpha() {
        let mut doc = Doc::new(RgbaImage::from_pixel(60, 60, image::Rgba([255, 255, 255, 255])));
        // Padding wider than the shadow's spread, so the corners must stay clear.
        doc.background = Some(Background { fill: BgFill::Transparent, padding: 120.0, shadow: 0.6, auto_balance: false, ..Default::default() });
        let out = flatten(&doc).unwrap();
        assert_eq!(out.dimensions(), (300, 300));
        assert_eq!(out.get_pixel(0, 0).0[3], 0, "far corner stays transparent");
        let below = out.get_pixel(150, 120 + 60 + 6).0[3];
        assert!(below > 0 && below < 255, "shadow below content is translucent, got {below}");
    }

    #[test]
    fn scaled_render_reports_its_layout() {
        let mut doc = Doc::new(white(100, 80));
        doc.crop = Some(Rect::from_min_size(pos2(20.0, 10.0), vec2(50.0, 40.0)));
        doc.background = Some(Background { padding: 16.0, auto_balance: false, ..Default::default() });
        let small = image::imageops::resize(&*doc.base, 50, 40, image::imageops::FilterType::Triangle);
        let opts = RenderOpts { scale: 0.5, ..Default::default() };
        let r = render(&doc, &small, &opts).unwrap();
        assert_eq!(r.region, doc.crop.unwrap());
        assert_eq!(r.offset, vec2(8.0, 8.0));
        assert_eq!(vec2(r.pixmap.width() as f32, r.pixmap.height() as f32), output_size(&doc, &opts));
    }
}
