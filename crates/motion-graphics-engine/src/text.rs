// SPDX-License-Identifier: GPL-3.0-or-later
//! Text: glyphs from a font file, laid out in lines about the middle of the frame.

use std::collections::HashMap;
use std::fs;

use fontdue::{Font, FontSettings};

use crate::{over, Colour, Error, Result, Text};

/// Where the lines of a block of text sit against each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Align {
    Left,
    Centre,
    Right,
}

impl Align {
    pub fn name(self) -> &'static str {
        match self {
            Align::Left => "left",
            Align::Centre => "centre",
            Align::Right => "right",
        }
    }

    pub fn parse(s: &str) -> Option<Align> {
        match s {
            "left" => Some(Align::Left),
            "centre" | "center" | "middle" => Some(Align::Centre),
            "right" => Some(Align::Right),
            _ => None,
        }
    }
}

/// Fonts read so far, so a font file is read once however many frames use it.
#[derive(Default)]
pub struct Fonts {
    loaded: HashMap<String, Font>,
}

/// Where to look for a font when none is named, in order.
const SYSTEM_FONTS: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
    "/usr/share/fonts/opentype/inter/Inter-Regular.otf",
    "/Library/Fonts/Arial.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
    "C:\\Windows\\Fonts\\arial.ttf",
    "C:\\Windows\\Fonts\\segoeui.ttf",
];

impl Fonts {
    pub fn new() -> Fonts {
        Fonts::default()
    }

    /// The path used when a font is not named: the first usual system font that exists.
    pub fn default_path() -> Option<&'static str> {
        SYSTEM_FONTS
            .iter()
            .copied()
            .find(|p| std::path::Path::new(p).is_file())
    }

    fn get(&mut self, path: Option<&str>) -> Result<&Font> {
        let key = match path {
            Some(p) => p.to_string(),
            None => Fonts::default_path().ok_or(Error::NoFont)?.to_string(),
        };
        if !self.loaded.contains_key(&key) {
            let bytes = fs::read(&key).map_err(|_| Error::FontNotFound(key.clone()))?;
            let font = Font::from_bytes(bytes, FontSettings::default())
                .map_err(|_| Error::FontInvalid(key.clone()))?;
            self.loaded.insert(key.clone(), font);
        }
        Ok(&self.loaded[&key])
    }
}

struct Line {
    glyphs: Vec<(char, f32)>, // character and its pen x from the line start
    width: f32,
}

fn layout(font: &Font, px: f32, tracking: f32, line: &str) -> Line {
    let mut glyphs = Vec::new();
    let mut pen = 0.0f32;
    let mut prev: Option<char> = None;
    for ch in line.chars() {
        if let Some(p) = prev {
            pen += font.horizontal_kern(p, ch, px).unwrap_or(0.0);
        }
        glyphs.push((ch, pen));
        pen += font.metrics(ch, px).advance_width + tracking;
        prev = Some(ch);
    }
    Line {
        glyphs,
        width: (pen - if prev.is_some() { tracking } else { 0.0 }).max(0.0),
    }
}

