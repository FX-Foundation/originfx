// SPDX-License-Identifier: GPL-3.0-or-later
//! # OriginFX Compositing Engine
//!
//! Putting layers together: opacity, blend modes, position, scale and rotation, crop and masks,
//! on straight-alpha RGBA8 pictures.
//!
//! Pure CPU code with no dependencies. Four steps, each a plain function on a whole picture:
//!
//! 1. [`crop`]: cut away the edges of a layer (percent of its size).
//! 2. [`mask`]: fade the layer's transparency by a rectangle or ellipse, optionally soft or inverted.
//! 3. [`transform`]: move, scale and rotate the layer about the middle of the frame.
//! 4. [`composite`]: lay it over the picture beneath with an opacity and a [`BlendMode`].
//!
//! * Neutral settings change no bits: an identity transform is not run at all ([`Affine::is_identity`]),
//!   opacity 100 % with `Normal` is the plain "over" of the Render Engine, and a layer that is
//!   fully transparent changes nothing.
//! * Sizes and positions are percent of the frame, so a look does not depend on the resolution.
//! * The same input always gives the same output on every machine.

mod blend;
mod geometry;

pub use blend::{composite, BlendMode};
pub use geometry::{crop, mask, transform, Affine, MaskShape, MaskSpec};

#[cfg(test)]
mod tests;
