//! Joins successive frames of a scrolling region into one tall image.
//!
//! Each frame is matched against the previous one by row hashes: the scroll offset is the
//! shift that makes the most rows line up. Rows that stay put in both frames at the top
//! (sticky headers) and bottom (sticky footers) are excluded from matching; the header is
//! kept once from the first frame and the footer once from the last.

use image::RgbaImage;

const MIN_SCORE: f32 = 0.85;
const MIN_COMPARED_ROWS: usize = 6;

#[derive(Debug, PartialEq, Eq)]
pub enum Push {
    /// `n` new rows were appended.
    Appended(usize),
    /// The frame matches the previous one (not scrolled yet, or the end was reached).
    Unchanged,
    /// No reliable offset; the frame was ignored.
    NoMatch,
}

pub struct Stitcher {
    width: u32,
    height: u32,
    /// Output rows so far (RGBA), excluding the sticky footer.
    out: Vec<u8>,
    last: Option<Frame>,
    footer: Option<usize>,
    max_height: u32,
}

struct Frame {
    img: RgbaImage,
    hashes: Vec<u64>,
    uniform: Vec<bool>,
}

impl Frame {
    fn new(img: RgbaImage) -> Self {
        let row = img.width() as usize * 4;
        let raw = img.as_raw();
        let mut hashes = Vec::with_capacity(img.height() as usize);
        let mut uniform = Vec::with_capacity(img.height() as usize);
        for r in raw.chunks_exact(row) {
            hashes.push(r.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3)));
            uniform.push(r.as_chunks::<4>().0.iter().all(|p| p[..] == r[..4]));
        }
        Self { img, hashes, uniform }
    }
}

impl Stitcher {
    pub fn new(max_height: u32) -> Self {
        Self { width: 0, height: 0, out: Vec::new(), last: None, footer: None, max_height }
    }

    pub fn height(&self) -> u32 {
        let footer = self.footer.unwrap_or(0) as u32;
        (self.out.len() / (self.width.max(1) as usize * 4)) as u32 + footer
    }

    pub fn is_full(&self) -> bool {
        self.height() >= self.max_height
    }

    pub fn push(&mut self, img: RgbaImage) -> Push {
        // A display that shrank mid-capture can crop to nothing; there are no rows to match.
        if img.width() == 0 || img.height() == 0 {
            return Push::NoMatch;
        }
        let frame = Frame::new(img);
        let Some(prev) = &self.last else {
            self.width = frame.img.width();
            self.height = frame.img.height();
            self.out = frame.img.as_raw().clone();
            self.last = Some(frame);
            return Push::Appended(self.height as usize);
        };
        if frame.img.dimensions() != (self.width, self.height) {
            return Push::NoMatch;
        }
        if frame.hashes == prev.hashes {
            return Push::Unchanged;
        }
        let h = self.height as usize;
        let row = self.width as usize * 4;
        let same = |i: usize| prev.hashes[i] == frame.hashes[i];
        let header = (0..h / 3).take_while(|&i| same(i)).count();
        let footer = self.footer.unwrap_or_else(|| (0..h / 3).take_while(|&i| same(h - 1 - i)).count());
        let Some(dy) = find_offset(prev, &frame, header, footer) else { return Push::NoMatch };
        if self.footer.is_none() {
            // First match: the first frame was appended whole, but its footer belongs at the very end.
            self.footer = Some(footer);
            self.out.truncate((h - footer) * row);
        }
        let room = (self.max_height as usize).saturating_sub(self.height() as usize);
        let take = dy.min(room);
        let start = h - footer - dy;
        self.out.extend_from_slice(&frame.img.as_raw()[start * row..(start + take) * row]);
        self.last = Some(frame);
        Push::Appended(take)
    }

    /// The most recent `rows` stitched rows, for a live preview.
    pub fn tail(&self, rows: u32) -> Option<RgbaImage> {
        let row = self.width as usize * 4;
        let total = self.out.len() / row.max(1);
        let n = (rows as usize).min(total);
        if n == 0 {
            return None;
        }
        RgbaImage::from_raw(self.width, n as u32, self.out[(total - n) * row..].to_vec())
    }

