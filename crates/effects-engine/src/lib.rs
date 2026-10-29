// SPDX-License-Identifier: GPL-3.0-or-later
//! # OriginFX Effects Engine
//!
//! Picture effects on RGBA pixels: colour (brightness, contrast, exposure, gamma, saturation,
//! hue, temperature, invert, grayscale, sepia), blur, sharpen, vignette and pixelate.
//!
//! The engine is pure CPU code with no dependencies. Every effect is a [`Kind`] with named
//! settings (see [`Kind::params`]: name, unit, default and range) and one function,
//! [`apply`], which changes a straight-alpha RGBA8 picture in place. Values are plain `f64`s in
//! the order of `Kind::params()`, so a caller can keep them anywhere and animate them freely.
//!
//! * A setting at its neutral value changes **no bits** ([`is_neutral`]).
//! * Alpha is never changed by a colour effect; blur, sharpen and pixelate treat colour and
//!   alpha together so transparent edges do not get dark or bright halos.
//! * Sizes are given as a **percent of the picture's height**, so the same effect looks the same
//!   at preview size and at full size.
//! * The same input always gives the same output on every machine.

mod blur;
mod colour;

use std::fmt;

/// One kind of effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Brightness,
    Contrast,
    Exposure,
    Gamma,
    Saturation,
    Hue,
    Temperature,
    Invert,
    Grayscale,
    Sepia,
    Blur,
    Sharpen,
    Vignette,
    Pixelate,
}

/// One setting of an effect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    pub name: &'static str,
    /// What the number means, for display: `%`, `stops`, `deg`, `x`.
    pub unit: &'static str,
    pub default: f64,
    pub min: f64,
    pub max: f64,
    /// The value at which the effect changes nothing.
    pub neutral: f64,
}

const fn p(
    name: &'static str,
    unit: &'static str,
    default: f64,
    min: f64,
    max: f64,
    neutral: f64,
) -> ParamSpec {
    ParamSpec {
        name,
        unit,
        default,
        min,
        max,
        neutral,
    }
}

impl Kind {
    pub const ALL: [Kind; 14] = [
        Kind::Brightness,
        Kind::Contrast,
        Kind::Exposure,
        Kind::Gamma,
        Kind::Saturation,
        Kind::Hue,
        Kind::Temperature,
        Kind::Invert,
        Kind::Grayscale,
        Kind::Sepia,
        Kind::Blur,
        Kind::Sharpen,
        Kind::Vignette,
        Kind::Pixelate,
    ];

