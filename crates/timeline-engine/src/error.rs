// SPDX-License-Identifier: GPL-3.0-or-later
//! The Timeline Engine's error type.

use crate::ids::{AssetId, ClipId, SequenceId, TrackId};
use crate::time::Time;

/// Everything that can go wrong in the Timeline Engine.
///
/// Edit operations are all-or-nothing: when one returns an error the project is exactly as it was.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no such sequence: {0}")]
    NoSuchSequence(SequenceId),
    #[error("no such track: {0}")]
    NoSuchTrack(TrackId),
    #[error("no such clip: {0}")]
    NoSuchClip(ClipId),
    #[error("no such asset: {0}")]
    NoSuchAsset(AssetId),
    #[error("{0} is locked")]
    TrackLocked(TrackId),
    #[error("{asset} has no {wanted} stream {stream} to put on a {wanted} track")]
    WrongTrackKind {
        asset: AssetId,
        wanted: &'static str,
        stream: u32,
    },
    #[error("a clip must last at least one frame")]
    TooShort,
    #[error("time {0} is before the start of the sequence")]
    BeforeStart(Time),
    #[error("the source of {clip} would run outside the media ({detail})")]
    OutsideSource { clip: ClipId, detail: String },
    #[error("{clip} would run into {other}")]
    Collision { clip: ClipId, other: ClipId },
    #[error("{time} is not inside {clip}")]
    NotInsideClip { clip: ClipId, time: Time },
    #[error("{0} needs a clip right next to it on both sides")]
    NeedsNeighbours(ClipId),
    #[error("{left} and {right} are not next to each other")]
    NotAdjacent { left: ClipId, right: ClipId },
    #[error("the edit is blocked on {track} by {clip}")]
    Blocked { track: TrackId, clip: ClipId },
    #[error("{0} is still used by a clip")]
    AssetInUse(AssetId),
    #[error("a still image needs a duration")]
    NeedsDuration,
    #[error("track {0} cannot move that far")]
    NoTrackThere(TrackId),
    #[error("nothing to {0}")]
    NothingTo(&'static str),
    #[error("invalid project: {0}")]
    Invalid(String),
    #[error("project file: {0}")]
    File(String),
    #[error("project file is version {found}, this build reads up to version {supported}")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("media: {0}")]
    Media(String),
}

/// Shorthand for `std::result::Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;