    /// The stitched image, with the latest footer re-attached.
    pub fn finish(self) -> Option<RgbaImage> {
        let last = self.last?;
        let mut out = self.out;
        let row = self.width as usize * 4;
        let h = self.height as usize;
        if let Some(footer) = self.footer {
            out.extend_from_slice(&last.img.as_raw()[(h - footer) * row..h * row]);
        }
        let rows = (out.len() / row) as u32;
        RgbaImage::from_raw(self.width, rows, out)
    }
}

/// Scroll distance `dy` such that `prev` row `k + dy` equals `cur` row `k` across the
/// non-sticky band, ignoring featureless (single-colour) rows.
fn find_offset(prev: &Frame, cur: &Frame, header: usize, footer: usize) -> Option<usize> {
    let h = prev.hashes.len();
    let band_end = h - footer;
    let mut best: Option<(usize, f32)> = None;
    for dy in 1..band_end.saturating_sub(header + MIN_COMPARED_ROWS) {
        let mut compared = 0usize;
        let mut matched = 0usize;
        for k in header..band_end - dy {
            if cur.uniform[k] && prev.uniform[k + dy] {
                continue;
            }
            compared += 1;
            if prev.hashes[k + dy] == cur.hashes[k] {
                matched += 1;
            }
        }
        if compared < MIN_COMPARED_ROWS {
            continue;
        }
        let score = matched as f32 / compared as f32;
        if score >= MIN_SCORE && best.is_none_or(|(_, s)| score > s) {
            best = Some((dy, score));
        }
    }
    best.map(|(dy, _)| dy)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tall page whose every row is distinct noise.
    fn page(w: u32, h: u32) -> RgbaImage {
        let mut state = 0x1234_5678u32;
        RgbaImage::from_fn(w, h, |_, _| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let b = state.to_le_bytes();
            image::Rgba([b[0], b[1], b[2], 255])
        })
    }

    fn view(page: &RgbaImage, top: u32, h: u32) -> RgbaImage {
        image::imageops::crop_imm(page, 0, top, page.width(), h).to_image()
    }

    #[test]
    fn stitches_plain_scroll_exactly() {
        let p = page(16, 300);
        let mut s = Stitcher::new(10_000);
        for top in [0, 37, 80, 150, 200] {
            assert!(matches!(s.push(view(&p, top, 100)), Push::Appended(_)));
        }
        assert_eq!(s.push(view(&p, 200, 100)), Push::Unchanged);
        assert_eq!(s.finish().unwrap(), p);
    }

    #[test]
    fn keeps_sticky_header_and_footer_once() {
        let p = page(12, 400);
        let header = page(12, 10);
        let footer = view(&page(12, 50), 30, 15);
        let frame = |top: u32| {
            let mut f = view(&p, top, 120);
            image::imageops::replace(&mut f, &header, 0, 0);
            image::imageops::replace(&mut f, &footer, 0, 105);
            f
        };
        let mut s = Stitcher::new(10_000);
        for top in [0, 40, 95, 160] {
            s.push(frame(top));
        }
        let out = s.finish().unwrap();
        // header(10) + page rows 10..(160+105) + footer(15)
        assert_eq!(out.height(), 10 + (160 + 105 - 10) + 15);
        assert_eq!(view(&out, 0, 10), header);
        assert_eq!(view(&out, 10, 255), view(&p, 10, 255));
        assert_eq!(view(&out, out.height() - 15, 15), footer);
    }

    #[test]
    fn ignores_unrelated_frame() {
        let mut s = Stitcher::new(10_000);
        s.push(page(8, 60));
        let other = RgbaImage::from_fn(8, 60, |x, y| image::Rgba([(x * 31) as u8, (y * 7) as u8, 99, 255]));
        assert_eq!(s.push(other), Push::NoMatch);
    }

    #[test]
    fn stops_at_max_height() {
        let p = page(8, 400);
        let mut s = Stitcher::new(150);
        s.push(view(&p, 0, 100));
        s.push(view(&p, 90, 100));
        assert!(s.is_full());
        assert_eq!(s.finish().unwrap().height(), 150);
    }

    #[test]
    fn empty_frames_are_skipped() {
        let p = page(8, 200);
        let mut s = Stitcher::new(10_000);
        assert_eq!(s.push(RgbaImage::new(0, 100)), Push::NoMatch, "nothing to start from");
        s.push(view(&p, 0, 100));
        assert_eq!(s.push(RgbaImage::new(0, 100)), Push::NoMatch);
        assert!(matches!(s.push(view(&p, 40, 100)), Push::Appended(40)));
    }
}
