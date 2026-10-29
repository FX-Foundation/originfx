// SPDX-License-Identifier: GPL-3.0-or-later
//! Blur, sharpen and pixelate. Colour and alpha are processed together in premultiplied form so
//! transparent areas never bleed a dark or bright fringe.

/// Straight RGBA8 to premultiplied floats (0 to 255).
fn premultiply(rgba: &[u8]) -> Vec<f32> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        let a = f32::from(px[3]) / 255.0;
        out.extend([
            f32::from(px[0]) * a,
            f32::from(px[1]) * a,
            f32::from(px[2]) * a,
            f32::from(px[3]),
        ]);
    }
    out
}

/// Premultiplied floats back to straight RGBA8.
fn unpremultiply(buf: &[f32], rgba: &mut [u8]) {
    for (px, o) in buf.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
        let a = px[3];
        if a <= 0.0 {
            o.copy_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let k = 255.0 / a;
        for c in 0..3 {
            o[c] = (px[c] * k + 0.5).clamp(0.0, 255.0) as u8;
        }
        o[3] = (a + 0.5).clamp(0.0, 255.0) as u8;
    }
}

/// Box widths whose three passes approximate a gaussian of standard deviation `sigma`.
fn box_sizes(sigma: f64) -> [usize; 3] {
    let n = 3.0;
    let ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut lower = ideal.floor() as i64;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let lower = lower.max(1);
    let upper = lower + 2;
    let m = ((12.0 * sigma * sigma - n * (lower * lower) as f64 - 4.0 * n * lower as f64 - 3.0 * n)
        / (-4.0 * lower as f64 - 4.0))
        .round() as i64;
    let mut sizes = [0usize; 3];
    for (i, s) in sizes.iter_mut().enumerate() {
        *s = if (i as i64) < m { lower } else { upper } as usize;
    }
    sizes
}

/// One running-sum box pass along rows (`horizontal`) or columns, with edges extended.
fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, size: usize, horizontal: bool) {
    let r = (size / 2) as i64;
    let (lines, len, step_line, step_pos) = if horizontal {
        (h, w, w * 4, 4)
    } else {
        (w, h, 4, w * 4)
    };
    let inv = 1.0 / size as f32;
    for line in 0..lines {
        let base = line * step_line;
        for c in 0..4 {
            let at =
                |i: i64| -> f32 { src[base + i.clamp(0, len as i64 - 1) as usize * step_pos + c] };
            let mut sum = 0.0f32;
            for i in -r..=r {
                sum += at(i);
            }
            for i in 0..len as i64 {
                dst[base + i as usize * step_pos + c] = sum * inv;
                sum += at(i + r + 1) - at(i - r);
            }
        }
    }
}

fn gaussian(buf: &mut [f32], w: usize, h: usize, sigma: f64) {
    let mut tmp = vec![0f32; buf.len()];
    for size in box_sizes(sigma) {
        if size <= 1 {
            continue;
        }
        box_pass(buf, &mut tmp, w, h, size, true);
        box_pass(&tmp, buf, w, h, size, false);
    }
}

/// Blur with a gaussian of standard deviation `sigma_px` pixels.
pub fn blur(rgba: &mut [u8], w: usize, h: usize, sigma_px: f64) {
    if sigma_px < 0.3 {
        return;
    }
    let mut buf = premultiply(rgba);
    gaussian(&mut buf, w, h, sigma_px);
    unpremultiply(&buf, rgba);
}

/// Unsharp mask: add `amount` times the difference between the picture and its blur.
pub fn sharpen(rgba: &mut [u8], w: usize, h: usize, amount: f64, sigma_px: f64) {
    let sigma = sigma_px.max(0.3);
    let original = premultiply(rgba);
    let mut soft = original.clone();
    gaussian(&mut soft, w, h, sigma);
    let k = amount as f32;
    let mut out = original.clone();
    for ((o, a), b) in out
        .chunks_exact_mut(4)
        .zip(original.chunks_exact(4))
        .zip(soft.chunks_exact(4))
    {
        // Sharpen the colour only; alpha stays as it was.
        for c in 0..3 {
            o[c] = (a[c] + (a[c] - b[c]) * k).clamp(0.0, a[3]);
        }
    }
    unpremultiply(&out, rgba);
}

/// Replace each `block` x `block` square (block in pixels) with its average.
pub fn pixelate(rgba: &mut [u8], w: usize, h: usize, block_px: f64) {
    let block = block_px.round() as usize;
    if block <= 1 {
        return;
    }
    let buf = premultiply(rgba);
    let mut out = vec![0f32; buf.len()];
    for by in (0..h).step_by(block) {
        for bx in (0..w).step_by(block) {
            let (x1, y1) = ((bx + block).min(w), (by + block).min(h));
            let mut sum = [0f32; 4];
            for y in by..y1 {
                for x in bx..x1 {
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += buf[(y * w + x) * 4 + c];
                    }
                }
            }
            let n = ((x1 - bx) * (y1 - by)) as f32;
            for y in by..y1 {
                for x in bx..x1 {
                    for (c, s) in sum.iter().enumerate() {
                        out[(y * w + x) * 4 + c] = s / n;
                    }
                }
            }
        }
    }
    unpremultiply(&out, rgba);
}
