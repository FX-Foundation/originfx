// SPDX-License-Identifier: GPL-3.0-or-later
//! The look of a picture clip: opacity, position, scale, rotation, crop, blend mode and mask.
//!
//! The pixel maths is in the Compositing Engine. This module holds what the project remembers.
//! Every number here can also be animated (see [`crate::animation`]); an animated number's curve
//! replaces its fixed value. Times in a clip's curves are measured from the clip's start.

use crate::animation::Param;

pub use compositing_engine::{Affine, BlendMode, MaskShape, MaskSpec};

/// The fixed (not animated) transform values of a picture clip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Percent, 100 is fully solid.
    pub opacity: f64,
    /// Move right and down, percent of the frame width and height.
    pub x: f64,
    pub y: f64,
    /// Size, percent.
    pub scale_x: f64,
    pub scale_y: f64,
    /// Clockwise turn, degrees.
    pub rotation: f64,
    /// Crop from each side, percent of the picture's width or height.
    pub crop_left: f64,
    pub crop_right: f64,
    pub crop_top: f64,
    pub crop_bottom: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Transform {
            opacity: 100.0,
            x: 0.0,
            y: 0.0,
            scale_x: 100.0,
            scale_y: 100.0,
            rotation: 0.0,
            crop_left: 0.0,
            crop_right: 0.0,
            crop_top: 0.0,
            crop_bottom: 0.0,
        }
    }
}

// Values are always finite and in range (edits and loading refuse anything else).
impl Eq for Transform {}

impl Transform {
    pub fn is_default(&self) -> bool {
        *self == Transform::default()
    }

    /// The fixed value of a transform parameter (`None` for other parameters).
    pub fn get(&self, p: Param) -> Option<f64> {
        Some(match p {
            Param::Opacity => self.opacity,
            Param::PositionX => self.x,
            Param::PositionY => self.y,
            Param::ScaleX => self.scale_x,
            Param::ScaleY => self.scale_y,
            Param::Rotation => self.rotation,
            Param::CropLeft => self.crop_left,
            Param::CropRight => self.crop_right,
            Param::CropTop => self.crop_top,
            Param::CropBottom => self.crop_bottom,
            _ => return None,
        })
    }

    pub(crate) fn set(&mut self, p: Param, v: f64) -> bool {
        let slot = match p {
            Param::Opacity => &mut self.opacity,
            Param::PositionX => &mut self.x,
            Param::PositionY => &mut self.y,
            Param::ScaleX => &mut self.scale_x,
            Param::ScaleY => &mut self.scale_y,
            Param::Rotation => &mut self.rotation,
            Param::CropLeft => &mut self.crop_left,
            Param::CropRight => &mut self.crop_right,
            Param::CropTop => &mut self.crop_top,
            Param::CropBottom => &mut self.crop_bottom,
            _ => return false,
        };
        *slot = v;
        true
    }

    /// The transform parameters in a fixed order.
    pub const PARAMS: [Param; 10] = [
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
    ];
}

/// A mask on a picture clip: the layer shows only inside (or, inverted, outside) a shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mask {
    pub shape: MaskShape,
    pub invert: bool,
    /// Centre, percent of the frame.
    pub x: f64,
    pub y: f64,
    /// Full size, percent of the frame.
    pub width: f64,
    pub height: f64,
    /// Soft edge, percent of the frame height.
    pub feather: f64,
}

impl Eq for Mask {}

impl Mask {
    pub fn new(shape: MaskShape) -> Mask {
        Mask {
            shape,
            invert: false,
            x: 50.0,
            y: 50.0,
            width: 50.0,
            height: 50.0,
            feather: 0.0,
        }
    }

    pub fn get(&self, p: Param) -> Option<f64> {
        Some(match p {
            Param::MaskX => self.x,
            Param::MaskY => self.y,
            Param::MaskWidth => self.width,
            Param::MaskHeight => self.height,
            Param::MaskFeather => self.feather,
            _ => return None,
        })
    }

    pub(crate) fn set(&mut self, p: Param, v: f64) -> bool {
        let slot = match p {
            Param::MaskX => &mut self.x,
            Param::MaskY => &mut self.y,
            Param::MaskWidth => &mut self.width,
            Param::MaskHeight => &mut self.height,
            Param::MaskFeather => &mut self.feather,
            _ => return false,
        };
        *slot = v;
        true
    }

    pub const PARAMS: [Param; 5] = [
        Param::MaskX,
        Param::MaskY,
        Param::MaskWidth,
        Param::MaskHeight,
        Param::MaskFeather,
    ];
}

/// How a picture clip is laid over the layers beneath it at one moment: every animated value
/// worked out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// 0 to 1.
    pub opacity: f64,
    pub affine: Affine,
    /// Left, right, top, bottom, percent.
    pub crop: [f64; 4],
    pub mask: Option<MaskSpec>,
    pub blend: BlendMode,
}

impl Look {
    /// True when the clip is simply drawn over what is beneath, unchanged.
    pub fn is_plain(&self) -> bool {
        self.opacity >= 1.0
            && self.blend == BlendMode::Normal
            && self.affine.is_identity()
            && self.crop == [0.0; 4]
            && self.mask.is_none()
    }
}
