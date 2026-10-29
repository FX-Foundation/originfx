// SPDX-License-Identifier: GPL-3.0-or-later
//! Placing a picture on the canvas and blending layers.

use crate::scale::par_rows;

/// How a picture of a different shape is fitted onto the sequence's frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fit {
    /// Show all of it, with bars at the sides if the shapes differ (the usual default).
    #[default]
    Contain,
    /// Fill the frame, cutting off whatever hangs over.
    Cover,
    /// Fill the frame by stretching, changing the shape.
    Stretch,
}

/// Where a picture lands on the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub width: usize,
    pub height: usize,
    /// Position of the picture's top-left corner; negative when it hangs over the edge.
    pub x: i64,
    pub y: i64,
}

/// Size and position of a `sw` x `sh` picture on a `cw` x `ch` canvas.
pub fn placement(sw: usize, sh: usize, cw: usize, ch: usize, fit: Fit) -> Placement {
    if sw == 0 || sh == 0 {
        return Placement {
            width: 1,
            height: 1,
            x: 0,
            y: 0,
        };
    }
    let (w, h) = match fit {
        Fit::Stretch => (cw, ch),
        Fit::Contain | Fit::Cover => {
            let sx = cw as f64 / sw as f64;
            let sy = ch as f64 / sh as f64;
            let s = if fit == Fit::Contain {
                sx.min(sy)
            } else {
                sx.max(sy)
            };
            let w = ((sw as f64 * s).round() as usize).max(1);
            let h = ((sh as f64 * s).round() as usize).max(1);
            // The side that should exactly fill the canvas does so despite rounding.
            match (fit, sx <= sy) {
                (Fit::Contain, true) | (Fit::Cover, false) => (cw, h),
                _ => (w, ch),
            }
        }
    };
    Placement {
        width: w,
        height: h,
        x: (cw as i64 - w as i64) / 2,
        y: (ch as i64 - h as i64) / 2,
    }
}

/// A canvas filled with one colour.
pub fn solid(w: usize, h: usize, rgba: [u8; 4]) -> Vec<u8> {
    let mut v = Vec::with_capacity(w * h * 4);
    for _ in 0..w * h {
        v.extend_from_slice(&rgba);
    }
    v
}

/// Draw `layer` (straight alpha, `lw` x `lh`) over `canvas` (`cw` x `ch`) with its top-left
/// corner at `(x, y)`.
/// Parts that fall outside the canvas are cut off.
pub fn blend_over(
    canvas: &mut [u8],
    (cw, ch): (usize, usize),
    layer: &[u8],
    (lw, lh): (usize, usize),
    (x, y): (i64, i64),
) {
    debug_assert_eq!(canvas.len(), cw * ch * 4);
    debug_assert_eq!(layer.len(), lw * lh * 4);
    // Columns of the layer that land on the canvas.
    let x0 = (-x).max(0) as usize;
    let x1 = (cw as i64 - x).clamp(0, lw as i64) as usize;
    if x0 >= x1 {
        return;
    }
    par_rows(canvas, cw * 4, |cy, row| {
        let ly = cy as i64 - y;
        if ly < 0 || ly >= lh as i64 {
            return;
        }
        let src = &layer[ly as usize * lw * 4..(ly as usize + 1) * lw * 4];
        for lx in x0..x1 {
            let cx = (lx as i64 + x) as usize;
            let s = &src[lx * 4..lx * 4 + 4];
            let d = &mut row[cx * 4..cx * 4 + 4];
            blend_pixel(d, s);
        }
    });
}

#[inline]
fn blend_pixel(d: &mut [u8], s: &[u8]) {
    let sa = u32::from(s[3]);
    if sa == 0 {
        return;
    }
    if sa == 255 {
        d.copy_from_slice(s);
        return;
    }
    let da = u32::from(d[3]);
    if da == 255 {
        for c in 0..3 {
            d[c] = ((u32::from(s[c]) * sa + u32::from(d[c]) * (255 - sa) + 127) / 255) as u8;
        }
        return;
    }
    // General case: both partly transparent.
    let dst_weight = (da * (255 - sa) + 127) / 255;
    let oa = sa + dst_weight;
    if oa == 0 {
        return;
    }
    for c in 0..3 {
        d[c] = ((u32::from(s[c]) * sa + u32::from(d[c]) * dst_weight + oa / 2) / oa) as u8;
    }
    d[3] = oa as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contain_letterboxes_and_cover_crops() {
        // 4:3 picture on a 16:9 canvas.
        let p = placement(400, 300, 1920, 1080, Fit::Contain);
        assert_eq!((p.width, p.height, p.x, p.y), (1440, 1080, 240, 0));
        let p = placement(400, 300, 1920, 1080, Fit::Cover);
        assert_eq!((p.width, p.height, p.x), (1920, 1440, 0));
        assert_eq!(p.y, -180);
        let p = placement(400, 300, 1920, 1080, Fit::Stretch);
        assert_eq!((p.width, p.height, p.x, p.y), (1920, 1080, 0, 0));
    }

    #[test]
    fn same_shape_fills_exactly() {
        for fit in [Fit::Contain, Fit::Cover, Fit::Stretch] {
            let p = placement(1280, 720, 1920, 1080, fit);
            assert_eq!((p.width, p.height, p.x, p.y), (1920, 1080, 0, 0), "{fit:?}");
        }
    }

    #[test]
    fn opaque_replaces_transparent_skips_half_mixes() {
        let mut c = solid(1, 1, [100, 100, 100, 255]);
        blend_over(&mut c, (1, 1), &[0, 0, 0, 0], (1, 1), (0, 0));
        assert_eq!(c, [100, 100, 100, 255]);
        blend_over(&mut c, (1, 1), &[200, 0, 0, 255], (1, 1), (0, 0));
        assert_eq!(c, [200, 0, 0, 255]);
        let mut c = solid(1, 1, [0, 0, 0, 255]);
        blend_over(&mut c, (1, 1), &[255, 255, 255, 128], (1, 1), (0, 0));
        assert!((i32::from(c[0]) - 128).abs() <= 1 && c[3] == 255);
    }

    #[test]
    fn layers_are_cut_at_the_canvas_edge() {
        let mut c = solid(4, 2, [0, 0, 0, 255]);
        let layer = solid(3, 3, [9, 9, 9, 255]);
        blend_over(&mut c, (4, 2), &layer, (3, 3), (2, -1));
        // Only columns 2 and 3 of both rows are painted.
        for y in 0..2 {
            for x in 0..4 {
                let px = &c[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
                let want = if x >= 2 {
                    [9, 9, 9, 255]
                } else {
                    [0, 0, 0, 255]
                };
                assert_eq!(px, want, "({x},{y})");
            }
        }
        // Entirely off canvas does nothing.
        let before = c.clone();
        blend_over(&mut c, (4, 2), &layer, (3, 3), (10, 0));
        blend_over(&mut c, (4, 2), &layer, (3, 3), (-5, 0));
        assert_eq!(c, before);
    }

    #[test]
    fn two_translucent_layers_on_nothing() {
        let mut c = solid(1, 1, [0, 0, 0, 0]);
        blend_over(&mut c, (1, 1), &[255, 0, 0, 128], (1, 1), (0, 0));
        assert_eq!(c, [255, 0, 0, 128]);
    }
}
