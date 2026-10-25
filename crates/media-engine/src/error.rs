//! Error type shared by every Media Engine module.

use std::path::PathBuf;

/// Convenience alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong inside the Media Engine.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error reported by FFmpeg itself.
    #[error("ffmpeg: {0}")]
    Ffmpeg(#[from] ffmpeg_next::Error),

    /// Plain I/O failure (missing file, permissions, ...).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// The input file does not exist.
    #[error("file not found: {0}")]
    NotFound(PathBuf),

    /// The file has no stream of the requested kind.
    #[error("no {0} stream in input")]
    NoStream(&'static str),

    /// A codec or encoder needed by the request is not available in this FFmpeg build.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// The caller passed an argument that makes no sense.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// The operation was cancelled through its progress callback.
    #[error("cancelled")]
    Cancelled,
}
