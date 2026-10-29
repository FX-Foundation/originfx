// SPDX-License-Identifier: GPL-3.0-or-later
//! Animation: values on clips, tracks and the master that change over time.
//!
//! An [`Animation`] holds one [`Curve`] per [`Param`]. A parameter with a curve is *animated*:
//! the curve replaces the fixed value (the fixed value is then ignored, and changing it is
//! refused until the curve is cleared).
//!
//! Times in a clip's curves are measured from the start of the clip, so they move with it. Times
//! in a track's or the master's curves are measured from the start of the sequence.

use std::collections::BTreeMap;
use std::fmt;

pub use keyframe_engine::{Curve, Interp, Key};

use crate::ids::{ClipId, TrackId};
use crate::retime::{MAX_SPEED, MIN_SPEED};

/// A value that can be animated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Param {
    /// Level in decibels, -120 to +24. (Clips, audio tracks and the master.)
    Gain,
    /// Pan in percent, -100 (left) to 100 (right). (Audio clips and audio tracks.)
    Pan,
    /// Playback speed as a ratio (1 is normal), 0.01 to 100. (Clips.) This is a *speed ramp*.
    Speed,
    /// Picture clips: how solid the layer is, percent.
    Opacity,
    /// Picture clips: move right, percent of the frame width.
    PositionX,
    /// Picture clips: move down, percent of the frame height.
    PositionY,
    /// Picture clips: horizontal size, percent.
    ScaleX,
    /// Picture clips: vertical size, percent.
    ScaleY,
    /// Picture clips: clockwise turn in degrees.
    Rotation,
    /// Picture clips: crop from the left, percent of the width.
    CropLeft,
    CropRight,
    CropTop,
    CropBottom,
    /// Picture clips with a mask: the mask's centre, percent of the frame.
    MaskX,
    MaskY,
    /// The mask's size, percent of the frame.
    MaskWidth,
    MaskHeight,
    /// The mask's soft edge, percent of the frame height.
    MaskFeather,
}

impl Param {
    pub const ALL: [Param; 18] = [
        Param::Gain,
        Param::Pan,
        Param::Speed,
        Param::Opacity,
        Param::PositionX,
        Param::PositionY,
        Param::ScaleX,
        Param::ScaleY,
        Param::Rotation,
        Param::CropLeft,
        Param::CropRight,
        Param::CropTop,
        Param::CropBottom,
        Param::MaskX,
        Param::MaskY,
        Param::MaskWidth,
        Param::MaskHeight,
        Param::MaskFeather,
    ];

    /// What an audio clip can animate.
    pub const AUDIO: [Param; 3] = [Param::Gain, Param::Pan, Param::Speed];

    /// What a picture clip can animate.
    pub const PICTURE: [Param; 16] = [
        Param::Speed,
        Param::Opacity,
        Param::PositionX,
        Param::PositionY,
        Param::ScaleX,
        Param::ScaleY,
        Param::Rotation,
        Param::CropLeft,
        Param::CropRight,
        Param::CropTop,
        Param::CropBottom,
        Param::MaskX,
        Param::MaskY,
        Param::MaskWidth,
        Param::MaskHeight,
        Param::MaskFeather,
    ];

    /// True for the look of a picture clip (everything except gain, pan and speed).
    pub fn is_look(self) -> bool {
        !matches!(self, Param::Gain | Param::Pan | Param::Speed)
    }

    /// True for the parameters of a mask.
    pub fn is_mask(self) -> bool {
        matches!(
            self,
            Param::MaskX | Param::MaskY | Param::MaskWidth | Param::MaskHeight | Param::MaskFeather
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Param::Gain => "gain",
            Param::Pan => "pan",
            Param::Speed => "speed",
            Param::Opacity => "opacity",
            Param::PositionX => "x",
            Param::PositionY => "y",
            Param::ScaleX => "scale-x",
            Param::ScaleY => "scale-y",
            Param::Rotation => "rotation",
            Param::CropLeft => "crop-left",
            Param::CropRight => "crop-right",
            Param::CropTop => "crop-top",
            Param::CropBottom => "crop-bottom",
            Param::MaskX => "mask-x",
            Param::MaskY => "mask-y",
            Param::MaskWidth => "mask-width",
            Param::MaskHeight => "mask-height",
            Param::MaskFeather => "mask-feather",
        }
    }

