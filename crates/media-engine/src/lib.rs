//! # OriginFX Media Engine
//!
//! The first of OriginFX's ten engines. It owns everything that touches a
//! media *file*: reading metadata, decoding frames and audio, seeking,
//! caching decoded frames, encoding, and generating proxies.
//!
//! The engine is a plain library: no windows, no UI, no global state beyond
//! the one-time FFmpeg initialisation.

pub mod cache;
pub mod decode;
pub mod error;
pub mod hwaccel;
pub mod preset;
pub mod probe;
pub mod proxy;
pub mod transcode;
pub mod types;

pub use error::{Error, Result};
pub use cache::{CacheStats, FrameCache, FrameReader};
pub use decode::{AudioDecoder, AudioOptions, VideoDecoder};
pub use proxy::{ProxyManager, ProxySettings};
pub use preset::{ExportPreset, Quality};
pub use probe::probe;
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
