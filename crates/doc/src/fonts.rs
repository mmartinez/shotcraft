//! Bundled fonts and vector text layout for rendering annotations.
//! Sizes are pixels per em, the same unit egui uses, so editor previews match exports.

use std::sync::LazyLock;

use emath::{Pos2, Vec2, vec2};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

pub const INTER_400: &[u8] = include_bytes!("../assets/fonts/Inter-400.ttf");
pub const INTER_500: &[u8] = include_bytes!("../assets/fonts/Inter-500.ttf");
pub const INTER_700: &[u8] = include_bytes!("../assets/fonts/Inter-700.ttf");

/// Annotation text font followed by fallbacks for glyphs Inter's Latin subset lacks.
const ANNOTATION_CHAIN: [&[u8]; 3] = [INTER_700, epaint_default_fonts::UBUNTU_LIGHT, epaint_default_fonts::NOTO_EMOJI_REGULAR];

/// egui font family names the UI registers for these fonts.
pub const ANNOTATION_FAMILY: &str = "annotation";
pub const MEDIUM_FAMILY: &str = "medium";

#[allow(clippy::expect_used)] // the bundled fonts are fixed bytes known to parse (see the tests)
static CHAIN: LazyLock<Vec<FontRef<'static>>> = LazyLock::new(|| ANNOTATION_CHAIN.iter().map(|b| FontRef::new(b).expect("bundled font parses")).collect());

fn chain() -> &'static [FontRef<'static>] {
    &CHAIN
}

struct Glyph {
    font: usize,
    id: GlyphId,
    x: f32,
    baseline: f32,
}

struct Layout {
    glyphs: Vec<Glyph>,
    size: Vec2,
}

fn line_metrics(size: f32) -> (f32, f32) {
    let m = chain()[0].metrics(Size::new(size), LocationRef::default());
    let ascent = m.ascent;
    let line_height = m.ascent - m.descent + m.leading;
    (ascent, line_height.max(size))
}

fn layout(text: &str, size: f32) -> Layout {
    let fonts = chain();
    let (ascent, line_height) = line_metrics(size);
    let metrics: Vec<_> = fonts.iter().map(|f| f.glyph_metrics(Size::new(size), LocationRef::default())).collect();
    let charmaps: Vec<_> = fonts.iter().map(|f| f.charmap()).collect();
    let mut glyphs = Vec::new();
    let mut width = 0.0f32;
    let mut lines = 0;
    for (li, line) in text.split('\n').enumerate() {
        lines += 1;
        let baseline = li as f32 * line_height + ascent;
        let mut x = 0.0;
        for c in line.chars() {
            let found = charmaps.iter().enumerate().find_map(|(i, cm)| cm.map(c).filter(|g| *g != GlyphId::NOTDEF).map(|g| (i, g)));
            let Some((font, id)) = found else { continue };
            glyphs.push(Glyph { font, id, x, baseline });
            x += metrics[font].advance_width(id).unwrap_or(0.0);
        }
        width = width.max(x);
    }
    Layout { glyphs, size: vec2(width.max(size * 0.3), lines as f32 * line_height) }
}

/// Size of the text block; empty text still has a caret-sized box.
pub fn measure(text: &str, size: f32) -> Vec2 {
    layout(text, size).size
}

struct PathPen<'a> {
    pb: &'a mut tiny_skia::PathBuilder,
    dx: f32,
    dy: f32,
}

impl OutlinePen for PathPen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.pb.move_to(self.dx + x, self.dy - y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.pb.line_to(self.dx + x, self.dy - y);
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.pb.quad_to(self.dx + cx0, self.dy - cy0, self.dx + x, self.dy - y);
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.pb.cubic_to(self.dx + cx0, self.dy - cy0, self.dx + cx1, self.dy - cy1, self.dx + x, self.dy - y);
    }
    fn close(&mut self) {
        self.pb.close();
    }
}

/// Glyph outlines of `text` with its top-left corner at `origin`.
pub fn text_path(text: &str, size: f32, origin: Pos2) -> Option<tiny_skia::Path> {
    let fonts = chain();
    let lay = layout(text, size);
    let mut pb = tiny_skia::PathBuilder::new();
    for g in &lay.glyphs {
        let Some(outline) = fonts[g.font].outline_glyphs().get(g.id) else { continue };
        let mut pen = PathPen { pb: &mut pb, dx: origin.x + g.x, dy: origin.y + g.baseline };
        let _ = outline.draw(DrawSettings::unhinted(Size::new(size), LocationRef::default()), &mut pen);
    }
    pb.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_text_is_taller_and_wider_text_wider() {
        let one = measure("Hello", 20.0);
        let two = measure("Hello\nHello", 20.0);
        assert!((two.y - 2.0 * one.y).abs() < 0.01);
        assert!(measure("Hello world", 20.0).x > one.x);
        assert!(measure("Hello", 40.0).x > one.x * 1.9);
    }

    #[test]
    fn glyph_path_fits_measured_box() {
        let origin = Pos2::new(10.0, 20.0);
        let size = measure("Hg", 32.0);
        let bounds = text_path("Hg", 32.0, origin).unwrap().bounds();
        assert!(bounds.left() >= origin.x - 1.0 && bounds.right() <= origin.x + size.x + 1.0);
        assert!(bounds.top() >= origin.y - 1.0 && bounds.bottom() <= origin.y + size.y + 1.0);
    }

    #[test]
    fn non_latin_falls_back() {
        // Inter's Latin subset lacks Cyrillic; the fallback chain must still produce outlines.
        assert!(text_path("Привет", 24.0, Pos2::ZERO).is_some());
    }
}
