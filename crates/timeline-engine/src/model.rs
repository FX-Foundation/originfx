// SPDX-License-Identifier: GPL-3.0-or-later
//! The project model: assets, clips, tracks, sequences and the project itself.
//!
//! Everything here is a plain value. The collections are persistent
//! (structurally shared) so copying a whole project is cheap, which is what
//! makes undo, redo and background readers simple and safe.
//!
//! Nothing in this module changes a project; edits go through
//! [`crate::Editor`], which keeps every project valid.

use std::collections::HashSet;
use std::path::PathBuf;

use im::{OrdMap, Vector};

use crate::error::{Error, Result};
use crate::ids::{AssetId, ClipId, LinkId, MarkerId, SequenceId, TrackId};
use crate::time::{FrameRate, Time, TimeRange, Timecode};

/// One video stream of a media file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoProps {
    pub width: u32,
    pub height: u32,
    /// `None` for still images.
    pub frame_rate: Option<FrameRate>,
}

/// One audio stream of a media file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioProps {
    pub sample_rate: u32,
    pub channels: u16,
}

/// What is needed to add a media file to a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAsset {
    pub name: String,
    pub path: PathBuf,
    /// Length of the media. `None` for a still image, which can be held for any length.
    pub duration: Option<Time>,
    pub video: Vec<VideoProps>,
    pub audio: Vec<AudioProps>,
}

/// A media file known to the project.
///
/// The project stores the facts it needs to edit (length, sizes, rates), not the
/// pixels. Frames and samples come from the Media Engine when they are wanted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub id: AssetId,
    pub name: String,
    pub path: PathBuf,
    pub duration: Option<Time>,
    pub video: Vec<VideoProps>,
    pub audio: Vec<AudioProps>,
}

impl Asset {
    /// True if the file has at least one video stream (or is an image).
    pub fn has_video(&self) -> bool {
        !self.video.is_empty()
    }

    /// True if the file has at least one audio stream.
    pub fn has_audio(&self) -> bool {
        !self.audio.is_empty()
    }

    /// True for a still image: video with no length of its own.
    pub fn is_still(&self) -> bool {
        self.has_video() && self.duration.is_none()
    }
}

/// Which kind of track something is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TrackKind {
    Video,
    Audio,
}

impl TrackKind {
    pub(crate) fn word(self) -> &'static str {
        match self {
            TrackKind::Video => "video",
            TrackKind::Audio => "audio",
        }
    }
}

/// A piece of an asset placed on a track.
///
/// The clip shows `duration` of the asset starting `source_in` into it, and
/// shows it from timeline position `start`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub id: ClipId,
    pub asset: AssetId,
    /// Which video or audio stream of the asset, counted within its kind (0 is the first).
    pub stream: u32,
    pub name: String,
    /// Where the clip starts on the timeline.
    pub start: Time,
    /// Where in the asset the clip starts.
    pub source_in: Time,
    pub duration: Time,
    /// A disabled clip stays in place but is skipped when playing or rendering.
    pub enabled: bool,
    /// Clips with the same link move, split and trim together.
    pub link: Option<LinkId>,
}

impl Clip {
    /// Where the clip ends on the timeline (this time itself is not part of the clip).
    pub fn end(&self) -> Time {
        self.start + self.duration
    }

    /// The span of timeline the clip covers.
    pub fn range(&self) -> TimeRange {
        TimeRange {
            start: self.start,
            end: self.end(),
        }
    }

    /// The span of the asset the clip shows.
    pub fn source_range(&self) -> TimeRange {
        TimeRange {
            start: self.source_in,
            end: self.source_in + self.duration,
        }
    }

    /// The position in the asset shown at timeline time `t`.
    pub fn source_time_at(&self, t: Time) -> Time {
        self.source_in + (t - self.start)
    }
}

/// One video or audio track: an ordered row of clips that never overlap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub name: String,
    /// A video track that is off is not shown; an audio track that is off is muted.
    pub enabled: bool,
    /// A locked track refuses every edit.
    pub locked: bool,
    /// Audio only: when any track is soloed, only soloed tracks are heard.
    pub solo: bool,
    pub(crate) items: OrdMap<Time, Clip>,
}

impl Track {
    pub(crate) fn new(id: TrackId, kind: TrackKind, name: String) -> Track {
        Track {
            id,
            kind,
            name,
            enabled: true,
            locked: false,
            solo: false,
            items: OrdMap::new(),
        }
    }

