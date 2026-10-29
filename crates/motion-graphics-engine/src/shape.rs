// SPDX-License-Identifier: GPL-3.0-or-later
//! Fills and shapes, from signed distance functions (negative inside, in pixels).

use crate::{over, Colour, Fill, Shape, ShapeKind};

fn mix(a: Colour, b: Colour, t: f32) -> Colour {
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Colour {
        r: l(a.r, b.r),
        g: l(a.g, b.g),
        b: l(a.b, b.b),
        a: l(a.a, b.a),
    }
}

/// The fill colour at pixel (x, y) of a box of `bw` by `bh` centred on (cx, cy).
fn fill_at(f: &Fill, x: f32, y: f32, cx: f32, cy: f32, bw: f32, bh: f32) -> Colour {
    match *f {
        Fill::Solid(c) => c,
        Fill::Linear {
            from,
            to,
            angle_deg,
        } => {
            let a = (angle_deg as f32).to_radians();
            let (dx, dy) = (a.cos(), a.sin());
            let len = (bw * dx.abs() + bh * dy.abs()).max(1e-3);
            let t = (0.5 + ((x - cx) * dx + (y - cy) * dy) / len).clamp(0.0, 1.0);
            mix(from, to, t)
        }
    }
}

pub(crate) fn fill_all(out: &mut [u8], w: usize, h: usize, f: &Fill) {
    let (fw, fh) = (w as f32, h as f32);
    for y in 0..h {
        for x in 0..w {
            let c = fill_at(
                f,
                x as f32 + 0.5,
                y as f32 + 0.5,
                fw / 2.0,
                fh / 2.0,
                fw,
                fh,
            );
            let i = (y * w + x) * 4;
            over(&mut out[i..i + 4], c, 1.0);
        }
    }
}

fn len(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

/// Rounded box with half sizes (bx, by) and corner radius r.
fn sd_box(px: f32, py: f32, bx: f32, by: f32, r: f32) -> f32 {
    let (qx, qy) = (px.abs() - bx + r, py.abs() - by + r);
    len(qx.max(0.0), qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Ellipse with radii (rx, ry): a close approximation of the true distance.
fn sd_ellipse(px: f32, py: f32, rx: f32, ry: f32) -> f32 {
    let k0 = len(px / rx, py / ry);
    let k1 = len(px / (rx * rx), py / (ry * ry));
    if k1 == 0.0 {
        return -rx.min(ry);
    }
    k0 * (k0 - 1.0) / k1
}

/// Regular polygon of `n` sides, circumradius r, one corner pointing up (screen y points down).
fn sd_polygon(px: f32, py: f32, r: f32, n: u32) -> f32 {
    use std::f32::consts::PI;
    let sector = 2.0 * PI / n as f32;
    // Angle clockwise from straight up, then the edge facing that direction.
    let ang = px.atan2(-py).rem_euclid(2.0 * PI);
    let mid = (ang / sector).floor() * sector + sector / 2.0;
    let apothem = r * (sector / 2.0).cos();
    px * mid.sin() - py * mid.cos() - apothem
}

/// Star with `n` tips, outer radius r, inner radius r*m, one tip up.
fn sd_star(px: f32, py: f32, r: f32, n: u32, m: f32) -> f32 {
    use std::f32::consts::PI;
    let an = PI / n as f32;
    // Fold into one half-tip wedge using the star's symmetry (y up in the folded frame).
    let ang = px.atan2(-py).abs();
    let w = ang.rem_euclid(2.0 * an);
    let w = if w > an { 2.0 * an - w } else { w };
    let l = len(px, py);
    let (qx, qy) = (l * w.sin(), l * w.cos());
    // The edge from the tip (0, r) to the inner corner.
    let ri = r * m;
    let (ex, ey) = (ri * an.sin(), ri * an.cos() - r);
    let (wx, wy) = (qx, qy - r);
    let t = ((wx * ex + wy * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
    let d = len(wx - ex * t, wy - ey * t);
    if ex * wy - ey * wx < 0.0 {
        -d
    } else {
        d
    }
}

pub(crate) fn draw(out: &mut [u8], w: usize, h: usize, s: &Shape) {
    let (fw, fh) = (w as f32, h as f32);
    let (bw, bh) = (
        (s.width_pct as f32 / 100.0 * fw).max(0.0),
        (s.height_pct as f32 / 100.0 * fh).max(0.0),
    );
    if bw <= 0.0 || bh <= 0.0 {
        return;
    }
    let (cx, cy) = (fw / 2.0, fh / 2.0);
    let (hx, hy) = (bw / 2.0, bh / 2.0);
    let stroke_w = s
        .stroke
        .map_or(0.0, |k| (k.width_pct as f32 / 100.0 * fh).max(0.0));
    let dist = |px: f32, py: f32| -> f32 {
        match s.kind {
            ShapeKind::Rect { corner_pct } => {
                let r = (corner_pct as f32 / 100.0).clamp(0.0, 1.0) * hx.min(hy);
                sd_box(px, py, hx, hy, r)
            }
            ShapeKind::Ellipse => sd_ellipse(px, py, hx, hy),
            ShapeKind::Polygon { sides } => {
                // Fit the unit polygon to the box by scaling each axis.
                let n = sides.clamp(3, 32);
                let (ux, uy) = (px / hx, py / hy);
                sd_polygon(ux, uy, 1.0, n) * hx.min(hy)
            }
            ShapeKind::Star { points, inner_pct } => {
                let n = points.clamp(3, 32);
                let m = (inner_pct as f32 / 100.0).clamp(0.05, 1.0);
                let (ux, uy) = (px / hx, py / hy);
                sd_star(ux, uy, 1.0, n, m) * hx.min(hy)
            }
        }
    };
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let d = dist(fx - cx, fy - cy);
            let i = (y * w + x) * 4;
            let cover = (0.5 - d).clamp(0.0, 1.0);
            if cover > 0.0 {
                let c = fill_at(&s.fill, fx, fy, cx, cy, bw, bh);
                over(&mut out[i..i + 4], c, cover);
            }
            if let Some(k) = s.stroke {
                if stroke_w > 0.0 {
                    let sc = (0.5 - (d.abs() - stroke_w / 2.0)).clamp(0.0, 1.0);
                    over(&mut out[i..i + 4], k.colour, sc);
                }
            }
        }
    }
}
