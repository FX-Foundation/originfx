// SPDX-License-Identifier: GPL-3.0-or-later
//! # OriginFX Render Engine
//!
//! Turns a timeline into pictures, sound and finished files.
//!
//! * A [`Renderer`] takes a snapshot of a [`timeline_engine::Project`] and a sequence and answers
//!   "what does frame 120 look like?" ([`Renderer::frame`]) and "what is heard between these two
//!   moments?" ([`Renderer::audio`]). Layers are fitted to the frame ([`Fit`]), blended bottom to
//!   top with straight alpha, and sound is mixed with mute and solo respected.
//! * [`export_sequence`] renders a whole sequence (or an in/out range) into any of the Media
//!   Engine's export presets, with progress and cancel.
//! * Pictures and sound come from a [`MediaSource`]. [`FileSource`] reads real files; tests and
//!   proxy workflows plug in their own.
//!
//! This is the CPU renderer: the same code on every machine gives the same pixels, and it is the
//! reference that a future GPU renderer is checked against.

pub mod composite;
pub mod error;
pub mod export;
pub mod renderer;
pub mod scale;
pub mod source;
pub mod transition;

pub use composite::{blend_over, placement, solid, Fit, Placement};
pub use error::{Error, Result};
pub use export::{
    export_sequence, export_with_source, ExportOptions, Phase, RenderProgress, RenderReport,
};
pub use renderer::{MissingMedia, RenderSettings, Renderer, OFFLINE_COLOUR};
pub use scale::resize;
pub use source::{FileSource, MediaSource};
