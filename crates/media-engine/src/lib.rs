//! # OriginFX Media Engine
//!
//! The first of OriginFX's ten engines. It owns everything that touches a
//! media *file*: reading metadata, decoding frames and audio, seeking,
//! caching decoded frames, encoding, and generating proxies.
//!
//! The engine is a plain library: no windows, no UI, no global state beyond
//! the one-time FFmpeg initialisation. Nothing in its public API exposes an
//! FFmpeg type, so other engines never depend on FFmpeg directly.
//!
//! ## Tour
//!
//! | You want to...                          | Use                                   |
//! |-----------------------------------------|---------------------------------------|
//! | know what is in a file                  | [`probe`]                             |
//! | read frames one after another           | [`VideoDecoder`]                      |
//! | jump to any frame, repeatedly, fast     | [`FrameReader`] (decoder + [`FrameCache`]) |
//! | read audio as `f32` samples             | [`AudioDecoder`]                      |
//! | write a file in a standard format       | [`export`] with an [`ExportPreset`]   |
//! | make scrubbing cheap for heavy footage  | [`ProxyManager`]                      |
//! | see which GPU APIs exist (probe only)   | [`hwaccel`]                           |
//!
//! ```no_run
//! use media_engine::{export, probe, ExportPreset, ExportSettings, FrameReader, Quality};
//!
//! # fn main() -> media_engine::Result<()> {
//! let info = probe("holiday.mp4")?;
//! let video = info.primary_video().expect("has video");
//! println!("{}x{} {}", video.width, video.height, video.codec);
//!
//! // Random access with a 512 MiB frame cache.
//! let mut reader = FrameReader::open("holiday.mp4", 512 * 1024 * 1024)?;
//! let frame = reader.frame_at(12.5)?; // RGBA8, upright, frame-accurate
//! assert_eq!(frame.data.len(), (frame.width * frame.height * 4) as usize);
//!
//! // Re-encode as a 720p H.264 MP4.
//! let settings = ExportSettings::new(ExportPreset::Mp4H264).quality(Quality::Standard).height(720);
//! export("holiday.mp4", "holiday-720p.mp4", &settings)?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Conventions
//!
//! * Times are seconds from the start of the stream, as `f64`. Container
//!   start offsets (common in MPEG-TS) are removed for you.
//! * Video frames are RGBA8, tightly packed, already rotated upright.
//! * Audio is interleaved `f32`.
//! * Export output is constant frame rate.
//! * Set `ORIGINFX_FFMPEG_LOG=debug` (or `info`, `warning`, `quiet`) to see FFmpeg's own log.

pub mod cache;
pub mod colour;
pub mod decode;
pub mod error;
pub mod hwaccel;
pub mod index;
pub mod preset;
pub mod probe;
pub mod proxy;
pub mod transcode;
pub mod types;

pub use cache::{CacheStats, FrameCache, FrameReader};
pub use colour::ColourInfo;
pub use decode::{AudioDecoder, AudioOptions, VideoDecoder};
pub use error::{Error, Result};
pub use hwaccel::HwMode;
pub use index::FrameIndex;
pub use preset::{ExportPreset, Quality};
pub use probe::probe;
pub use proxy::{ProxyManager, ProxySettings};
pub use transcode::{export, export_with_progress, ExportReport, ExportSettings, Progress};
pub use types::*;

use std::sync::Once;

/// Initialise FFmpeg exactly once. Every public entry point calls this, so
/// callers never need to.
pub fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = ffmpeg_next::init();
        // Quiet by default; set ORIGINFX_FFMPEG_LOG=debug|info|warning|error|quiet to see FFmpeg's own log.
        let level = match std::env::var("ORIGINFX_FFMPEG_LOG").as_deref() {
            Ok("quiet") => ffmpeg_next::log::Level::Quiet,
            Ok("warning") => ffmpeg_next::log::Level::Warning,
            Ok("info") => ffmpeg_next::log::Level::Info,
            Ok("debug") => ffmpeg_next::log::Level::Debug,
            _ => ffmpeg_next::log::Level::Error,
        };
        ffmpeg_next::log::set_level(level);
    });
}
