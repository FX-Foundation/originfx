// SPDX-License-Identifier: GPL-3.0-or-later
//! # OriginFX Motion Graphics Engine
//!
//! Layers that are drawn rather than filmed: a solid colour, a gradient, a shape or some text.
//! [`render`] draws one [`Generator`] onto a transparent straight-alpha RGBA8 picture of the
//! frame's size; the Compositing Engine then moves, fades and blends it like any other clip.
//!
//! * Sizes are percent of the frame (width for widths, height for heights and text size), so a
//!   graphic looks the same at preview and export size.
//! * Shapes are drawn from signed distance functions, so edges are smoothly anti-aliased.
//! * Text is rasterised with `fontdue` from a font file (see [`Fonts`]).
//! * The same input always gives the same output.

mod shape;
mod text;

#[cfg(test)]
mod tests;

pub use text::{Align, Fonts};

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "String", into = "String"))]
/// A straight (not premultiplied) colour, 8 bits a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Colour {
    pub const WHITE: Colour = Colour::rgb(255, 255, 255);
    pub const BLACK: Colour = Colour::rgb(0, 0, 0);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Colour {
        Colour { r, g, b, a: 255 }
    }

    /// `#rgb`, `#rrggbb` or `#rrggbbaa` (the `#` is optional), or a basic colour name.
    pub fn parse(text: &str) -> Option<Colour> {
        let t = text.trim().to_ascii_lowercase();
        let named = match t.as_str() {
            "white" => Some(Colour::WHITE),
            "black" => Some(Colour::BLACK),
            "red" => Some(Colour::rgb(255, 0, 0)),
            "green" => Some(Colour::rgb(0, 200, 0)),
            "blue" => Some(Colour::rgb(0, 0, 255)),
            "yellow" => Some(Colour::rgb(255, 255, 0)),
            "cyan" => Some(Colour::rgb(0, 255, 255)),
            "magenta" => Some(Colour::rgb(255, 0, 255)),
            "orange" => Some(Colour::rgb(255, 140, 0)),
            "grey" | "gray" => Some(Colour::rgb(128, 128, 128)),
            "none" | "transparent" => Some(Colour {
                r: 0,
                g: 0,
                b: 0,
                a: 0,
            }),
            _ => None,
        };
        if named.is_some() {
            return named;
        }
        let h = t.trim_start_matches('#');
        if !h.is_ascii() {
            return None;
        }
        let byte = |s: &str| u8::from_str_radix(s, 16).ok();
        match h.len() {
            3 => {
                let d = |i: usize| byte(&h[i..=i]).map(|v| v * 17);
                Some(Colour::rgb(d(0)?, d(1)?, d(2)?))
            }
            6 | 8 => {
                let a = if h.len() == 8 { byte(&h[6..8])? } else { 255 };
                Some(Colour {
                    r: byte(&h[0..2])?,
                    g: byte(&h[2..4])?,
                    b: byte(&h[4..6])?,
                    a,
                })
            }
            _ => None,
        }
    }

    /// `#rrggbb`, or `#rrggbbaa` when not fully solid.
    pub fn hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }
}

impl From<Colour> for String {
    fn from(c: Colour) -> String {
        c.hex()
    }
}

impl TryFrom<String> for Colour {
    type Error = String;
    fn try_from(s: String) -> std::result::Result<Colour, String> {
        Colour::parse(&s).ok_or_else(|| format!("'{s}' is not a colour"))
    }
}

/// How a shape or the whole frame is filled.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fill {
    Solid(Colour),
    /// A straight blend from `from` to `to`. `angle_deg` 0 runs left to right, 90 top to bottom.
    Linear {
        from: Colour,
        to: Colour,
        angle_deg: f64,
    },
}

/// The outline of a shape, drawn centred on its edge.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stroke {
    pub colour: Colour,
    /// Percent of the frame height.
    pub width_pct: f64,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type", rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShapeKind {
    /// Corners rounded by `corner_pct`, percent of the shorter side (0 square, 100 a pill).
    Rect {
        corner_pct: f64,
    },
    Ellipse,
    /// A regular polygon with `sides` (3 to 32) corners, fitted in the box.
    Polygon {
        sides: u32,
    },
    /// A star with `points` (3 to 32) tips; `inner_pct` is the inner radius as a percent of the outer.
    Star {
        points: u32,
        inner_pct: f64,
    },
}

/// A shape in the middle of the frame. Move, size and turn it afterwards with the clip's own
/// position, scale and rotation.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub kind: ShapeKind,
    /// Percent of the frame width and height.
    pub width_pct: f64,
    pub height_pct: f64,
    pub fill: Fill,
    pub stroke: Option<Stroke>,
}

/// Text in the middle of the frame (lines are split at `\n`).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub text: String,
    /// A font file (.ttf, .otf). `None` uses the first usual system font found.
    pub font: Option<String>,
    /// Percent of the frame height (the em size).
    pub size_pct: f64,
    pub colour: Colour,
    pub align: Align,
    /// Extra space between lines, percent of the size (0 is the font's own).
    pub line_gap_pct: f64,
    /// Extra space between letters, percent of the size.
    pub tracking_pct: f64,
    pub outline: Option<Stroke>,
    pub shadow: Option<Shadow>,
}

/// A hard drop shadow.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    pub colour: Colour,
    /// Percent of the frame height, right and down.
    pub dx_pct: f64,
    pub dy_pct: f64,
}

/// Something drawn rather than filmed.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "graphic", rename_all = "snake_case"))]
#[derive(Debug, Clone, PartialEq)]
pub enum Generator {
    /// Fills the whole frame.
    Fill(Fill),
    Shape(Shape),
    Text(Text),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The font file could not be read.
    FontNotFound(String),
    /// The file is not a font this engine can read.
    FontInvalid(String),
    /// No font was named and no usual system font was found.
    NoFont,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::FontNotFound(p) => write!(f, "cannot read the font file {p}"),
            Error::FontInvalid(p) => write!(f, "{p} is not a usable font"),
            Error::NoFont => write!(
                f,
                "no font was given and none was found on this machine; name a font file"
            ),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Draw `generator` on a transparent `w` by `h` picture.
pub fn render(generator: &Generator, w: usize, h: usize, fonts: &mut Fonts) -> Result<Vec<u8>> {
    let mut out = vec![0u8; w * h * 4];
    match generator {
        Generator::Fill(f) => shape::fill_all(&mut out, w, h, f),
        Generator::Shape(s) => shape::draw(&mut out, w, h, s),
        Generator::Text(t) => text::draw(&mut out, w, h, t, fonts)?,
    }
    Ok(out)
}

/// Lay a straight-alpha colour with `coverage` (0 to 1) over a straight-alpha pixel.
pub(crate) fn over(px: &mut [u8], c: Colour, coverage: f32) {
    let sa = f32::from(c.a) / 255.0 * coverage.clamp(0.0, 1.0);
    if sa <= 0.0 {
        return;
    }
    let da = f32::from(px[3]) / 255.0;
    let oa = sa + da * (1.0 - sa);
    let mix = |s: u8, d: u8| -> u8 {
        let v = (f32::from(s) * sa + f32::from(d) * da * (1.0 - sa)) / oa;
        v.round().clamp(0.0, 255.0) as u8
    };
    px[0] = mix(c.r, px[0]);
    px[1] = mix(c.g, px[1]);
    px[2] = mix(c.b, px[2]);
    px[3] = (oa * 255.0).round().clamp(0.0, 255.0) as u8;
}
