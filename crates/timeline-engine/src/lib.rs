// SPDX-License-Identifier: GPL-3.0-or-later
//! # OriginFX Timeline Engine
//!
//! The editing brain of OriginFX: what is on the timeline, and every way to change it.
//! It never touches pixels or samples; it says *which* part of *which* file is on screen
//! or heard at any moment, and the Media Engine supplies the actual frames.
//!
//! ## The shape of it
//!
//! * A [`Project`] holds [`Asset`]s (media files and their facts) and [`Sequence`]s.
//! * A sequence has video and audio [`Track`]s of non-overlapping [`Clip`]s, and [`Marker`]s.
//! * Time is a [`Time`]: a whole number of ticks (1/254,016,000,000 s), so 23.976, 24, 25,
//!   29.97, 30, 50, 59.94, 60 fps and every common audio rate land exactly, with no drift.
//! * The only way to change a project is an [`Editor`]. Each edit runs on a private copy,
//!   is checked against every rule, and is committed or thrown away whole.
//! * Projects are persistent data: copies share what did not change. Undo is therefore just
//!   "go back to the old project", and a snapshot handed to a player thread never changes.
//!
//! ```
//! use timeline_engine::*;
//!
//! let mut ed = Editor::new("demo");
//! let seq = ed.add_sequence(SequenceSettings::default())?;
//! let asset = ed.add_asset(NewAsset {
//!     name: "interview".into(),
//!     path: "interview.mp4".into(),
//!     duration: Some(Time::from_secs_f64(60.0)),
//!     video: vec![VideoProps { width: 1920, height: 1080, frame_rate: Some(FrameRate::FPS_24) }],
//!     audio: vec![AudioProps { sample_rate: 48_000, channels: 2 }],
//! })?;
//! let s = ed.project().sequence(seq).unwrap();
//! let (v1, a1) = (s.video_tracks()[0].id, s.audio_tracks()[0].id);
//!
//! // Picture and sound arrive as one linked pair.
//! let placed = ed.place_asset(seq, asset, Time::ZERO, Time::ZERO, Some(Time::from_secs_f64(10.0)),
//!                             Some(v1), Some(a1), PlaceMode::Overwrite)?;
//! // Cut at 4 s and ripple-delete the first half: the rest slides back to 0.
//! let (first, _second) = ed.split_clip(seq, placed.video.unwrap(), Time::from_secs_f64(4.0), LinkMode::Follow)?;
//! ed.delete_clips(seq, &[first], Ripple::Sync, LinkMode::Follow)?;
//! assert_eq!(ed.project().sequence(seq).unwrap().duration(), Time::from_secs_f64(6.0));
//! ed.undo()?;
//! # Ok::<(), Error>(())
//! ```
//!
//! With the `media` cargo feature, [`media::probe_asset`] reads a real file through the
//! Media Engine and returns a ready [`NewAsset`].

pub mod audio;
pub mod editor;
pub mod error;
pub mod file;
pub mod ids;
#[cfg(feature = "media")]
pub mod media;
pub mod model;
pub mod ops;
pub mod query;
pub mod retime;
pub mod time;

pub use audio::{ClipAudio, Fade, FadeCurve, Gain, Pan};
pub use editor::Editor;
pub use error::{Error, Result};
pub use ids::*;
pub use model::*;
pub use ops::{ClipSpec, Edge, LinkMode, PlaceMode, Placed, Ripple, TrackUpdate};
pub use query::{ActiveTransition, AudioSource, Side, VideoLayer};
pub use retime::{
    Alignment, Direction, Speed, SpeedMode, Transition, TransitionKind, MAX_SPEED,
    MAX_TRANSITION_SECS, MIN_SPEED,
};
pub use time::*;
