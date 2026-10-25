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
pub mod probe;
pub mod types;

pub use error::{Error, Result};
pub use cache::{CacheStats, FrameCache, FrameReader};
pub use decode::{AudioDecoder, AudioOptions, VideoDecoder};
pub use probe::probe;
pub use types::*;

use std::sync::Once;

/// Initialise FFmpeg exactly once. Every public entry point calls this, so
/// callers never need to.
pub fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = ffmpeg_next::init();
        ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Error);
    });
}
