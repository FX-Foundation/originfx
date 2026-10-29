// SPDX-License-Identifier: GPL-3.0-or-later
//! # OriginFX Audio Engine
//!
//! The sound maths of OriginFX, as small pure functions and one streaming meter. It knows
//! nothing about timelines or files: it takes interleaved `f32` samples and returns
//! interleaved `f32` samples or numbers. The [Render Engine](../render_engine/index.html)
//! uses it to mix a timeline; an editor can use it for meters and waveforms.
//!
//! * [`gain`]: decibels to and from linear gain, and the pan law.
//! * [`fade`]: fade curves.
//! * [`limit`]: hard and soft clipping for the master output. Both are stateless, so a mix is the
//!   same however it is cut into blocks.
//! * [`loudness`]: ITU-R BS.1770 / EBU R128 integrated loudness (LUFS), sample peak and RMS.
//! * [`peaks`]: min/max buckets for drawing waveforms.

pub mod fade;
pub mod gain;
pub mod limit;
pub mod loudness;
pub mod peaks;

pub use fade::{fade_in_gain, fade_out_gain, FadeCurve};
pub use gain::{db_to_gain, gain_to_db, pan_gains, MIN_DB};
pub use limit::{apply_limiter, Limiter};
pub use loudness::{measure, Loudness, LoudnessMeter};
pub use peaks::{peaks, Peak};