/// Coverage (0 to 1) of the whole block, one value per pixel.
fn coverage(font: &Font, w: usize, h: usize, t: &Text) -> Vec<f32> {
    let px = (t.size_pct as f32 / 100.0 * h as f32).max(1.0);
    let tracking = t.tracking_pct as f32 / 100.0 * px;
    let lines: Vec<Line> = t
        .text
        .split('\n')
        .map(|l| layout(font, px, tracking, l))
        .collect();
    let (ascent, line_h) = font
        .horizontal_line_metrics(px)
        .map_or((px * 0.8, px * 1.2), |m| (m.ascent, m.new_line_size));
    let line_h = line_h + t.line_gap_pct as f32 / 100.0 * px;
    let block_w = lines.iter().map(|l| l.width).fold(0.0f32, f32::max);
    let block_h = line_h * lines.len() as f32;
    let (left, top) = ((w as f32 - block_w) / 2.0, (h as f32 - block_h) / 2.0);
    let mut cov = vec![0.0f32; w * h];
    for (n, line) in lines.iter().enumerate() {
        let x0 = left
            + match t.align {
                Align::Left => 0.0,
                Align::Centre => (block_w - line.width) / 2.0,
                Align::Right => block_w - line.width,
            };
        let baseline = (top + line_h * n as f32 + ascent).round();
        for &(ch, pen) in &line.glyphs {
            if ch.is_whitespace() {
                continue;
            }
            let (m, bitmap) = font.rasterize(ch, px);
            let gx = (x0 + pen).round() as i64 + i64::from(m.xmin);
            let gy = baseline as i64 - i64::from(m.ymin) - m.height as i64;
            for by in 0..m.height {
                let y = gy + by as i64;
                if y < 0 || y >= h as i64 {
                    continue;
                }
                for bx in 0..m.width {
                    let x = gx + bx as i64;
                    if x < 0 || x >= w as i64 {
                        continue;
                    }
                    let v = f32::from(bitmap[by * m.width + bx]) / 255.0;
                    let c = &mut cov[y as usize * w + x as usize];
                    *c = (*c + v).min(1.0);
                }
            }
        }
    }
    cov
}

/// Grow coverage outwards by `r` pixels (a round brush).
fn dilate(cov: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
    if r <= 0.0 {
        return cov.to_vec();
    }
    let mut offsets = vec![(0i64, 0i64)];
    let rings = (r.ceil() as usize).clamp(1, 3);
    for k in 1..=rings {
        let rr = r * k as f32 / rings as f32;
        let steps = 16 + 8 * k;
        for s in 0..steps {
            let a = std::f32::consts::TAU * s as f32 / steps as f32;
            offsets.push(((rr * a.cos()).round() as i64, (rr * a.sin()).round() as i64));
        }
    }
    offsets.sort_unstable();
    offsets.dedup();
    let mut out = vec![0.0f32; w * h];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let mut best = 0.0f32;
            for &(dx, dy) in &offsets {
                let (sx, sy) = (x + dx, y + dy);
                if sx >= 0 && sy >= 0 && sx < w as i64 && sy < h as i64 {
                    best = best.max(cov[sy as usize * w + sx as usize]);
                }
            }
            out[y as usize * w + x as usize] = best;
        }
    }
    out
}

fn paint(out: &mut [u8], cov: &[f32], colour: Colour, shift: (i64, i64), w: usize, h: usize) {
    for y in 0..h as i64 {
        let sy = y - shift.1;
        if sy < 0 || sy >= h as i64 {
            continue;
        }
        for x in 0..w as i64 {
            let sx = x - shift.0;
            if sx < 0 || sx >= w as i64 {
                continue;
            }
            let c = cov[sy as usize * w + sx as usize];
            if c > 0.0 {
                let i = (y as usize * w + x as usize) * 4;
                over(&mut out[i..i + 4], colour, c);
            }
        }
    }
}

pub(crate) fn draw(out: &mut [u8], w: usize, h: usize, t: &Text, fonts: &mut Fonts) -> Result<()> {
    let font = fonts.get(t.font.as_deref())?;
    if t.text.trim().is_empty() {
        return Ok(());
    }
    let fill = coverage(font, w, h, t);
    let outline = t.outline.map(|o| {
        (
            dilate(&fill, w, h, o.width_pct as f32 / 100.0 * h as f32 / 2.0),
            o.colour,
        )
    });
    if let Some(s) = t.shadow {
        let shift = (
            (s.dx_pct as f32 / 100.0 * h as f32).round() as i64,
            (s.dy_pct as f32 / 100.0 * h as f32).round() as i64,
        );
        let base = outline.as_ref().map_or(&fill, |(o, _)| o);
        paint(out, base, s.colour, shift, w, h);
    }
    if let Some((o, colour)) = &outline {
        paint(out, o, *colour, (0, 0), w, h);
    }
    paint(out, &fill, t.colour, (0, 0), w, h);
    Ok(())
}
