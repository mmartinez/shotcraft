//! Screen and window capture on top of xcap (monitors on X11 go through `x11`).
//!
//! Geometry here is in "units": logical points on macOS and physical pixels elsewhere,
//! the space xcap uses for windows and `cursor_position` reports. xcap's X11 monitors are
//! the exception (it divides them by the Xft.dpi scale), so `geometry` undoes that.
//! Captured images are always physical pixels; callers convert through `px_per_unit`.

use emath::{Rect, pos2, vec2};
use image::RgbaImage;

use crate::doc::effects::IRect;
use crate::{Error, Result};

pub struct MonitorShot {
    pub id: u32,
    /// Geometry in units (see module docs).
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub primary: bool,
    pub image: RgbaImage,
}

impl MonitorShot {
    /// Captured pixels per unit.
    pub fn px_per_unit(&self) -> f32 {
        self.image.width() as f32 / self.width.max(1) as f32
    }

    pub fn contains_unit(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width as i32 && y < self.y + self.height as i32
    }

    /// Monitor rectangle in winit logical coordinates, for placing windows on it.
    pub fn logical_rect(&self) -> Rect {
        logical_rect(self.x, self.y, self.width, self.height, self.scale)
    }
}

pub fn logical_rect(x: i32, y: i32, w: u32, h: u32, scale: f32) -> Rect {
    let k = if cfg!(target_os = "macos") { 1.0 } else { 1.0 / scale.max(0.1) };
    Rect::from_min_size(pos2(x as f32 * k, y as f32 * k), vec2(w as f32 * k, h as f32 * k))
}

/// Primary monitor in logical coordinates, without capturing it.
pub fn primary_logical_rect() -> Rect {
    xcap::Monitor::all()
        .ok()
        .and_then(|ms| {
            let m = ms.iter().find(|m| m.is_primary().unwrap_or(false)).or(ms.first())?;
            let (x, y, w, h, scale) = geometry(m).ok()?;
            Some(logical_rect(x, y, w, h, scale))
        })
        .unwrap_or(Rect::from_min_size(pos2(0.0, 0.0), vec2(1920.0, 1080.0)))
}

/// Scale factor of the primary monitor (physical pixels per logical point).
pub fn primary_scale() -> f32 {
    xcap::Monitor::all()
        .ok()
        .and_then(|ms| {
            let m = ms.iter().find(|m| m.is_primary().unwrap_or(false)).or(ms.first())?;
            m.scale_factor().ok()
        })
        .filter(|s| *s > 0.0)
        .unwrap_or(1.0)
}

/// Monitor geometry in units (see module docs) and its scale factor.
fn geometry(m: &xcap::Monitor) -> Result<(i32, i32, u32, u32, f32)> {
    let scale = m.scale_factor().unwrap_or(1.0);
    let k = if cfg!(target_os = "linux") { scale } else { 1.0 };
    let f = |v: f32| (v * k).round();
    Ok((f(m.x()? as f32) as i32, f(m.y()? as f32) as i32, f(m.width()? as f32) as u32, f(m.height()? as f32) as u32, scale))
}

fn shoot(m: &xcap::Monitor) -> Result<MonitorShot> {
    let (x, y, width, height, scale) = geometry(m)?;
    let image = capture_image(m)?;
    // Physical units equal the image size; the X11 round trip in `geometry` can be a pixel off.
    let (width, height) = if cfg!(target_os = "macos") { (width, height) } else { image.dimensions() };
    Ok(MonitorShot { id: m.id()?, x, y, width, height, scale, primary: m.is_primary().unwrap_or(false), image })
}

fn capture_image(m: &xcap::Monitor) -> Result<RgbaImage> {
    #[cfg(target_os = "linux")]
    if !x11::wayland_session()
        && let Some(image) = x11::grab_output(m.id()?).map_err(|e| Error::Other(format!("screen capture: {e}")))?
    {
        return Ok(image);
    }
    m.capture_image().map_err(Error::ScreenRecording)
}

pub fn capture_monitors() -> Result<Vec<MonitorShot>> {
    let monitors = xcap::Monitor::all()?;
    let shots: Vec<_> = monitors.iter().map(shoot).collect::<Result<_>>()?;
    if shots.is_empty() {
        return Err(Error::NoMonitors);
    }
    Ok(shots)
}

