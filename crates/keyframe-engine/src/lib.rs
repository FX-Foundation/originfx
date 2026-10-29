// SPDX-License-Identifier: GPL-3.0-or-later
//! # OriginFX Keyframe Engine
//!
//! Values that change over time. A [`Curve`] is a list of [`Key`]s (a time and a value each) and
//! says what the value is at any moment between them.
//!
//! The engine is pure maths with no dependencies. Times are plain `i64` ticks, so it works with
//! the Timeline Engine's `Time` (which is a count of ticks) without depending on it. Values are
//! `f64` and mean whatever the caller says: decibels for volume, percent for pan, a ratio for
//! speed, and later position, scale or opacity.
//!
//! ## Interpolation
//!
//! Each key says how to travel from it to the next key:
//!
//! * [`Interp::Hold`]: keep this value until the next key, then jump.
//! * [`Interp::Linear`]: a straight line.
//! * [`Interp::Smooth`]: a smooth cubic curve. Each key has a *tangent* (its slope); left on
//!   automatic, a key's slope is zero at the ends of the curve and otherwise the monotone
//!   average of the slopes either side, so a smooth curve through a set of keys never overshoots
//!   them. It eases in and out at the ends and flows through the middle.
//!
//! Before the first key the curve holds the first value; after the last it holds the last.
//!
//! ## Cutting
//!
//! [`Curve::slice`] cuts a curve to a time range and shifts it to start at zero, and the pieces
//! evaluate **exactly** as the original did, including smooth segments (a cubic cut in two is
//! two cubics). That is what lets an edit split a clip without changing how its animation looks.
//!
//! ## Integrals
//!
//! [`Curve::integral`] gives the exact area under the curve. It is how a changing speed becomes a
//! position in the media: if the curve is a speed, its integral is how far the media has played.

mod curve;

pub use curve::{Curve, CurveError, Interp, Key};
