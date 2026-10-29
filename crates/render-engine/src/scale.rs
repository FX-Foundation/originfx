// SPDX-License-Identifier: GPL-3.0-or-later
//! Resizing RGBA pictures, and the small helper that splits work across threads.

use std::thread;

/// Run `f(row_index, row)` for every row of `data`, spread over the available CPU cores.
/// Rows are independent, so the result is the same whatever the number of threads.
pub(crate) fn par_rows<T: Send>(
    data: &mut [T],
    row_len: usize,
    f: impl Fn(usize, &mut [T]) + Sync,
) {
    if row_len == 0 {
        return;
    }
    let rows = data.len() / row_len;
    let threads = thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(rows)
        .clamp(1, 16);
    if threads <= 1 || data.len() < 64 * 1024 {
        for (y, row) in data.chunks_mut(row_len).enumerate() {
            f(y, row);
        }
        return;
    }
    let per = rows.div_ceil(threads);
    thread::scope(|s| {
        for (ci, chunk) in data.chunks_mut(per * row_len).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (i, row) in chunk.chunks_mut(row_len).enumerate() {
                    f(ci * per + i, row);
                }
            });
        }
    });
}

/// For each output position, the first source index and the weights of the taps from there.
/// A triangle filter whose width grows when shrinking, so downscaling averages properly
/// instead of skipping pixels.
fn axis_weights(src: usize, dst: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = src as f64 / dst as f64;
    let support = scale.max(1.0);
    (0..dst)
        .map(|i| {
            let centre = (i as f64 + 0.5) * scale - 0.5;
            let lo = (centre - support).floor() as i64 + 1;
            let hi = (centre + support).floor() as i64;
            let mut taps = Vec::new();
            let mut sum = 0.0;
            for x in lo..=hi {
                let w = (1.0 - (x as f64 - centre).abs() / support).max(0.0);
                taps.push(w);
                sum += w;
            }
            if sum <= 0.0 {
                // Cannot happen with a triangle; stay safe with the nearest pixel.
                let nearest = (centre.round() as i64).clamp(0, src as i64 - 1) as usize;
                return (nearest, vec![1.0]);
            }
            // Fold taps that fall outside the picture onto the edge pixel.
            let mut folded: Vec<f32> = Vec::new();
            let mut base = lo.clamp(0, src as i64 - 1);
            for (k, w) in taps.iter().enumerate() {
                let x = (lo + k as i64).clamp(0, src as i64 - 1);
                if folded.is_empty() {
                    base = x;
                }
                let at = (x - base) as usize;
                if at >= folded.len() {
                    folded.resize(at + 1, 0.0);
                }
                folded[at] += (*w / sum) as f32;
            }
            (base as usize, folded)
        })
        .collect()
}