/// A fresh capture of one monitor: by id, else the 1-based `display` index (1 = primary),
/// else the monitor under the cursor. An id or index that matches nothing is an error, never another screen.
pub fn capture_monitor(id: Option<u32>, display: Option<usize>) -> Result<MonitorShot> {
    let monitors = xcap::Monitor::all()?;
    let m = if let Some(id) = id {
        monitors.iter().find(|m| m.id().ok() == Some(id)).ok_or(Error::MonitorGone(id))?
    } else if let Some(n) = display {
        n.checked_sub(1).and_then(|i| ordered_displays(&monitors).get(i).copied()).ok_or(Error::NoSuchDisplay(n))?
    } else {
        cursor_position()
            .and_then(|(x, y)| {
                monitors.iter().find(|m| {
                    let Ok((mx, my, w, h, _)) = geometry(m) else { return false };
                    x >= mx && y >= my && x < mx + w as i32 && y < my + h as i32
                })
            })
            .or_else(|| ordered_displays(&monitors).first().copied())
            .ok_or(Error::NoMonitors)?
    };
    shoot(m)
}

/// A monitor kept open for repeated captures (scrolling capture).
pub struct Monitor(xcap::Monitor);

impl Monitor {
    pub fn by_id(id: u32) -> Option<Self> {
        xcap::Monitor::all().ok()?.into_iter().find(|m| m.id().ok() == Some(id)).map(Self)
    }

    pub fn capture(&self) -> Result<RgbaImage> {
        capture_image(&self.0)
    }
}

/// Primary first, then the rest in enumeration order (the `--display` numbering).
fn ordered_displays(monitors: &[xcap::Monitor]) -> Vec<&xcap::Monitor> {
    let mut v: Vec<_> = monitors.iter().collect();
    v.sort_by_key(|m| !m.is_primary().unwrap_or(false));
    v
}

pub fn index_of_display(shots: &[MonitorShot], display: usize) -> Option<usize> {
    let mut order: Vec<usize> = (0..shots.len()).collect();
    order.sort_by_key(|&i| !shots[i].primary);
    order.get(display.saturating_sub(1)).copied()
}

#[cfg(not(target_os = "macos"))]
pub fn cursor_position() -> Option<(i32, i32)> {
    use enigo::Mouse;
    let enigo = enigo::Enigo::new(&enigo::Settings::default()).ok()?;
    enigo.location().ok()
}

/// Read through CoreGraphics: enigo's constructor demands the Accessibility permission on macOS,
/// which reading the cursor does not need.
#[cfg(target_os = "macos")]
pub fn cursor_position() -> Option<(i32, i32)> {
    use objc2_core_graphics::CGEvent;
    let event = CGEvent::new(None)?;
    let p = CGEvent::location(Some(&event));
    Some((p.x.floor() as i32, p.y.floor() as i32))
}

/// Index of the monitor under the cursor, else the primary one.
pub fn cursor_monitor(shots: &[MonitorShot]) -> usize {
    cursor_position().and_then(|(x, y)| shots.iter().position(|m| m.contains_unit(x, y))).or_else(|| shots.iter().position(|m| m.primary)).unwrap_or(0)
}

#[derive(Clone, Debug)]
pub struct WinInfo {
    pub id: u32,
    pub title: String,
    pub app: String,
    /// Geometry in units.
    pub rect: Rect,
}

/// Visible windows of other processes, topmost first.
pub fn list_windows() -> Vec<WinInfo> {
    let me = std::process::id();
    let Ok(all) = xcap::Window::all() else { return Vec::new() };
    all.iter()
        .filter_map(|w| {
            if w.pid().ok() == Some(me) || w.is_minimized().unwrap_or(true) {
                return None;
            }
            let (x, y, width, height) = (w.x().ok()?, w.y().ok()?, w.width().ok()?, w.height().ok()?);
            let id = w.id().ok()?;
            (width >= 40 && height >= 30).then(|| WinInfo {
                id,
                title: w.title().unwrap_or_default(),
                app: w.app_name().unwrap_or_default(),
                rect: Rect::from_min_size(pos2(x as f32, y as f32), vec2(width as f32, height as f32)),
            })
        })
        .collect()
}

