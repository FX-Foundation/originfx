// SPDX-License-Identifier: GPL-3.0-or-later
//! Crop, mask and transform.

/// Move, scale and rotate a layer about the middle of the frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    /// Move right, percent of the frame width.
    pub x_pct: f64,
    /// Move down, percent of the frame height.
    pub y_pct: f64,
    /// Horizontal size, percent (100 is as it is).
    pub scale_x_pct: f64,
    /// Vertical size, percent.
    pub scale_y_pct: f64,
    /// Clockwise turn in degrees.
    pub rotation_deg: f64,
}

impl Default for Affine {
    fn default() -> Self {
        Affine {
            x_pct: 0.0,
            y_pct: 0.0,
            scale_x_pct: 100.0,
            scale_y_pct: 100.0,
            rotation_deg: 0.0,
        }
    }
}

impl Affine {
    /// True when the transform changes nothing (and so is not run).
    pub fn is_identity(&self) -> bool {
        *self == Affine::default()
    }
}

/// Premultiplied sample of the picture at a fractional position, bilinear, transparent outside.
fn sample(src: &[u8], w: usize, h: usize, x: f64, y: f64) -> [f32; 4] {
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = ((fx - x0) as f32, (fy - y0) as f32);
    let mut out = [0f32; 4];
    for (dy, wy) in [(0i64, 1.0 - ty), (1, ty)] {
        for (dx, wx) in [(0i64, 1.0 - tx), (1, tx)] {
            let (px, py) = (x0 as i64 + dx, y0 as i64 + dy);
            if px < 0 || py < 0 || px >= w as i64 || py >= h as i64 {
                continue;
            }
            let i = (py as usize * w + px as usize) * 4;
            let a = f32::from(src[i + 3]) / 255.0;
            let k = wx * wy;
            out[0] += f32::from(src[i]) * a * k;
            out[1] += f32::from(src[i + 1]) * a * k;
            out[2] += f32::from(src[i + 2]) * a * k;
            out[3] += a * k;
        }
    }
    out
}

/// Move, scale and rotate a `w` x `h` picture about its middle. Parts that move outside the frame
/// are cut off; newly uncovered parts are transparent. Colour and transparency are mixed in
/// premultiplied form, so edges stay clean.
pub fn transform(src: &[u8], w: usize, h: usize, t: &Affine) -> Vec<u8> {
    debug_assert_eq!(src.len(), w * h * 4);
    if t.is_identity() {
        return src.to_vec();
    }
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let (tx, ty) = (t.x_pct / 100.0 * w as f64, t.y_pct / 100.0 * h as f64);
    let (sx, sy) = (t.scale_x_pct / 100.0, t.scale_y_pct / 100.0);
    let mut out = vec![0u8; src.len()];
    if sx <= 0.0 || sy <= 0.0 {
        return out;
    }
    let (sin, cos) = t.rotation_deg.to_radians().sin_cos();
    for y in 0..h {
        for x in 0..w {
            // From the output pixel back to the picture: undo the move, the turn, then the scale.
            let (dx, dy) = (x as f64 + 0.5 - cx - tx, y as f64 + 0.5 - cy - ty);
            let (rx, ry) = (dx * cos + dy * sin, -dx * sin + dy * cos);
            let (u, v) = (rx / sx + cx, ry / sy + cy);
            let p = sample(src, w, h, u, v);
            let a = p[3];
            if a <= 0.0 {
                continue;
            }
            let i = (y * w + x) * 4;
            for c in 0..3 {
                out[i + c] = (p[c] / a + 0.5).clamp(0.0, 255.0) as u8;
            }
            out[i + 3] = (a * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Cut away the edges of a layer: each side is a percent of the picture's width or height.
/// The cut-away area becomes transparent.
pub fn crop(rgba: &mut [u8], w: usize, h: usize, left: f64, right: f64, top: f64, bottom: f64) {
    if left <= 0.0 && right <= 0.0 && top <= 0.0 && bottom <= 0.0 {
        return;
    }
    let x0 = (left.max(0.0) / 100.0 * w as f64).round() as usize;
    let x1 = w.saturating_sub((right.max(0.0) / 100.0 * w as f64).round() as usize);
    let y0 = (top.max(0.0) / 100.0 * h as f64).round() as usize;
    let y1 = h.saturating_sub((bottom.max(0.0) / 100.0 * h as f64).round() as usize);
    for y in 0..h {
        for x in 0..w {
            if x < x0 || x >= x1 || y < y0 || y >= y1 {
                let i = (y * w + x) * 4;
                rgba[i..i + 4].copy_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
}

/// The shape of a mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MaskShape {
    Rect,
    Ellipse,
}

impl MaskShape {
    pub const ALL: [MaskShape; 2] = [MaskShape::Rect, MaskShape::Ellipse];

    pub fn name(self) -> &'static str {
        match self {
            MaskShape::Rect => "rect",
            MaskShape::Ellipse => "ellipse",
        }
    }

    pub fn parse(name: &str) -> Option<MaskShape> {
        MaskShape::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// A mask: where the layer shows. Positions and sizes are percent of the frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaskSpec {
    pub shape: MaskShape,
    /// Show the outside of the shape instead of the inside.
    pub invert: bool,
    /// Centre of the shape, percent of the width and of the height.
    pub x_pct: f64,
    pub y_pct: f64,
    /// Full width and height of the shape, percent of the frame.
    pub width_pct: f64,
    pub height_pct: f64,
    /// Width of the soft edge, percent of the frame height, centred on the edge.
    pub feather_pct: f64,
}

/// How much of a pixel the shape covers: 1 inside, 0 outside, smooth across the feather.
fn coverage(m: &MaskSpec, px: f64, py: f64, w: f64, h: f64) -> f64 {
    let (cx, cy) = (m.x_pct / 100.0 * w, m.y_pct / 100.0 * h);
    let (hw, hh) = (m.width_pct / 200.0 * w, m.height_pct / 200.0 * h);
    if hw <= 0.0 || hh <= 0.0 {
        return 0.0;
    }
    // Signed distance in pixels to the edge, negative inside.
    let dist = match m.shape {
        MaskShape::Rect => {
            let (dx, dy) = ((px - cx).abs() - hw, (py - cy).abs() - hh);
            dx.max(dy)
        }
        MaskShape::Ellipse => {
            // Distance along the ray to the centre, scaled so the radii are the half sizes.
            let (nx, ny) = ((px - cx) / hw, (py - cy) / hh);
            let r = (nx * nx + ny * ny).sqrt();
            (r - 1.0) * hw.min(hh)
        }
    };
    let feather = (m.feather_pct / 100.0 * h).max(1.0);
    let t = (0.5 - dist / feather).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Multiply a layer's transparency by a mask.
pub fn mask(rgba: &mut [u8], w: usize, h: usize, m: &MaskSpec) {
    for y in 0..h {
        for x in 0..w {
            let mut c = coverage(m, x as f64 + 0.5, y as f64 + 0.5, w as f64, h as f64);
            if m.invert {
                c = 1.0 - c;
            }
            if c >= 1.0 {
                continue;
            }
            let i = (y * w + x) * 4 + 3;
            rgba[i] = (f64::from(rgba[i]) * c + 0.5) as u8;
        }
    }
}