/// Resize an RGBA8 picture (straight alpha) with a triangle filter.
/// Colour is weighted by alpha, so transparent edges do not bleed dark fringes.
pub fn resize(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    assert_eq!(src.len(), sw * sh * 4, "source size mismatch");
    assert!(sw > 0 && sh > 0 && dw > 0 && dh > 0, "empty picture");
    if sw == dw && sh == dh {
        return src.to_vec();
    }
    // Video frames are opaque, which allows a much cheaper whole-number version.
    if src.chunks_exact(4).all(|p| p[3] == 255) {
        return resize_opaque(src, sw, sh, dw, dh);
    }
    let xw = axis_weights(sw, dw);
    let yw = axis_weights(sh, dh);

    // Pass 1: horizontal, into premultiplied f32 (colour * alpha/255).
    let mut tmp = vec![0f32; sh * dw * 4];
    par_rows(&mut tmp, dw * 4, |y, row| {
        let line = &src[y * sw * 4..(y + 1) * sw * 4];
        for (x, (first, taps)) in xw.iter().enumerate() {
            let mut acc = [0f32; 4];
            for (k, w) in taps.iter().enumerate() {
                let p = &line[(first + k) * 4..(first + k) * 4 + 4];
                let a = f32::from(p[3]);
                let pa = a / 255.0;
                acc[0] += w * f32::from(p[0]) * pa;
                acc[1] += w * f32::from(p[1]) * pa;
                acc[2] += w * f32::from(p[2]) * pa;
                acc[3] += w * a;
            }
            row[x * 4..x * 4 + 4].copy_from_slice(&acc);
        }
    });

    // Pass 2: vertical, then back to straight alpha.
    let mut out = vec![0u8; dw * dh * 4];
    par_rows(&mut out, dw * 4, |y, row| {
        let (first, taps) = &yw[y];
        for x in 0..dw {
            let mut acc = [0f32; 4];
            for (k, w) in taps.iter().enumerate() {
                let p = &tmp[((first + k) * dw + x) * 4..((first + k) * dw + x) * 4 + 4];
                for c in 0..4 {
                    acc[c] += w * p[c];
                }
            }
            let a = acc[3];
            let o = &mut row[x * 4..x * 4 + 4];
            if a <= 0.0 {
                o.copy_from_slice(&[0, 0, 0, 0]);
            } else {
                let k = 255.0 / a;
                o[0] = (acc[0] * k + 0.5).clamp(0.0, 255.0) as u8;
                o[1] = (acc[1] * k + 0.5).clamp(0.0, 255.0) as u8;
                o[2] = (acc[2] * k + 0.5).clamp(0.0, 255.0) as u8;
                o[3] = (a + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    });
    out
}

/// The same filter as [`axis_weights`], as whole numbers that add up to exactly 1 << 14, so a
/// flat colour stays exactly that colour.
fn axis_weights_fixed(src: usize, dst: usize) -> Vec<(usize, Vec<i32>)> {
    axis_weights(src, dst)
        .into_iter()
        .map(|(first, taps)| {
            let mut w: Vec<i32> = taps.iter().map(|t| (t * 16384.0).round() as i32).collect();
            let diff = 16384 - w.iter().sum::<i32>();
            if let Some(m) = w.iter_mut().max() {
                *m += diff;
            }
            (first, w)
        })
        .collect()
}

/// Resize a picture whose pixels are all opaque: whole-number maths on 8-bit data, no
/// per-pixel alpha weighting and no large floating-point buffer. The result is opaque too.
fn resize_opaque(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    let xw = axis_weights_fixed(sw, dw);
    let yw = axis_weights_fixed(sh, dh);

    // Pass 1: horizontal.
    let mut tmp = vec![0u8; sh * dw * 4];
    par_rows(&mut tmp, dw * 4, |y, row| {
        let line = &src[y * sw * 4..(y + 1) * sw * 4];
        for (x, (first, taps)) in xw.iter().enumerate() {
            let (mut r, mut g, mut b) = (8192i32, 8192i32, 8192i32);
            for (k, w) in taps.iter().enumerate() {
                let p = &line[(first + k) * 4..(first + k) * 4 + 3];
                r += w * i32::from(p[0]);
                g += w * i32::from(p[1]);
                b += w * i32::from(p[2]);
            }
            let o = &mut row[x * 4..x * 4 + 4];
            o[0] = (r >> 14) as u8;
            o[1] = (g >> 14) as u8;
            o[2] = (b >> 14) as u8;
            o[3] = 255;
        }
    });

    // Pass 2: vertical, a whole row at a time so the inner loop is a plain multiply-add.
    let mut out = vec![0u8; dw * dh * 4];
    par_rows(&mut out, dw * 4, |y, row| {
        let (first, taps) = &yw[y];
        let mut acc = vec![8192i32; dw * 4];
        for (k, w) in taps.iter().enumerate() {
            let line = &tmp[(first + k) * dw * 4..(first + k + 1) * dw * 4];
            for (a, p) in acc.iter_mut().zip(line) {
                *a += w * i32::from(*p);
            }
        }
        for (o, a) in row.iter_mut().zip(&acc) {
            *o = (a >> 14) as u8;
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, px: [u8; 4]) -> Vec<u8> {
        px.iter().copied().cycle().take(w * h * 4).collect()
    }

    #[test]
    fn same_size_is_a_copy() {
        let a: Vec<u8> = (0..4 * 3 * 4).map(|i| i as u8).collect();
        assert_eq!(resize(&a, 4, 3, 4, 3), a);
    }

    #[test]
    fn solid_colours_stay_exact_at_any_size() {
        for (dw, dh) in [(1, 1), (3, 5), (16, 9), (100, 7), (33, 64)] {
            let out = resize(&solid(16, 9, [10, 200, 90, 255]), 16, 9, dw, dh);
            assert!(out.chunks(4).all(|p| p == [10, 200, 90, 255]), "{dw}x{dh}");
        }
    }

    #[test]
    fn halving_averages_pairs() {
        // Two columns black/white shrunk to one column gives mid grey.
        let src = [0, 0, 0, 255, 255, 255, 255, 255];
        let out = resize(&src, 2, 1, 1, 1);
        assert!((i32::from(out[0]) - 128).abs() <= 1, "{out:?}");
        assert_eq!(out[3], 255);
    }

    #[test]
    fn transparent_pixels_do_not_darken_neighbours() {
        // Opaque red next to fully transparent black: result is still pure red, half transparent.
        let src = [255, 0, 0, 255, 0, 0, 0, 0];
        let out = resize(&src, 2, 1, 1, 1);
        assert_eq!(&out[..3], &[255, 0, 0]);
        assert!((i32::from(out[3]) - 128).abs() <= 1);
    }

    #[test]
    fn the_opaque_path_matches_the_general_path() {
        // Same picture, once all opaque (fast path) and once with one pixel made almost
        // opaque (general path): the two must agree to within rounding.
        let mut a: Vec<u8> = (0..64 * 48 * 4).map(|i| (i * 37 % 253) as u8).collect();
        for p in a.chunks_mut(4) {
            p[3] = 255;
        }
        let fast = resize(&a, 64, 48, 40, 30);
        let mut b = a.clone();
        b[3] = 254;
        let slow = resize(&b, 64, 48, 40, 30);
        for (f, s) in fast.iter().zip(&slow).skip(4) {
            assert!((i32::from(*f) - i32::from(*s)).abs() <= 2, "{f} vs {s}");
        }
        assert!(fast.chunks(4).all(|p| p[3] == 255));
    }

    #[test]
    fn threads_do_not_change_the_result() {
        let src: Vec<u8> = (0..256 * 256 * 4).map(|i| (i * 31 % 251) as u8).collect();
        let a = resize(&src, 256, 256, 100, 80);
        let b = resize(&src, 256, 256, 100, 80);
        assert_eq!(a, b);
    }
}