pub fn capture_window(id: u32) -> Result<RgbaImage> {
    let win = xcap::Window::all()?.into_iter().find(|w| w.id().ok() == Some(id)).ok_or_else(|| Error::Other(format!("window {id} is gone")))?;
    Ok(win.capture_image()?)
}

pub fn crop(img: &RgbaImage, r: IRect) -> RgbaImage {
    image::imageops::crop_imm(img, r.x, r.y, r.w, r.h).to_image()
}

/// `rect` in units mapped into `shot`'s image pixels.
pub fn unit_rect_to_px(shot: &MonitorShot, rect: Rect) -> Option<IRect> {
    let k = shot.px_per_unit();
    let r = Rect::from_min_max(
        pos2((rect.min.x - shot.x as f32) * k, (rect.min.y - shot.y as f32) * k),
        pos2((rect.max.x - shot.x as f32) * k, (rect.max.y - shot.y as f32) * k),
    );
    IRect::clipped(r, shot.image.width(), shot.image.height())
}

#[cfg(target_os = "linux")]
mod x11 {
    //! Monitor capture on X11. `GetImage` on the root window has returned bands of stale
    //! content on a composited NVIDIA desktop, so the root is first copied into a pixmap with
    //! `IncludeInferiors` (the server draws each window into it) and that pixmap is read.

    use image::RgbaImage;
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::{ConnectionExt as _, CreateGCAux, ImageFormat, ImageOrder, SubwindowMode};

    type BoxError = Box<dyn std::error::Error + Send + Sync>;

    /// The test xcap uses to pick its Wayland backend, which then also captures for us.
    pub fn wayland_session() -> bool {
        std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland") || std::env::var("WAYLAND_DISPLAY").is_ok_and(|d| d.to_lowercase().contains("wayland"))
    }

    /// The monitor driving RandR output `output` (xcap's monitor id), or `None` when the
    /// root's pixel format is not 32 bits per pixel and xcap should read it instead.
    pub fn grab_output(output: u32) -> Result<Option<RgbaImage>, BoxError> {
        let (conn, screen) = x11rb::connect(None)?;
        let setup = conn.setup();
        let root = setup.roots.get(screen).ok_or("no X screen")?.root;
        let monitors = conn.randr_get_monitors(root, true)?.reply()?.monitors;
        let m = monitors.iter().find(|m| m.outputs.contains(&output)).ok_or("the monitor is no longer connected")?;
        let depth = conn.get_geometry(root)?.reply()?.depth;
        if setup.pixmap_formats.iter().find(|f| f.depth == depth).map(|f| f.bits_per_pixel) != Some(32) {
            return Ok(None);
        }
        // The pixmap and GC go away with the connection.
        let pixmap = conn.generate_id()?;
        conn.create_pixmap(depth, pixmap, root, m.width, m.height)?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, pixmap, &CreateGCAux::new().subwindow_mode(SubwindowMode::INCLUDE_INFERIORS))?;
        conn.copy_area(root, pixmap, gc, m.x, m.y, 0, 0, m.width, m.height)?;
        let data = conn.get_image(ImageFormat::Z_PIXMAP, pixmap, 0, 0, m.width, m.height, !0)?.reply()?.data;
        let rgba = to_rgba(data, setup.image_byte_order == ImageOrder::LSB_FIRST);
        Ok(Some(RgbaImage::from_raw(m.width.into(), m.height.into(), rgba).ok_or("the X server returned a short image")?))
    }

    /// 32-bit `xRGB` pixels in the server's byte order to opaque RGBA, in place.
    fn to_rgba(mut data: Vec<u8>, lsb_first: bool) -> Vec<u8> {
        for p in data.as_chunks_mut::<4>().0 {
            let [a, b, c, d] = *p;
            *p = if lsb_first { [c, b, a, 255] } else { [b, c, d, 255] };
        }
        data
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn pixels_convert_from_either_byte_order() {
            // 0x00112233 (red 0x11, green 0x22, blue 0x33) as the server sends it.
            assert_eq!(super::to_rgba(vec![0x33, 0x22, 0x11, 0x00], true), [0x11, 0x22, 0x33, 255]);
            assert_eq!(super::to_rgba(vec![0x00, 0x11, 0x22, 0x33], false), [0x11, 0x22, 0x33, 255]);
        }
    }
}