    /// The clips in time order.
    pub fn clips(&self) -> impl DoubleEndedIterator<Item = &Clip> + '_ {
        self.items.values()
    }

    /// Number of clips.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when the track has no clips.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The clip with this id, if it is on this track.
    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.items.values().find(|c| c.id == id)
    }

    /// The clip that is playing at time `t`, if any.
    pub fn clip_at(&self, t: Time) -> Option<&Clip> {
        self.items
            .range(..=t)
            .next_back()
            .map(|(_, c)| c)
            .filter(|c| c.end() > t)
    }

    /// Every clip that shares some time with `range`, in order.
    pub fn clips_in(&self, range: TimeRange) -> Vec<&Clip> {
        let mut found = Vec::new();
        if range.is_empty() {
            return found;
        }
        if let Some((_, c)) = self.items.range(..range.start).next_back() {
            if c.end() > range.start {
                found.push(c);
            }
        }
        found.extend(self.items.range(range.start..range.end).map(|(_, c)| c));
        found
    }

    /// The last clip that starts before `t`.
    pub fn clip_before(&self, t: Time) -> Option<&Clip> {
        self.items.range(..t).next_back().map(|(_, c)| c)
    }

    /// The first clip that starts at or after `t`.
    pub fn clip_from(&self, t: Time) -> Option<&Clip> {
        self.items.range(t..).next().map(|(_, c)| c)
    }

    /// Where the last clip ends, or zero for an empty track.
    pub fn end(&self) -> Time {
        self.items
            .values()
            .next_back()
            .map_or(Time::ZERO, |c| c.end())
    }
}

/// A coloured note on the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub id: MarkerId,
    pub time: Time,
    /// Zero for a point marker.
    pub duration: Time,
    pub name: String,
    pub note: String,
    pub colour: MarkerColour,
}

/// The colour of a marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerColour {
    Red,
    Orange,
    Yellow,
    Green,
    Cyan,
    Blue,
    Purple,
    Pink,
    White,
}

impl MarkerColour {
    /// The name used in project files and on the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            MarkerColour::Red => "red",
            MarkerColour::Orange => "orange",
            MarkerColour::Yellow => "yellow",
            MarkerColour::Green => "green",
            MarkerColour::Cyan => "cyan",
            MarkerColour::Blue => "blue",
            MarkerColour::Purple => "purple",
            MarkerColour::Pink => "pink",
            MarkerColour::White => "white",
        }
    }

    /// The colour called `name`.
    pub fn parse(name: &str) -> Option<MarkerColour> {
        Some(match name {
            "red" => MarkerColour::Red,
            "orange" => MarkerColour::Orange,
            "yellow" => MarkerColour::Yellow,
            "green" => MarkerColour::Green,
            "cyan" => MarkerColour::Cyan,
            "blue" => MarkerColour::Blue,
            "purple" => MarkerColour::Purple,
            "pink" => MarkerColour::Pink,
            "white" => MarkerColour::White,
            _ => return None,
        })
    }
}

/// The settings a sequence is created with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceSettings {
    pub name: String,
    pub frame_rate: FrameRate,
    pub width: u32,
    pub height: u32,
    pub sample_rate: u32,
    /// Show drop-frame timecode (only meaningful at 29.97 and 59.94).
    pub drop_frame: bool,
    /// The timecode shown at the very start, usually zero or one hour.
    pub timecode_start: Time,
    /// How many video tracks to start with.
    pub video_tracks: u32,
    /// How many audio tracks to start with.
    pub audio_tracks: u32,
}

impl Default for SequenceSettings {
    fn default() -> Self {
        SequenceSettings {
            name: "Sequence 1".to_string(),
            frame_rate: FrameRate::FPS_24,
            width: 1920,
            height: 1080,
            sample_rate: 48_000,
            drop_frame: false,
            timecode_start: Time::ZERO,
            video_tracks: 2,
            audio_tracks: 2,
        }
    }
}

/// One timeline: its settings, its video and audio tracks and its markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sequence {
    pub id: SequenceId,
    pub name: String,
    pub frame_rate: FrameRate,
    pub width: u32,
    pub height: u32,
    pub sample_rate: u32,
    pub drop_frame: bool,
    pub timecode_start: Time,
    pub(crate) video: Vector<Track>,
    pub(crate) audio: Vector<Track>,
    pub(crate) markers: Vector<Marker>,
}

impl Sequence {
    pub(crate) fn new(id: SequenceId, settings: SequenceSettings) -> Sequence {
        Sequence {
            id,
            name: settings.name,
            frame_rate: settings.frame_rate,
            width: settings.width,
            height: settings.height,
            sample_rate: settings.sample_rate,
            drop_frame: settings.drop_frame,
            timecode_start: settings.timecode_start,
            video: Vector::new(),
            audio: Vector::new(),
            markers: Vector::new(),
        }
    }

