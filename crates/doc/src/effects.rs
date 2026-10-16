//! Pixel effects on RGBA8 buffers: blur, pixelation and margin balancing.

use image::RgbaImage;

/// Integer rectangle in pixels, clipped by callers to the buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl IRect {
    /// Rounds a rect outward and clips it to a `width`x`height` buffer.
    pub fn clipped(r: emath::Rect, width: u32, height: u32) -> Option<IRect> {
        let x0 = r.min.x.floor().clamp(0.0, width as f32) as u32;
        let y0 = r.min.y.floor().clamp(0.0, height as f32) as u32;
        let x1 = r.max.x.ceil().clamp(0.0, width as f32) as u32;
        let y1 = r.max.y.ceil().clamp(0.0, height as f32) as u32;
        (x1 > x0 && y1 > y0).then(|| IRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }
}

fn copy_out(data: &[u8], width: u32, r: IRect) -> Vec<u8> {
    let mut out = Vec::with_capacity((r.w * r.h * 4) as usize);
    for y in r.y..r.y + r.h {
        let start = ((y * width + r.x) * 4) as usize;
        out.extend_from_slice(&data[start..start + (r.w * 4) as usize]);
    }
    out
}

fn copy_in(data: &mut [u8], width: u32, r: IRect, src: &[u8]) {
    let row = (r.w * 4) as usize;
    for (i, y) in (r.y..r.y + r.h).enumerate() {
        let start = ((y * width + r.x) * 4) as usize;
        data[start..start + row].copy_from_slice(&src[i * row..(i + 1) * row]);
    }
}

/// One box-blur pass along rows (`stride` 4) or columns (`stride` 4*w); edges are clamped.
fn box_pass(src: &[u8], dst: &mut [u8], len: usize, lines: usize, step: usize, line_step: usize, r: usize) {
    let div = (2 * r + 1) as u32;
    let last = len - 1;
    for line in 0..lines {
        let base = line * line_step;
        for c in 0..4 {
            let at = |i: usize| src[base + i * step + c] as u32;
            let mut acc: u32 = (0..=2 * r).map(|k| at(k.saturating_sub(r).min(last))).sum();
            for i in 0..len {
                dst[base + i * step + c] = ((acc + div / 2) / div) as u8;
                acc += at((i + r + 1).min(last));
                acc -= at(i.saturating_sub(r));
            }
        }
    }
}

/// Three box passes per axis, a close approximation of a gaussian with sigma ~ radius.
pub fn blur_buffer(buf: &mut [u8], w: u32, h: u32, radius: u32) {
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    let (w, h) = (w as usize, h as usize);
    let r = (radius as usize).min(w.max(h));
    let mut tmp = vec![0u8; buf.len()];
    for _ in 0..3 {
        box_pass(buf, &mut tmp, w, h, 4, w * 4, r.min(w));
        box_pass(&tmp, buf, h, w, w * 4, 4, r.min(h));
    }
}

pub fn blur_region(data: &mut [u8], width: u32, r: IRect, radius: u32) {
    let mut region = copy_out(data, width, r);
    blur_buffer(&mut region, r.w, r.h, radius);
    copy_in(data, width, r, &region);
}

