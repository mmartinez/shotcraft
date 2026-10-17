//! Headless operations for agents and, later, the command registry: a typed request in, a typed
//! result out. Nothing here toasts, opens windows or touches the clipboard unless that is the
//! operation's whole point (`copy_capture`). The function name is the operation's stable id.
//! Request field docs become the JSON Schema descriptions a model reads.

use std::path::PathBuf;

use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::{ExportFormat, Settings};
use crate::doc::effects::IRect;
use crate::doc::model::Doc;
use crate::history::{self, Entry, HistoryId};
use crate::{Result, capture, ocr, share};

/// Region in full-resolution pixels of the captured image, origin at its top-left corner.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Area {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl From<Area> for IRect {
    fn from(a: Area) -> Self {
        IRect { x: a.x, y: a.y, w: a.width, h: a.height }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureScreen {
    /// Display number: 1 is the primary display, 2 the next one, and so on. Default 1.
    pub display: Option<usize>,
    /// Crop to this region of the display, in full-resolution pixels. Default: the whole display.
    pub area: Option<Area>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureWindow {
    /// A window id from `list_windows`.
    pub window_id: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureRef {
    pub id: HistoryId,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetCapture {
    pub id: HistoryId,
    /// Zoom into this region of the stored capture, in its full-resolution pixels. Default: the whole capture.
    pub area: Option<Area>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadText {
    pub id: HistoryId,
    /// Keep the line breaks of the recognized text (else lines are joined with spaces). Default: the user's OCR setting.
    pub linebreaks: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListCaptures {
    /// How many captures to return, newest first. Default 20, at most 200.
    pub limit: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(inline)]
pub enum View {
    Editor,
    Pin,
    QuickAccess,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowCapture {
    pub id: HistoryId,
    /// `editor`: the annotation editor, which saves the user's edits back into this history entry.
    /// `pin`: a window floating above all others. `quick_access`: the overlay with copy, save, upload and drag.
    pub view: View,
}

/// What every image-returning op reports.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Captured {
    /// The history entry. `width`/`height` are the full-resolution size of the image the preview shows:
    /// after `get_capture` with `area`, the size of that crop, while `path` still names the whole capture.
    #[serde(flatten)]
    pub entry: Entry,
    /// Size of the inline preview image in pixels.
    pub shown_width: u32,
    pub shown_height: u32,
    /// The full-resolution PNG of the whole capture in history.
    pub path: PathBuf,
    /// The display captured (`capture_screen` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<usize>,
    /// How many displays are connected (`capture_screen` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub displays: Option<usize>,
}

/// Inline image for the agent: never larger than `PREVIEW_MAX_EDGE` on its long edge.
pub struct Preview {
    pub mime: &'static str,
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub struct Shot {
    pub captured: Captured,
    pub preview: Preview,
}

/// Position and size are in screen units: logical points on macOS, pixels elsewhere.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Window {
    pub id: u32,
    pub title: String,
    pub app: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Windows {
    pub windows: Vec<Window>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Captures {
    pub captures: Vec<Entry>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Text {
    pub text: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Saved {
    pub path: PathBuf,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Copied {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Link {
    pub link: String,
}

pub const PREVIEW_MAX_EDGE: u32 = 1568;
pub const PREVIEW_PNG_MAX_BYTES: usize = 1 << 20;

/// Visible windows of other apps, topmost first.
pub fn list_windows() -> Windows {
    let windows = capture::list_windows()
        .into_iter()
        .map(|w| Window {
            id: w.id,
            title: w.title,
            app: w.app,
            x: w.rect.min.x as i32,
            y: w.rect.min.y as i32,
            width: w.rect.width() as u32,
            height: w.rect.height() as u32,
        })
        .collect();
    Windows { windows }
}

/// Store in history (kind "fullscreen" or "area") and prune.
pub fn capture_screen(req: CaptureScreen) -> Result<Shot> {
    let display = req.display.unwrap_or(1);
    let shot = capture::capture_monitor(None, Some(display))?;
    let displays = capture::display_count()?;
    let (image, kind) = match req.area {
        Some(area) => (capture::crop(&shot.image, capture::clip(area.into(), &shot.image)?), "area"),
        None => (shot.image, "fullscreen"),
    };
    let mut shot = store(image, kind)?;
    (shot.captured.display, shot.captured.displays) = (Some(display), Some(displays));
    Ok(shot)
}

/// Plain window pixels (kind "window"); the user's window style is not applied.
pub fn capture_window(req: CaptureWindow) -> Result<Shot> {
    store(capture::capture_window(req.window_id)?, "window")
}

pub fn list_captures(req: ListCaptures) -> Captures {
    let mut captures = history::list();
    captures.truncate(req.limit.unwrap_or(20).clamp(1, 200));
    Captures { captures }
}

pub fn get_capture(req: GetCapture) -> Result<Shot> {
    let mut entry = history::find(&req.id)?;
    let full = history::load_image(req.id.as_str())?;
    let image = match req.area {
        Some(area) => capture::crop(&full, capture::clip(area.into(), &full)?),
        None => full,
    };
    (entry.width, entry.height) = image.dimensions();
    shot(entry, image)
}

/// QR payloads first, else OCR; an empty string when there is no text. The first call downloads the pinned models (~12 MB).
pub fn read_text(req: ReadText) -> Result<Text> {
    let image = load(&req.id)?;
    let keep_linebreaks = req.linebreaks.unwrap_or_else(|| Settings::load().ocr_keep_linebreaks);
    Ok(Text { text: ocr::extract(&image, keep_linebreaks)? })
}

/// Into the configured folder under a new unique name; never overwrites.
pub fn save_capture(req: CaptureRef) -> Result<Saved> {
    Ok(Saved { path: share::save_default(&load(&req.id)?, &Settings::load())? })
}

pub fn copy_capture(req: CaptureRef) -> Result<Copied> {
    let image = load(&req.id)?;
    share::copy_image(&image)?;
    Ok(Copied { width: image.width(), height: image.height() })
}

/// The link is returned, not copied.
pub fn upload_capture(req: CaptureRef) -> Result<Link> {
    Ok(Link { link: share::upload(&load(&req.id)?, &Settings::load())? })
}

/// Agent captures skip the human's after-capture flow: no copy, save, toast, Quick Access, window style or `last_area`.
fn store(image: RgbaImage, kind: &str) -> Result<Shot> {
    let doc = Doc::new(image);
    let entry = history::add(&doc, kind)?;
    history::prune(Settings::load().history_days);
    shot(entry, std::sync::Arc::unwrap_or_clone(doc.base))
}

fn shot(entry: Entry, image: RgbaImage) -> Result<Shot> {
    let preview = preview(image)?;
    let path = history::image_path(&entry.id);
    Ok(Shot { captured: Captured { entry, shown_width: preview.width, shown_height: preview.height, path, display: None, displays: None }, preview })
}

fn load(id: &HistoryId) -> Result<RgbaImage> {
    history::find(id)?;
    history::load_image(id.as_str())
}

/// Downscale to fit `PREVIEW_MAX_EDGE` (Triangle filter, never upscale); PNG when it encodes within
/// `PREVIEW_PNG_MAX_BYTES`, else JPEG q85 (alpha flattened on white).
pub fn preview(img: RgbaImage) -> Result<Preview> {
    let img = if img.width().max(img.height()) > PREVIEW_MAX_EDGE {
        DynamicImage::ImageRgba8(img).resize(PREVIEW_MAX_EDGE, PREVIEW_MAX_EDGE, FilterType::Triangle).into_rgba8()
    } else {
        img
    };
    let png = share::encode(&img, ExportFormat::Png, 100)?;
    let (mime, bytes) = if png.len() <= PREVIEW_PNG_MAX_BYTES { ("image/png", png) } else { ("image/jpeg", share::encode(&img, ExportFormat::Jpeg, 85)?) };
    Ok(Preview { mime, bytes, width: img.width(), height: img.height() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(width: u32, height: u32) -> RgbaImage {
        let mut state = 0x2545_f491_u32;
        RgbaImage::from_fn(width, height, |_, _| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let [r, g, b, _] = state.to_le_bytes();
            image::Rgba([r, g, b, 255])
        })
    }

    #[test]
    fn preview_keeps_the_aspect_and_never_exceeds_the_edge() {
        let p = preview(RgbaImage::new(4000, 1000)).unwrap();
        assert_eq!((p.width, p.height), (PREVIEW_MAX_EDGE, 392));
        let p = preview(RgbaImage::new(10, 5000)).unwrap();
        assert_eq!((p.width, p.height), (3, PREVIEW_MAX_EDGE));
    }

    #[test]
    fn a_noisy_preview_switches_to_jpeg() {
        let p = preview(noise(3000, 2000)).unwrap();
        assert_eq!((p.mime, p.width, p.height), ("image/jpeg", PREVIEW_MAX_EDGE, 1045));
        assert!(p.bytes.len() < 5 << 20, "{} bytes is over the ~5 MB hosts accept", p.bytes.len());
    }

    #[test]
    fn a_small_preview_stays_png_at_native_size() {
        let p = preview(noise(200, 100)).unwrap();
        assert_eq!((p.mime, p.width, p.height), ("image/png", 200, 100));
        assert_eq!(image::load_from_memory(&p.bytes).unwrap().into_rgba8(), noise(200, 100));
    }

    #[test]
    fn a_bad_history_id_fails_to_decode() {
        let err = serde_json::from_value::<GetCapture>(serde_json::json!({"id": "../x"})).unwrap_err();
        assert!(err.to_string().contains("invalid history id"), "{err}");
        assert!(serde_json::from_value::<GetCapture>(serde_json::json!({"id": "20261017-101502117-04211"})).is_ok());
    }
}
