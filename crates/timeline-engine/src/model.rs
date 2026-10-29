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
use std::sync::Arc;

use im::{OrdMap, Vector};

use crate::animation::{Animation, Param};
use crate::audio::{Fade, Gain, Pan};
use crate::compositing::{Affine, BlendMode, Look, Mask, MaskSpec, Transform};
use crate::effects::{Effect, MAX_EFFECTS};
use crate::error::{Error, Result};
use crate::ids::{AssetId, ClipId, LinkId, MarkerId, SequenceId, TrackId};
use crate::retime::{Speed, Transition, MAX_TRANSITION_SECS};
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

/// An effect that does something at one moment, with its settings worked out.
#[derive(Debug, Clone, PartialEq)]
pub enum LiveEffect {
    Builtin(effects_engine::Kind, Vec<f64>),
    /// An effect from a plugin; the renderer finds the plugin by `spec.id`.
    Plugin(Arc<crate::effects::PluginSpec>, Vec<f64>),
}

/// A media file known to the project.
///
/// The project stores the facts it needs to edit (length, sizes, rates), not the
/// pixels. Frames and samples come from the Media Engine when they are wanted.
#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub id: AssetId,
    pub name: String,
    pub path: PathBuf,
    pub duration: Option<Time>,
    pub video: Vec<VideoProps>,
    pub audio: Vec<AudioProps>,
    /// Set for a graphic that is drawn rather than read from a file (then `path` is empty).
    pub generator: Option<crate::graphics::Generator>,
}

impl Asset {
    /// True for a drawn graphic (a shape, text, a colour or a gradient).
    pub fn is_generated(&self) -> bool {
        self.generator.is_some()
    }

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
    /// How fast the media plays. `duration` is always the time on the timeline, so at 2x a
    /// clip of 5 seconds shows 10 seconds of media.
    pub speed: Speed,
    /// Play the media backwards. The clip still shows the same stretch of media,
    /// `source_in` up to `source_in + source_len()`, just from its last moment to its first.
    pub reverse: bool,
    /// A transition from the clip before into this one. The two must touch.
    pub transition_in: Option<Transition>,
    /// Audio clips only: level change, pan and fades. Neutral on video clips.
    pub gain: Gain,
    pub pan: Pan,
    pub fade_in: Option<Fade>,
    pub fade_out: Option<Fade>,
    /// Keyframed values, with times measured from the start of the clip. An animated value
    /// replaces its fixed field (`speed`, `gain`, `pan`).
    pub anim: Animation,
    /// Picture clips only: the effect stack, applied in order (first is applied first).
    pub effects: Vec<Effect>,
    /// Picture clips only: opacity, position, scale, rotation and crop (fixed values; animated
    /// ones come from `anim`).
    pub transform: Transform,
    /// Picture clips only: how the clip's colour combines with the layers beneath.
    pub blend: BlendMode,
    /// Picture clips only: an optional mask.
    pub mask: Option<Mask>,
}