    /// The name used in project files and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Brightness => "brightness",
            Kind::Contrast => "contrast",
            Kind::Exposure => "exposure",
            Kind::Gamma => "gamma",
            Kind::Saturation => "saturation",
            Kind::Hue => "hue",
            Kind::Temperature => "temperature",
            Kind::Invert => "invert",
            Kind::Grayscale => "grayscale",
            Kind::Sepia => "sepia",
            Kind::Blur => "blur",
            Kind::Sharpen => "sharpen",
            Kind::Vignette => "vignette",
            Kind::Pixelate => "pixelate",
        }
    }

    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == name)
    }

    /// The settings, in the order values are passed to [`apply`].
    pub fn params(self) -> &'static [ParamSpec] {
        const BRIGHTNESS: &[ParamSpec] = &[p("amount", "%", 0.0, -100.0, 100.0, 0.0)];
        const CONTRAST: &[ParamSpec] = &[p("amount", "%", 0.0, -100.0, 100.0, 0.0)];
        const EXPOSURE: &[ParamSpec] = &[p("stops", "stops", 0.0, -5.0, 5.0, 0.0)];
        const GAMMA: &[ParamSpec] = &[p("gamma", "x", 1.0, 0.1, 5.0, 1.0)];
        const SATURATION: &[ParamSpec] = &[p("amount", "%", 100.0, 0.0, 300.0, 100.0)];
        const HUE: &[ParamSpec] = &[p("degrees", "deg", 0.0, -180.0, 180.0, 0.0)];
        const TEMPERATURE: &[ParamSpec] = &[p("amount", "%", 0.0, -100.0, 100.0, 0.0)];
        const MIX: &[ParamSpec] = &[p("amount", "%", 100.0, 0.0, 100.0, 0.0)];
        const BLUR: &[ParamSpec] = &[p("radius", "% of height", 1.0, 0.0, 20.0, 0.0)];
        const SHARPEN: &[ParamSpec] = &[
            p("amount", "%", 100.0, 0.0, 500.0, 0.0),
            p("radius", "% of height", 0.3, 0.0, 5.0, 0.0),
        ];
        const VIGNETTE: &[ParamSpec] = &[
            p("amount", "%", 50.0, 0.0, 100.0, 0.0),
            p("softness", "%", 50.0, 0.0, 100.0, 50.0),
        ];
        const PIXELATE: &[ParamSpec] = &[p("size", "% of height", 2.0, 0.0, 30.0, 0.0)];
        match self {
            Kind::Brightness => BRIGHTNESS,
            Kind::Contrast => CONTRAST,
            Kind::Exposure => EXPOSURE,
            Kind::Gamma => GAMMA,
            Kind::Saturation => SATURATION,
            Kind::Hue => HUE,
            Kind::Temperature => TEMPERATURE,
            Kind::Invert | Kind::Grayscale | Kind::Sepia => MIX,
            Kind::Blur => BLUR,
            Kind::Sharpen => SHARPEN,
            Kind::Vignette => VIGNETTE,
            Kind::Pixelate => PIXELATE,
        }
    }

    /// The index of a setting by name.
    pub fn param_index(self, name: &str) -> Option<usize> {
        self.params().iter().position(|s| s.name == name)
    }

    /// The default values, one per setting.
    pub fn defaults(self) -> Vec<f64> {
        self.params().iter().map(|s| s.default).collect()
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// True if `values` change nothing for this kind of effect.
///
/// Vignette is neutral at amount 0 whatever its softness; sharpen at amount 0 or radius 0.
pub fn is_neutral(kind: Kind, values: &[f64]) -> bool {
    let v = |i: usize| {
        values
            .get(i)
            .copied()
            .unwrap_or_else(|| kind.params()[i].default)
    };
    match kind {
        Kind::Sharpen => v(0) <= 0.0 || v(1) <= 0.0,
        Kind::Vignette => v(0) <= 0.0,
        _ => kind
            .params()
            .iter()
            .enumerate()
            .all(|(i, s)| (v(i) - s.neutral).abs() < 1e-9),
    }
}

/// Clamp every value into its allowed range; missing values take their defaults.
pub fn clamp_values(kind: Kind, values: &[f64]) -> Vec<f64> {
    kind.params()
        .iter()
        .enumerate()
        .map(|(i, s)| {
            values
                .get(i)
                .copied()
                .unwrap_or(s.default)
                .clamp(s.min, s.max)
        })
        .collect()
}

/// Apply an effect to a `w` x `h` straight-alpha RGBA8 picture in place. Values are in the order of
/// [`Kind::params`] and are clamped to their ranges.
pub fn apply(kind: Kind, values: &[f64], rgba: &mut [u8], w: usize, h: usize) {
    debug_assert_eq!(rgba.len(), w * h * 4);
    if w == 0 || h == 0 || is_neutral(kind, values) {
        return;
    }
    let v = clamp_values(kind, values);
    match kind {
        Kind::Brightness => colour::brightness(rgba, v[0]),
        Kind::Contrast => colour::contrast(rgba, v[0]),
        Kind::Exposure => colour::exposure(rgba, v[0]),
        Kind::Gamma => colour::gamma(rgba, v[0]),
        Kind::Saturation => colour::saturation(rgba, v[0] / 100.0),
        Kind::Hue => colour::hue(rgba, v[0]),
        Kind::Temperature => colour::temperature(rgba, v[0]),
        Kind::Invert => colour::invert(rgba, v[0] / 100.0),
        Kind::Grayscale => colour::grayscale(rgba, v[0] / 100.0),
        Kind::Sepia => colour::sepia(rgba, v[0] / 100.0),
        Kind::Blur => blur::blur(rgba, w, h, v[0] / 100.0 * h as f64),
        Kind::Sharpen => blur::sharpen(rgba, w, h, v[0] / 100.0, v[1] / 100.0 * h as f64),
        Kind::Vignette => colour::vignette(rgba, w, h, v[0] / 100.0, v[1] / 100.0),
        Kind::Pixelate => blur::pixelate(rgba, w, h, v[0] / 100.0 * h as f64),
    }
}

#[cfg(test)]
mod tests;