fn mix(seed: u64, a: u32, b: u32) -> u64 {
    // splitmix64 over the block coordinates: deterministic, so re-rendering is stable.
    let mut z = seed ^ ((a as u64) << 32 | b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Replace each `block`-sized cell with its average colour plus per-cell noise.
/// `block` may be fractional (a scaled preview): cell edges fall on multiples of it, so the preview keeps the
/// export's grid. Below one pixel a cell is finer than the buffer and nothing is drawn.
pub fn pixelate_region(data: &mut [u8], width: u32, r: IRect, block: f32, seed: u64) {
    if block.is_nan() || block < 1.0 {
        return;
    }
    // f64 keeps consecutive edges at least one pixel apart for any block >= 1.
    let edge = |k: u32, len: u32| ((k as f64 * block as f64) as u32).min(len);
    let mut ky = 0;
    while edge(ky, r.h) < r.h {
        let (y0, y1) = (edge(ky, r.h), edge(ky + 1, r.h));
        let mut kx = 0;
        while edge(kx, r.w) < r.w {
            let (x0, x1) = (edge(kx, r.w), edge(kx + 1, r.w));
            let mut sum = [0u64; 4];
            for y in r.y + y0..r.y + y1 {
                for x in r.x + x0..r.x + x1 {
                    let i = ((y * width + x) * 4) as usize;
                    for c in 0..4 {
                        sum[c] += data[i + c] as u64;
                    }
                }
            }
            let n = ((x1 - x0) as u64 * (y1 - y0) as u64).max(1);
            // ±12 brightness jitter, the same on R, G and B so blocks don't pick up colour casts.
            let jitter = (mix(seed, kx, ky) & 0xff) as i32 * 24 / 255 - 12;
            let mut avg = [0u8; 4];
            for c in 0..4 {
                let mean = (sum[c] / n) as i32;
                // Alpha untouched to keep premultiplication valid.
                let j = if c < 3 { jitter } else { 0 };
                avg[c] = (mean + j).clamp(0, 255) as u8;
            }
            for c in 0..3 {
                avg[c] = avg[c].min(avg[3]);
            }
            for y in r.y + y0..r.y + y1 {
                for x in r.x + x0..r.x + x1 {
                    let i = ((y * width + x) * 4) as usize;
                    data[i..i + 4].copy_from_slice(&avg);
                }
            }
            kx += 1;
        }
        ky += 1;
    }
}

fn close(a: &[u8], b: &[u8], tol: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= tol)
}

/// Shrink `r` so every side keeps the same amount of uniform background margin
/// (the smallest one found), centring content that was captured off-centre.
pub fn balanced_rect(img: &RgbaImage, r: IRect, tol: u8) -> IRect {
    if r.w < 3 || r.h < 3 {
        return r;
    }
    let px = |x: u32, y: u32| img.get_pixel(x, y).0;
    let reference = px(r.x, r.y);
    let corners = [px(r.x + r.w - 1, r.y), px(r.x, r.y + r.h - 1), px(r.x + r.w - 1, r.y + r.h - 1)];
    if !corners.iter().all(|c| close(c, &reference, tol)) {
        return r;
    }
    let row_uniform = |y: u32| (r.x..r.x + r.w).all(|x| close(&px(x, y), &reference, tol));
    let col_uniform = |x: u32| (r.y..r.y + r.h).all(|y| close(&px(x, y), &reference, tol));
    let top = (0..r.h).take_while(|&i| row_uniform(r.y + i)).count() as u32;
    if top == r.h {
        return r; // Entirely uniform: nothing to balance.
    }
    let bottom = (0..r.h).take_while(|&i| row_uniform(r.y + r.h - 1 - i)).count() as u32;
    let left = (0..r.w).take_while(|&i| col_uniform(r.x + i)).count() as u32;
    let right = (0..r.w).take_while(|&i| col_uniform(r.x + r.w - 1 - i)).count() as u32;
    let m = top.min(bottom).min(left).min(right);
    IRect { x: r.x + (left - m), y: r.y + (top - m), w: r.w - (left - m) - (right - m), h: r.h - (top - m) - (bottom - m) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipped_keeps_every_partially_covered_pixel() {
        // Redaction relies on this: a sliver of coverage must still be redacted.
        let r = |x0, y0, x1, y1| emath::Rect::from_min_max(emath::pos2(x0, y0), emath::pos2(x1, y1));
        assert_eq!(IRect::clipped(r(9.995, 9.995, 20.005, 20.005), 100, 100), Some(IRect { x: 9, y: 9, w: 12, h: 12 }));
        assert_eq!(IRect::clipped(r(-5.0, -5.0, 2000.0, 2000.0), 1920, 1080), Some(IRect { x: 0, y: 0, w: 1920, h: 1080 }));
    }

    #[test]
    fn blur_preserves_flat_colour_and_spreads_edges() {
        let (w, h) = (20u32, 10u32);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 10..w {
                let i = ((y * w + x) * 4) as usize;
                buf[i..i + 4].copy_from_slice(&[200, 100, 50, 255]);
            }
        }
        let alpha_before: u32 = buf.iter().skip(3).step_by(4).map(|&a| a as u32).sum();
        blur_buffer(&mut buf, w, h, 2);
        let at = |x: u32, y: u32| buf[((y * w + x) * 4) as usize];
        assert_eq!(at(0, 5), 0, "far left stays black");
        assert_eq!(at(19, 5), 200, "far right keeps colour");
        assert!(at(9, 5) > 0 && at(10, 5) < 200, "edge is softened");
        let alpha_after: u32 = buf.iter().skip(3).step_by(4).map(|&a| a as u32).sum();
        assert!(alpha_before.abs_diff(alpha_after) < alpha_before / 50, "blur roughly conserves energy");
    }

    #[test]
    fn pixelate_makes_uniform_blocks_inside_region_only() {
        let w = 16u32;
        let mut buf: Vec<u8> = (0..w * w * 4).map(|i| (i * 7 % 251) as u8).collect();
        for a in buf.iter_mut().skip(3).step_by(4) {
            *a = 255;
        }
        let before = buf.clone();
        let r = IRect { x: 4, y: 4, w: 8, h: 8 };
        pixelate_region(&mut buf, w, r, 4.0, 42);
        let px = |b: &[u8], x: u32, y: u32| b[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize].to_vec();
        assert_eq!(px(&buf, 4, 4), px(&buf, 7, 7), "one block is one colour");
        assert_eq!(px(&buf, 0, 0), px(&before, 0, 0), "outside untouched");
        assert_eq!(px(&buf, 12, 12), px(&before, 12, 12), "outside untouched");
    }

    #[test]
    fn pixelate_noise_depends_on_seed() {
        let w = 8u32;
        let base = vec![128u8; (w * w * 4) as usize];
        let r = IRect { x: 0, y: 0, w: 8, h: 8 };
        let (mut a, mut b) = (base.clone(), base.clone());
        pixelate_region(&mut a, w, r, 4.0, 1);
        pixelate_region(&mut b, w, r, 4.0, 2);
        assert_ne!(a, b);
    }

    #[test]
    fn balance_trims_uneven_margins_to_the_smallest() {
        // White 100x60 canvas, dark content at x 30..60, y 10..40: margins L30 R40 T10 B20.
        let mut img = RgbaImage::from_pixel(100, 60, image::Rgba([255, 255, 255, 255]));
        for y in 10..40 {
            for x in 30..60 {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let r = balanced_rect(&img, IRect { x: 0, y: 0, w: 100, h: 60 }, 2);
        assert_eq!(r, IRect { x: 20, y: 0, w: 50, h: 50 });
    }

    #[test]
    fn balance_leaves_non_uniform_borders_alone() {
        let mut img = RgbaImage::from_pixel(10, 10, image::Rgba([255, 255, 255, 255]));
        img.put_pixel(9, 9, image::Rgba([0, 0, 0, 255]));
        let r = IRect { x: 0, y: 0, w: 10, h: 10 };
        assert_eq!(balanced_rect(&img, r, 2), r);
    }
}