/// Media is allowed to overshoot its length by this much (a few ticks), so that rounding in
/// speed changes never rejects a clip that ends exactly at the end of its file.
pub(crate) const SOURCE_SLACK: Time = Time::from_ticks(4);

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

    /// How much of the asset the clip shows: its length scaled by its speed.
    pub fn source_len(&self) -> Time {
        self.media_len(self.duration)
    }

    /// How much media plays in the first `off` of the clip: `off` scaled by the speed, or, for
    /// a clip with a speed ramp, the exact area under the speed curve. Outside the clip the
    /// first and last speeds simply carry on.
    pub fn media_len(&self, off: Time) -> Time {
        match self.anim.get(Param::Speed) {
            None => self.speed.media_len(off),
            Some(curve) => Time::from_ticks(curve.integral(off.ticks()).round() as i64),
        }
    }

    /// The settings of `effect` at timeline time `t`.
    pub fn effect_values_at(&self, effect: &Effect, t: Time) -> Vec<f64> {
        effect.values_at((t - self.start).ticks())
    }

    /// The effects that change the picture, with their settings at timeline time `t`, in order.
    pub fn live_effects_at(&self, t: Time) -> Vec<LiveEffect> {
        self.effects
            .iter()
            .filter(|e| e.is_live())
            .map(|e| {
                let v = self.effect_values_at(e, t);
                match &e.plugin {
                    Some(p) => LiveEffect::Plugin(Arc::clone(p), v),
                    None => LiveEffect::Builtin(e.kind, v),
                }
            })
            .filter(|e| match e {
                LiveEffect::Builtin(k, v) => !effects_engine::is_neutral(*k, v),
                LiveEffect::Plugin(..) => true,
            })
            .collect()
    }

    /// The fixed value of any animatable parameter (used when it has no curve).
    pub fn fixed_value(&self, p: Param) -> f64 {
        match p {
            Param::Gain => f64::from(self.gain.db()),
            Param::Pan => f64::from(self.pan.percent()),
            Param::Speed => self.speed.as_f64(),
            _ => self
                .transform
                .get(p)
                .or_else(|| self.mask.as_ref().and_then(|m| m.get(p)))
                .unwrap_or(0.0),
        }
    }

    /// The value of a parameter at timeline time `t`: its curve, or its fixed value.
    pub fn value_at(&self, p: Param, t: Time) -> f64 {
        self.anim
            .get(p)
            .and_then(|c| c.eval((t - self.start).ticks()))
            .unwrap_or_else(|| self.fixed_value(p))
    }

    /// How the clip is laid over what is beneath at timeline time `t`.
    pub fn look_at(&self, t: Time) -> Look {
        let v = |p| self.value_at(p, t);
        Look {
            opacity: v(Param::Opacity) / 100.0,
            affine: Affine {
                x_pct: v(Param::PositionX),
                y_pct: v(Param::PositionY),
                scale_x_pct: v(Param::ScaleX),
                scale_y_pct: v(Param::ScaleY),
                rotation_deg: v(Param::Rotation),
            },
            crop: [
                v(Param::CropLeft),
                v(Param::CropRight),
                v(Param::CropTop),
                v(Param::CropBottom),
            ],
            mask: self.mask.map(|m| MaskSpec {
                shape: m.shape,
                invert: m.invert,
                x_pct: v(Param::MaskX),
                y_pct: v(Param::MaskY),
                width_pct: v(Param::MaskWidth),
                height_pct: v(Param::MaskHeight),
                feather_pct: v(Param::MaskFeather),
            }),
            blend: self.blend,
        }
    }

    /// True if the clip has no look settings at all (the common case).
    pub fn has_default_look(&self) -> bool {
        self.transform.is_default()
            && self.blend == BlendMode::Normal
            && self.mask.is_none()
            && !self.anim.iter().any(|(p, _)| p.is_look())
    }

    /// True if the clip has a speed ramp.
    pub fn is_ramped(&self) -> bool {
        self.anim.is_animated(Param::Speed)
    }

    /// The speed (1 is normal) at timeline time `t`.
    pub fn speed_at(&self, t: Time) -> f64 {
        self.anim
            .get(Param::Speed)
            .and_then(|c| c.eval((t - self.start).ticks()))
            .unwrap_or_else(|| self.speed.as_f64())
    }

    /// The gain in decibels at timeline time `t`.
    pub fn gain_db_at(&self, t: Time) -> f64 {
        self.anim
            .get(Param::Gain)
            .and_then(|c| c.eval((t - self.start).ticks()))
            .unwrap_or_else(|| f64::from(self.gain.db()))
    }

    /// The pan in percent at timeline time `t`.
    pub fn pan_at(&self, t: Time) -> f64 {
        self.anim
            .get(Param::Pan)
            .and_then(|c| c.eval((t - self.start).ticks()))
            .unwrap_or_else(|| f64::from(self.pan.percent()))
    }

    /// The span of the asset the clip shows.
    pub fn source_range(&self) -> TimeRange {
        TimeRange {
            start: self.source_in,
            end: self.source_in + self.source_len(),
        }
    }

    /// The position in the asset at timeline time `t`, following the clip's speed and
    /// direction. Works for times outside the clip too (the picture the clip *would* show),
    /// which is what transitions use.
    pub fn source_time_at(&self, t: Time) -> Time {
        let off = t - self.start;
        if self.reverse {
            self.source_in + self.source_len() - self.media_len(off)
        } else {
            self.source_in + self.media_len(off)
        }
    }

    /// Where to read the picture for the frame that starts at `t` and lasts `frame`. A reversed
    /// clip shows the mirror image of each forward frame, so the frame is read from the start
    /// of the media stretch it covers.
    pub fn frame_source_time(&self, t: Time, frame: Time) -> Time {
        if self.reverse {
            self.source_time_at(t + frame)
        } else {
            self.source_time_at(t)
        }
    }

    /// The stretch of media played between timeline times `from` and `to` (either may lie
    /// outside the clip). Always returned low to high.
    pub fn source_span(&self, from: Time, to: Time) -> (Time, Time) {
        let (a, b) = (self.source_time_at(from), self.source_time_at(to));
        (a.min(b), a.max(b))
    }

    /// A copy that covers timeline `start..end`, with `source_in` set so every moment keeps
    /// showing the same media. `start..end` may be wider than the clip (it then shows media
    /// beyond what it showed before) or narrower. Nothing else changes.
    pub fn window(&self, start: Time, end: Time) -> Clip {
        let mut c = self.clone();
        c.start = start;
        c.duration = end - start;
        c.source_in = if self.reverse {
            self.source_in + self.source_len() - self.media_len(end - self.start)
        } else {
            self.source_in + self.media_len(start - self.start)
        };
        let (a, b) = ((start - self.start).ticks(), (end - self.start).ticks());
        c.anim = self.anim.slice(a, b);
        c.effects = self.effects.iter().map(|e| e.slice(a, b)).collect();
        c
    }

    /// True when `from..to` of timeline time, played by this clip, stays inside the media.
    pub(crate) fn media_covers(&self, from: Time, to: Time, asset: &Asset) -> bool {
        let (lo, hi) = self.source_span(from, to);
        lo >= -SOURCE_SLACK
            && asset
                .duration
                .is_none_or(|limit| hi <= limit + SOURCE_SLACK)
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
    /// Audio tracks: level change and pan applied to the sum of the track's clips.
    pub gain: Gain,
    pub pan: Pan,
    /// Audio tracks: keyframed gain and pan, with times measured from the start of the sequence.
    pub anim: Animation,
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
            gain: Gain::UNITY,
            pan: Pan::CENTRE,
            anim: Animation::default(),
            items: OrdMap::new(),
        }
    }

    /// The track's gain in decibels at sequence time `t`.
    pub fn gain_db_at(&self, t: Time) -> f64 {
        self.anim
            .get(Param::Gain)
            .and_then(|c| c.eval(t.ticks()))
            .unwrap_or_else(|| f64::from(self.gain.db()))
    }

    /// The track's pan in percent at sequence time `t`.
    pub fn pan_at(&self, t: Time) -> f64 {
        self.anim
            .get(Param::Pan)
            .and_then(|c| c.eval(t.ticks()))
            .unwrap_or_else(|| f64::from(self.pan.percent()))
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

    /// The transition under way at time `t`, if any: the outgoing clip, the incoming clip,
    /// the transition and the span of timeline it covers.
    pub fn transition_at(
        &self,
        t: Time,
        frame: Time,
    ) -> Option<(&Clip, &Clip, Transition, Time, Time)> {
        for b in [self.clip_at(t), self.clip_from(t)].into_iter().flatten() {
            let Some(tr) = b.transition_in else { continue };
            let (w0, w1) = tr.window(b.start, frame);
            if t < w0 || t >= w1 {
                continue;
            }
            if let Some(a) = self.clip_before(b.start).filter(|a| a.end() == b.start) {
                return Some((a, b, tr, w0, w1));
            }
        }
        None
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
    /// Level change applied to the whole mix, after every track.
    pub master_gain: Gain,
    /// Keyframed master gain, with times measured from the start of the sequence.
    pub master_anim: Animation,
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
            master_gain: Gain::UNITY,
            master_anim: Animation::default(),
            video: Vector::new(),
            audio: Vector::new(),
            markers: Vector::new(),
        }
    }

    /// The master gain in decibels at sequence time `t`.
    pub fn master_gain_db_at(&self, t: Time) -> f64 {
        self.master_anim
            .get(Param::Gain)
            .and_then(|c| c.eval(t.ticks()))
            .unwrap_or_else(|| f64::from(self.master_gain.db()))
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

    /// Why the transition into `clip` cannot work, or `None` if it is fine (or there is none).
    pub(crate) fn transition_problem(
        &self,
        track: &Track,
        clip: &Clip,
        assets: &OrdMap<AssetId, Asset>,
    ) -> Option<String> {
        let tr = clip.transition_in?;
        let frame = self.frame_duration();
        if track.kind == TrackKind::Audio && !tr.kind.works_on_audio() {
            return Some("audio tracks only take dissolve (crossfade)".into());
        }
        if tr.duration < frame || !tr.duration.is_on_frame(self.frame_rate) {
            return Some("must last a whole number of frames, at least one".into());
        }
        if tr.duration.as_secs_f64() > MAX_TRANSITION_SECS {
            return Some(format!("longer than {MAX_TRANSITION_SECS} seconds"));
        }
        let Some(prev) = track
            .clip_before(clip.start)
            .filter(|p| p.end() == clip.start)
        else {
            return Some("needs a clip touching it on the left".into());
        };
        let (before, after) = (tr.before_cut(frame), tr.after_cut(frame));
        if before > prev.duration || after > clip.duration {
            return Some("is longer than the clips it joins".into());
        }
        if let Some(next) = track
            .clip_from(clip.end())
            .filter(|n| n.start == clip.end())
        {
            if let Some(nt) = next.transition_in {
                if clip.start + after + nt.before_cut(frame) > next.start {
                    return Some("overlaps the transition at the other end of the clip".into());
                }
            }
        }
        let (Some(pa), Some(ca)) = (assets.get(&prev.asset), assets.get(&clip.asset)) else {
            return Some("a clip's media is missing".into());
        };
        if !prev.media_covers(prev.end(), prev.end() + after, pa) {
            return Some("the outgoing clip has no spare media after its end".into());
        }
        if !clip.media_covers(clip.start - before, clip.start, ca) {
            return Some("the incoming clip has no spare media before its start".into());
        }
        None
    }

    /// Shorten fades that no longer fit their clip (after a trim): the fade out first, then the fade in.
    pub(crate) fn fit_fades(&mut self) {
        for kind in [TrackKind::Audio] {
            for track in self.tracks_of_mut(kind).iter_mut() {
                let starts: Vec<Time> = track
                    .items
                    .values()
                    .filter(|c| {
                        let total = c.fade_in.map_or(Time::ZERO, |f| f.duration)
                            + c.fade_out.map_or(Time::ZERO, |f| f.duration);
                        total > c.duration
                    })
                    .map(|c| c.start)
                    .collect();
                for start in starts {
                    if let Some(c) = track.items.get_mut(&start) {
                        let d = c.duration;
                        if let Some(fi) = c.fade_in.as_mut() {
                            fi.duration = fi.duration.min(d);
                        }
                        let room = d - c.fade_in.map_or(Time::ZERO, |f| f.duration);
                        if let Some(fo) = c.fade_out.as_mut() {
                            fo.duration = fo.duration.min(room);
                        }
                    }
                }
            }
        }
    }

    /// Drop every transition that no longer works (after an edit moved a clip away, trimmed
    /// away its spare media, and so on).
    pub(crate) fn prune_transitions(&mut self, assets: &OrdMap<AssetId, Asset>) {
        let mut bad: Vec<(TrackKind, usize, Time)> = Vec::new();
        for kind in [TrackKind::Video, TrackKind::Audio] {
            for (i, track) in self.tracks_of(kind).iter().enumerate() {
                for clip in track.clips() {
                    if clip.transition_in.is_some()
                        && self.transition_problem(track, clip, assets).is_some()
                    {
                        bad.push((kind, i, clip.start));
                    }
                }
            }
        }
        for (kind, i, start) in bad {
            if let Some(track) = self.tracks_of_mut(kind).get_mut(i) {
                if let Some(clip) = track.items.get_mut(&start) {
                    clip.transition_in = None;
                }
            }
        }
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
                let allowed: &[Param] = match kind {
                    TrackKind::Video => &[],
                    TrackKind::Audio => &[Param::Gain, Param::Pan],
                };
                if let Some(p) = track.anim.problem(allowed, None) {
                    return bad(format!("{}: {p}", track.id));
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
                        if clip.source_in + clip.source_len() > limit + SOURCE_SLACK {
                            return bad(format!("{} runs past the end of its media", clip.id));
                        }
                    }
                    let fades = clip.fade_in.map_or(Time::ZERO, |f| f.duration)
                        + clip.fade_out.map_or(Time::ZERO, |f| f.duration);
                    if clip.fade_in.is_some_and(|f| f.duration < Time::ZERO)
                        || clip.fade_out.is_some_and(|f| f.duration < Time::ZERO)
                        || fades > clip.duration
                    {
                        return bad(format!("{} has fades longer than the clip", clip.id));
                    }
                    let allowed: &[Param] = match kind {
                        TrackKind::Video => &Param::PICTURE,
                        TrackKind::Audio => &Param::AUDIO,
                    };
                    if let Some(p) = clip.anim.problem(allowed, Some(clip.duration.ticks())) {
                        return bad(format!("{}: {p}", clip.id));
                    }
                    if kind == TrackKind::Audio && !clip.has_default_look() {
                        return bad(format!("{} is a sound clip with picture settings", clip.id));
                    }
                    for p in crate::compositing::Transform::PARAMS {
                        let v = clip.transform.get(p).unwrap_or(0.0);
                        if !p.contains(v) {
                            return bad(format!("{}: {p} {v} is out of range", clip.id));
                        }
                    }
                    if let Some(m) = &clip.mask {
                        for p in crate::compositing::Mask::PARAMS {
                            let v = m.get(p).unwrap_or(0.0);
                            if !p.contains(v) {
                                return bad(format!("{}: {p} {v} is out of range", clip.id));
                            }
                        }
                    } else if clip.anim.iter().any(|(p, _)| p.is_mask()) {
                        return bad(format!("{} animates a mask it does not have", clip.id));
                    }
                    if kind == TrackKind::Audio && !clip.effects.is_empty() {
                        return bad(format!("{} is a sound clip with picture effects", clip.id));
                    }
                    if clip.effects.len() > MAX_EFFECTS {
                        return bad(format!("{} has more than {MAX_EFFECTS} effects", clip.id));
                    }
                    let mut seen = HashSet::new();
                    for e in &clip.effects {
                        if !seen.insert(e.id) {
                            return bad(format!("{} has {} twice", clip.id, e.id));
                        }
                        if let Some(p) = e.problem(clip.duration.ticks()) {
                            return bad(format!("{} {} ({}): {p}", clip.id, e.id, e.label()));
                        }
                    }
                    if kind == TrackKind::Video
                        && (!clip.gain.is_unity()
                            || !clip.pan.is_centre()
                            || clip.fade_in.is_some()
                            || clip.fade_out.is_some())
                    {
                        return bad(format!("{} is a video clip with sound settings", clip.id));
                    }
                    if let Some(problem) = self.transition_problem(track, clip, assets) {
                        return bad(format!("transition into {}: {problem}", clip.id));
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
        if let Some(p) = self.master_anim.problem(&[Param::Gain], None) {
            return bad(format!("master: {p}"));
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
#[derive(Debug, Clone, PartialEq)]
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
