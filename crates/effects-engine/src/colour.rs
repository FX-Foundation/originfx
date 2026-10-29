// SPDX-License-Identifier: GPL-3.0-or-later
//! Colour effects. Alpha is never touched.

fn clamp8(v: f32) -> u8 {
    (v + 0.5).clamp(0.0, 255.0) as u8
}

/// Apply a 256-entry table to the colour channels.
fn table(rgba: &mut [u8], t: &[u8; 256]) {
    for px in rgba.chunks_exact_mut(4) {
        px[0] = t[px[0] as usize];
        px[1] = t[px[1] as usize];
        px[2] = t[px[2] as usize];
    }
}

fn build(f: impl Fn(f32) -> f32) -> [u8; 256] {
    let mut t = [0u8; 256];
    for (i, o) in t.iter_mut().enumerate() {
        *o = clamp8(f(i as f32 / 255.0) * 255.0);
    }
    t
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

pub fn brightness(rgba: &mut [u8], percent: f64) {
    let add = percent as f32 / 100.0;
    table(rgba, &build(|c| c + add));
}

pub fn contrast(rgba: &mut [u8], percent: f64) {
    // 0 is neutral; -100 flattens to grey; +100 triples the distance from mid grey.
    let f = if percent >= 0.0 {
        1.0 + percent as f32 / 50.0
    } else {
        1.0 + percent as f32 / 100.0
    };
    table(rgba, &build(|c| (c - 0.5) * f + 0.5));
}

pub fn exposure(rgba: &mut [u8], stops: f64) {
    let gain = 2f32.powf(stops as f32);
    table(
        rgba,
        &build(|c| linear_to_srgb((srgb_to_linear(c) * gain).min(1.0))),
    );
}

pub fn gamma(rgba: &mut [u8], g: f64) {
    let inv = 1.0 / g as f32;
    table(rgba, &build(|c| c.powf(inv)));
}

/// Move every colour towards `target` by `amount` (0 to 1) with a 3x3 matrix applied to RGB.
fn matrix(rgba: &mut [u8], m: [[f32; 3]; 3]) {
    for px in rgba.chunks_exact_mut(4) {
        let (r, g, b) = (f32::from(px[0]), f32::from(px[1]), f32::from(px[2]));
        for (c, row) in m.iter().enumerate() {
            px[c] = clamp8(row[0] * r + row[1] * g + row[2] * b);
        }
    }
}

const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// A matrix that blends the picture with `towards` (rows) by `amount`.
fn mixed(towards: [[f32; 3]; 3], amount: f32) -> [[f32; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            let id = if i == j { 1.0 } else { 0.0 };
            *v = id + (towards[i][j] - id) * amount;
        }
    }
    m
}

pub fn saturation(rgba: &mut [u8], s: f64) {
    let s = s as f32;
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            let id = if i == j { 1.0 } else { 0.0 };
            *v = LUMA[j] * (1.0 - s) + id * s;
        }
    }
    matrix(rgba, m);
}

pub fn grayscale(rgba: &mut [u8], amount: f64) {
    matrix(rgba, mixed([LUMA; 3], amount as f32));
}

pub fn sepia(rgba: &mut [u8], amount: f64) {
    let sepia = [
        [0.393, 0.769, 0.189],
        [0.349, 0.686, 0.168],
        [0.272, 0.534, 0.131],
    ];
    matrix(rgba, mixed(sepia, amount as f32));
}

pub fn invert(rgba: &mut [u8], amount: f64) {
    let a = amount as f32;
    table(rgba, &build(|c| c + (1.0 - 2.0 * c) * a));
}

pub fn hue(rgba: &mut [u8], degrees: f64) {
    let (s, c) = (
        degrees.to_radians().sin() as f32,
        degrees.to_radians().cos() as f32,
    );
    // The standard hue rotation about the grey axis.
    let m = [
        [
            0.213 + c * 0.787 - s * 0.213,
            0.715 - c * 0.715 - s * 0.715,
            0.072 - c * 0.072 + s * 0.928,
        ],
        [
            0.213 - c * 0.213 + s * 0.143,
            0.715 + c * 0.285 + s * 0.140,
            0.072 - c * 0.072 - s * 0.283,
        ],
        [
            0.213 - c * 0.213 - s * 0.787,
            0.715 - c * 0.715 + s * 0.715,
            0.072 + c * 0.928 + s * 0.072,
        ],
    ];
    matrix(rgba, m);
}

pub fn temperature(rgba: &mut [u8], percent: f64) {
    // Positive is warmer: more red, less blue.
    let k = percent as f32 / 100.0 * 0.25;
    let (r, b) = (1.0 + k, 1.0 - k);
    for px in rgba.chunks_exact_mut(4) {
        px[0] = clamp8(f32::from(px[0]) * r);
        px[2] = clamp8(f32::from(px[2]) * b);
    }
}

pub fn vignette(rgba: &mut [u8], w: usize, h: usize, amount: f64, softness: f64) {
    // Distance from the centre, 0 in the middle and 1 at the corners.
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let corner = (cx * cx + cy * cy).sqrt();
    // Darkening starts at `inner` and is complete at the corners; softer means it starts sooner.
    let inner = 0.9 - 0.9 * softness as f32;
    let amount = amount as f32;
    for y in 0..h {
        let dy = y as f32 + 0.5 - cy;
        for x in 0..w {
            let dx = x as f32 + 0.5 - cx;
            let d = (dx * dx + dy * dy).sqrt() / corner;
            let t = ((d - inner) / (1.0 - inner)).clamp(0.0, 1.0);
            let shade = t * t * (3.0 - 2.0 * t);
            let keep = 1.0 - amount * shade;
            if keep < 1.0 {
                let i = (y * w + x) * 4;
                for c in 0..3 {
                    rgba[i + c] = clamp8(f32::from(rgba[i + c]) * keep);
                }
            }
        }
    }
}
