// SPDX-License-Identifier: GPL-3.0-or-later
//! The Render Engine's error type.

use std::path::PathBuf;

use timeline_engine::{AssetId, SequenceId};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("timeline: {0}")]
    Timeline(#[from] timeline_engine::Error),

    #[error("media: {0}")]
    Media(media_engine::Error),

    #[error("no such sequence: {0}")]
    NoSuchSequence(SequenceId),

    /// A clip uses a file that is not on disk.
    #[error("media file is missing: {name} ({path})")]
    MissingMedia {
        asset: AssetId,
        name: String,
        path: PathBuf,
    },

    /// A drawn graphic could not be drawn (usually its font).
    #[error("graphic: {0}")]
    Graphic(String),

    /// The range to render holds no frames.
    #[error("nothing to render: the sequence or range is empty")]
    Empty,

    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Stopped through the progress callback.
    #[error("cancelled")]
    Cancelled,
}

impl From<media_engine::Error> for Error {
    fn from(e: media_engine::Error) -> Self {
        match e {
            media_engine::Error::Cancelled => Error::Cancelled,
            other => Error::Media(other),
        }
    }
}