    /// The video tracks, bottom first (V1 is index 0 and is drawn underneath the rest).
    pub fn video_tracks(&self) -> &Vector<Track> {
        &self.video
    }

    /// The audio tracks (A1 is index 0).
    pub fn audio_tracks(&self) -> &Vector<Track> {
        &self.audio
    }

    /// The tracks of one kind.
    pub fn tracks_of(&self, kind: TrackKind) -> &Vector<Track> {
        match kind {
            TrackKind::Video => &self.video,
            TrackKind::Audio => &self.audio,
        }
    }

    pub(crate) fn tracks_of_mut(&mut self, kind: TrackKind) -> &mut Vector<Track> {
        match kind {
            TrackKind::Video => &mut self.video,
            TrackKind::Audio => &mut self.audio,
        }
    }

    /// Every track, video first.
    pub fn tracks(&self) -> impl Iterator<Item = &Track> + '_ {
        self.video.iter().chain(self.audio.iter())
    }

    /// The track with this id.
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks().find(|t| t.id == id)
    }

    /// Where a track sits: its kind and its index within that kind.
    pub fn track_position(&self, id: TrackId) -> Option<(TrackKind, usize)> {
        for kind in [TrackKind::Video, TrackKind::Audio] {
            if let Some(i) = self.tracks_of(kind).iter().position(|t| t.id == id) {
                return Some((kind, i));
            }
        }
        None
    }

    pub(crate) fn track_mut(&mut self, id: TrackId) -> Result<&mut Track> {
        let (kind, index) = self.track_position(id).ok_or(Error::NoSuchTrack(id))?;
        self.tracks_of_mut(kind)
            .get_mut(index)
            .ok_or(Error::NoSuchTrack(id))
    }

    /// Every clip on every track.
    pub fn clips(&self) -> impl Iterator<Item = (&Track, &Clip)> + '_ {
        self.tracks().flat_map(|t| t.clips().map(move |c| (t, c)))
    }

    /// The clip with this id and the track it is on.
    pub fn find_clip(&self, id: ClipId) -> Option<(&Track, &Clip)> {
        self.clips().find(|(_, c)| c.id == id)
    }

    /// The markers, in time order.
    pub fn markers(&self) -> &Vector<Marker> {
        &self.markers
    }

    /// Where the last clip on any track ends.
    pub fn duration(&self) -> Time {
        self.tracks().map(Track::end).max().unwrap_or(Time::ZERO)
    }

    /// The length of one frame of this sequence.
    pub fn frame_duration(&self) -> Time {
        self.frame_rate.frame_duration()
    }

    /// Move a time to the nearest frame boundary of this sequence.
    pub fn snap(&self, t: Time) -> Time {
        t.snap(self.frame_rate)
    }

    /// The timecode shown at timeline time `t`, taking the sequence's start timecode into account.
    pub fn timecode_at(&self, t: Time) -> Option<Timecode> {
        let drop = self.drop_frame && self.frame_rate.supports_drop_frame();
        Timecode::from_time(t + self.timecode_start, self.frame_rate, drop)
    }

    /// The times where one clip ends or another begins on any track, in order, without repeats.
    /// Includes zero and the end of the sequence. Useful for "jump to next edit" and snapping.
    pub fn edit_points(&self) -> Vec<Time> {
        let mut points: Vec<Time> = vec![Time::ZERO];
        for (_, c) in self.clips() {
            points.push(c.start);
            points.push(c.end());
        }
        points.sort();
        points.dedup();
        points
    }

    pub(crate) fn validate(&self, assets: &OrdMap<AssetId, Asset>) -> Result<()> {
        let bad = |msg: String| {
            Err(Error::Invalid(format!(
                "{} ({}): {msg}",
                self.name, self.id
            )))
        };
        if self.width == 0 || self.height == 0 || self.sample_rate == 0 {
            return bad("size and sample rate must be positive".into());
        }
        let frame = self.frame_duration();
        let mut track_ids = HashSet::new();
        for kind in [TrackKind::Video, TrackKind::Audio] {
            for track in self.tracks_of(kind) {
                if track.kind != kind {
                    return bad(format!("{} is on the wrong list", track.id));
                }
                if !track_ids.insert(track.id) {
                    return bad(format!("{} appears twice", track.id));
                }
                let mut previous: Option<&Clip> = None;
                for (key, clip) in track.items.iter() {
                    if *key != clip.start {
                        return bad(format!("{} is filed under the wrong start", clip.id));
                    }
                    if clip.start < Time::ZERO {
                        return bad(format!("{} starts before zero", clip.id));
                    }
                    if clip.duration < frame {
                        return bad(format!("{} is shorter than one frame", clip.id));
                    }
                    if !clip.start.is_on_frame(self.frame_rate)
                        || !clip.duration.is_on_frame(self.frame_rate)
                    {
                        return bad(format!("{} is not on the frame grid", clip.id));
                    }
                    if clip.source_in < Time::ZERO {
                        return bad(format!("{} starts before its media", clip.id));
                    }
                    let Some(asset) = assets.get(&clip.asset) else {
                        return bad(format!("{} uses missing {}", clip.id, clip.asset));
                    };
                    let streams = match kind {
                        TrackKind::Video => asset.video.len(),
                        TrackKind::Audio => asset.audio.len(),
                    };
                    if clip.stream as usize >= streams {
                        return bad(format!(
                            "{} uses {} stream {} of {}, which has {streams}",
                            clip.id,
                            kind.word(),
                            clip.stream,
                            asset.id
                        ));
                    }
                    if let Some(limit) = asset.duration {
                        if clip.source_in + clip.duration > limit {
                            return bad(format!("{} runs past the end of its media", clip.id));
                        }
                    }
                    if let Some(prev) = previous {
                        if prev.end() > clip.start {
                            return bad(format!("{} overlaps {}", prev.id, clip.id));
                        }
                    }
                    previous = Some(clip);
                }
            }
        }
        let mut marker_ids = HashSet::new();
        for m in &self.markers {
            if !marker_ids.insert(m.id) {
                return bad(format!("{} appears twice", m.id));
            }
        }
        Ok(())
    }
}