    pub fn parse(name: &str) -> Option<Param> {
        Param::ALL.into_iter().find(|p| p.as_str() == name)
    }

    /// The allowed values, lowest and highest.
    pub fn range(self) -> (f64, f64) {
        match self {
            Param::Gain => (-120.0, 24.0),
            Param::Pan => (-100.0, 100.0),
            Param::Speed => (MIN_SPEED, MAX_SPEED),
            Param::Opacity => (0.0, 100.0),
            Param::PositionX | Param::PositionY => (-300.0, 300.0),
            Param::ScaleX | Param::ScaleY => (0.0, 1000.0),
            Param::Rotation => (-3600.0, 3600.0),
            Param::CropLeft | Param::CropRight | Param::CropTop | Param::CropBottom => (0.0, 100.0),
            Param::MaskX | Param::MaskY => (-100.0, 200.0),
            Param::MaskWidth | Param::MaskHeight => (0.0, 300.0),
            Param::MaskFeather => (0.0, 50.0),
        }
    }

    pub fn unit(self) -> &'static str {
        match self {
            Param::Gain => "dB",
            Param::Pan => "%",
            Param::Speed => "x",
            Param::Rotation => "deg",
            _ => "%",
        }
    }

    pub fn contains(self, value: f64) -> bool {
        let (lo, hi) = self.range();
        value.is_finite() && value >= lo && value <= hi
    }
}

impl fmt::Display for Param {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a keyframe edit applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Clip(ClipId),
    Track(TrackId),
    /// The master level of the sequence.
    Master,
}

/// The curves of one clip, track or master.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Animation {
    curves: BTreeMap<Param, Curve>,
}

impl Animation {
    pub fn is_empty(&self) -> bool {
        self.curves.is_empty()
    }

    /// The curve of a parameter, if it is animated.
    pub fn get(&self, param: Param) -> Option<&Curve> {
        self.curves.get(&param)
    }

    pub fn is_animated(&self, param: Param) -> bool {
        self.curves.contains_key(&param)
    }

    /// Every animated parameter and its curve.
    pub fn iter(&self) -> impl Iterator<Item = (Param, &Curve)> + '_ {
        self.curves.iter().map(|(p, c)| (*p, c))
    }

    /// Replace a parameter's curve. An empty curve removes the animation.
    pub fn set(&mut self, param: Param, curve: Curve) {
        if curve.is_empty() {
            self.curves.remove(&param);
        } else {
            self.curves.insert(param, curve);
        }
    }

    /// Stop animating a parameter. Returns whether it was animated.
    pub fn remove(&mut self, param: Param) -> bool {
        self.curves.remove(&param).is_some()
    }

    /// Every curve cut to `a..b` (ticks) and moved so `a` becomes zero.
    pub fn slice(&self, a: i64, b: i64) -> Animation {
        Animation {
            curves: self
                .curves
                .iter()
                .map(|(p, c)| (*p, c.slice(a, b)))
                .collect(),
        }
    }

    /// Check keys against the allowed parameters, value ranges and time span `0..=max` ticks
    /// (`None` for no upper limit). Returns a description of the first problem.
    pub(crate) fn problem(&self, allowed: &[Param], max: Option<i64>) -> Option<String> {
        for (param, curve) in &self.curves {
            if !allowed.contains(param) {
                return Some(format!("{param} cannot be animated here"));
            }
            for k in curve.keys() {
                if !param.contains(k.value) {
                    let (lo, hi) = param.range();
                    return Some(format!(
                        "{param} keyframe {} is outside {lo} to {hi}",
                        k.value
                    ));
                }
                if k.t < 0 || max.is_some_and(|m| k.t > m) {
                    return Some(format!("{param} keyframe lies outside its span"));
                }
            }
        }
        None
    }
}