/// A whole project: the media it knows about and its sequences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub(crate) assets: OrdMap<AssetId, Asset>,
    pub(crate) sequences: OrdMap<SequenceId, Sequence>,
    pub(crate) next_id: u64,
}

impl Project {
    /// An empty project.
    pub fn new(name: impl Into<String>) -> Project {
        Project {
            name: name.into(),
            assets: OrdMap::new(),
            sequences: OrdMap::new(),
            next_id: 1,
        }
    }

    pub(crate) fn from_parts(
        name: String,
        assets: OrdMap<AssetId, Asset>,
        sequences: OrdMap<SequenceId, Sequence>,
        next_id: u64,
    ) -> Project {
        Project {
            name,
            assets,
            sequences,
            next_id,
        }
    }

    /// The next id number that will be handed out (saved in the project file).
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    /// All assets, by id.
    pub fn assets(&self) -> &OrdMap<AssetId, Asset> {
        &self.assets
    }

    /// One asset.
    pub fn asset(&self, id: AssetId) -> Option<&Asset> {
        self.assets.get(&id)
    }

    /// All sequences, by id.
    pub fn sequences(&self) -> &OrdMap<SequenceId, Sequence> {
        &self.sequences
    }

    /// One sequence.
    pub fn sequence(&self, id: SequenceId) -> Option<&Sequence> {
        self.sequences.get(&id)
    }

    /// Check every rule a valid project follows. Edits through [`crate::Editor`] can never
    /// produce an invalid project; this is for projects loaded from disk and for tests.
    ///
    /// The rules: clips never overlap, sit on the frame grid, last at least a frame and stay
    /// inside their media; every clip points at a real asset and stream; ids are unique
    /// and below the project's next id.
    pub fn validate(&self) -> Result<()> {
        let mut used: HashSet<u64> = HashSet::new();
        let mut take = |raw: u64, what: String| -> Result<()> {
            if raw == 0 || raw >= self.next_id {
                return Err(Error::Invalid(format!(
                    "{what} has an id outside the project's range"
                )));
            }
            if !used.insert(raw) {
                return Err(Error::Invalid(format!("{what} reuses an id")));
            }
            Ok(())
        };
        for asset in self.assets.values() {
            take(asset.id.0, asset.id.to_string())?;
        }
        for seq in self.sequences.values() {
            take(seq.id.0, seq.id.to_string())?;
            seq.validate(&self.assets)?;
            for track in seq.tracks() {
                take(track.id.0, track.id.to_string())?;
                for clip in track.clips() {
                    take(clip.id.0, clip.id.to_string())?;
                }
            }
            for marker in seq.markers() {
                take(marker.id.0, marker.id.to_string())?;
            }
        }
        Ok(())
    }
}
