// SPDX-License-Identifier: GPL-3.0-or-later
//! The edit operations, written as plain functions on a sequence.
//!
//! These functions change the project they are given and may leave it half
//! changed when they fail. They are only ever called by [`crate::Editor`] on a
//! private copy of the project, which is thrown away on error, so every public
//! edit is all-or-nothing.

use std::collections::{BTreeSet, HashMap};

use im::OrdMap;

use crate::animation::{Animation, Interp, Param, Target};
use crate::audio::{ClipAudio, Gain, Pan};
use crate::effects::{Effect, EffectId, EffectKind, MAX_EFFECTS};
use crate::error::{Error, Result};
use crate::ids::{AssetId, ClipId, LinkId, SequenceId, TrackId};
use crate::model::{Asset, Clip, Project, Sequence, Track, TrackKind, SOURCE_SLACK};
use crate::retime::{Speed, SpeedMode, Transition};
use crate::time::{Time, TimeRange};

/// How a new clip is put onto a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceMode {
    /// Replace whatever is underneath. Clips it covers part of are trimmed or split.
    Overwrite,
    /// Make room: everything from the placement point onwards moves later, and a clip
    /// under the point is split. With `sync`, every unlocked track makes room, not just this one.
    Insert { sync: bool },
}

/// Whether an edit closes or opens space for the clips after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ripple {
    /// Leave everything else where it is.
    Off,
    /// Move later clips on the edited tracks only.
    Track,
    /// Move later clips on every unlocked track, so the whole sequence stays in sync.
    Sync,
}

/// Whether an edit on a clip also applies to the clips linked to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMode {
    /// Linked clips (such as a picture and its sound) are edited together.
    Follow,
    /// Only the named clips are edited.
    Ignore,
}

/// Which end of a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The start of the clip.
    Head,
    /// The end of the clip.
    Tail,
}

/// Describes a clip to put on a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipSpec {
    pub asset: AssetId,
    /// Which video or audio stream of the asset (0 is the first).
    pub stream: u32,
    /// Where on the timeline the clip starts (moved to the nearest frame).
    pub start: Time,
    /// Where in the asset the clip starts.
    pub source_in: Time,
    /// How long the clip is. `None` uses the rest of the asset; a still image needs one.
    pub duration: Option<Time>,
    /// The clip's name. `None` uses the asset's name.
    pub name: Option<String>,
    /// Playback speed. `duration` is timeline time, so at 2x a 5 s clip uses 10 s of media.
    pub speed: Speed,
    /// Play the media backwards.
    pub reverse: bool,
}

impl ClipSpec {
    /// The whole asset, from its start, placed at `start`.
    pub fn new(asset: AssetId, start: Time) -> ClipSpec {
        ClipSpec {
            asset,
            stream: 0,
            start,
            source_in: Time::ZERO,
            duration: None,
            name: None,
            speed: Speed::NORMAL,
            reverse: false,
        }
    }
}

/// The clips made by placing one asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub video: Option<ClipId>,
    pub audio: Option<ClipId>,
}

/// Changes to a track's settings. Fields left as `None` stay as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackUpdate {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub locked: Option<bool>,
    pub solo: Option<bool>,
    /// Audio tracks: level and pan of the whole track.
    pub gain: Option<Gain>,
    pub pan: Option<Pan>,
}

/// What the operations need besides the sequence: the assets and the id counter.
pub(crate) struct Ctx<'a> {
    pub assets: &'a OrdMap<AssetId, Asset>,
    pub next_id: &'a mut u64,
}

impl Ctx<'_> {
    fn next(&mut self) -> u64 {
        let id = *self.next_id;
        *self.next_id += 1;
        id
    }

    fn clip_id(&mut self) -> ClipId {
        ClipId(self.next())
    }

    fn link_id(&mut self) -> LinkId {
        LinkId(self.next())
    }
}

impl Project {
    /// Run `f` on one sequence together with the asset table and id counter.
    pub(crate) fn with_seq<R>(
        &mut self,
        id: SequenceId,
        f: impl FnOnce(&mut Sequence, &mut Ctx<'_>) -> Result<R>,
    ) -> Result<R> {
        let seq = self
            .sequences
            .get_mut(&id)
            .ok_or(Error::NoSuchSequence(id))?;
        let mut cx = Ctx {
            assets: &self.assets,
            next_id: &mut self.next_id,
        };
        f(seq, &mut cx)
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn ensure_unlocked(seq: &Sequence, id: TrackId) -> Result<()> {
    let track = seq.track(id).ok_or(Error::NoSuchTrack(id))?;
    if track.locked {
        Err(Error::TrackLocked(id))
    } else {
        Ok(())
    }
}

fn unlocked_tracks(seq: &Sequence) -> Vec<TrackId> {
    seq.tracks().filter(|t| !t.locked).map(|t| t.id).collect()
}

fn locate(seq: &Sequence, id: ClipId) -> Result<(TrackId, Clip)> {
    seq.find_clip(id)
        .map(|(t, c)| (t.id, c.clone()))
        .ok_or(Error::NoSuchClip(id))
}

/// The clips named, plus (when following links) every clip linked to one of them.
fn expand(seq: &Sequence, ids: &[ClipId], links: LinkMode) -> Result<Vec<ClipId>> {
    let mut set = BTreeSet::new();
    let mut groups = BTreeSet::new();
    for id in ids {
        let (_, clip) = seq.find_clip(*id).ok_or(Error::NoSuchClip(*id))?;
        set.insert(*id);
        if links == LinkMode::Follow {
            if let Some(l) = clip.link {
                groups.insert(l);
            }
        }
    }
    if !groups.is_empty() {
        for (_, c) in seq.clips() {
            if c.link.is_some_and(|l| groups.contains(&l)) {
                set.insert(c.id);
            }
        }
    }
    Ok(set.into_iter().collect())
}

fn put(seq: &mut Sequence, track: TrackId, clip: Clip) -> Result<()> {
    seq.track_mut(track)?.items.insert(clip.start, clip);
    Ok(())
}

fn take(seq: &mut Sequence, track: TrackId, start: Time) -> Result<Clip> {
    seq.track_mut(track)?
        .items
        .remove(&start)
        .ok_or_else(|| Error::Invalid(format!("{track} has no clip starting at {start}")))
}

/// Round an amount of time to a whole number of frames.
fn snap_delta(seq: &Sequence, delta: Time) -> Time {
    seq.snap(delta)
}

/// Move every clip that starts at or after `from` by `delta`. A move to the left fails
/// with [`Error::Blocked`] if it would push a clip into the one before.
fn shift_from(track: &mut Track, from: Time, delta: Time) -> Result<()> {
    if delta == Time::ZERO {
        return Ok(());
    }
    let moved: Vec<Clip> = track.items.range(from..).map(|(_, c)| c.clone()).collect();
    let Some(first) = moved.first() else {
        return Ok(());
    };
    if delta < Time::ZERO {
        let (floor, blocker) = match track.items.range(..from).next_back() {
            Some((_, prev)) => (prev.end(), Some(prev.id)),
            None => (Time::ZERO, None),
        };
        if first.start + delta < floor {
            return Err(match blocker {
                Some(clip) => Error::Blocked {
                    track: track.id,
                    clip,
                },
                None => Error::BeforeStart(first.start + delta),
            });
        }
    }
    for c in &moved {
        track.items.remove(&c.start);
    }
    for mut c in moved {
        c.start += delta;
        track.items.insert(c.start, c);
    }
    Ok(())
}

/// Empty `range` on a track: clips inside it are removed, clips crossing its edges are
/// trimmed, and a clip crossing both edges is split in two. The surviving left part keeps
/// the clip's id and link; a new right part gets a fresh id and no link.
fn clear_range(track: &mut Track, cx: &mut Ctx<'_>, range: TimeRange) {
    let hit: Vec<Clip> = track.clips_in(range).into_iter().cloned().collect();
    for c in hit {
        track.items.remove(&c.start);
        let mut kept_left = false;
        if c.start < range.start {
            let mut left = c.window(c.start, range.start);
            left.fade_out = None;
            track.items.insert(left.start, left);
            kept_left = true;
        }
        if c.end() > range.end {
            let mut right = c.window(range.end, c.end());
            right.transition_in = None;
            right.fade_in = None;
            if kept_left {
                right.id = cx.clip_id();
                right.link = None;
            }
            track.items.insert(right.start, right);
        }
    }
}

/// Split every clip that crosses `t` on the given tracks (only the ones in `only`, if given).
/// The left part keeps the clip's id and link; the right part gets a new id. Clips that were
/// linked stay linked within each side: the right parts share one new link per old link.
fn split_tracks_at(
    seq: &mut Sequence,
    cx: &mut Ctx<'_>,
    tracks: &[TrackId],
    t: Time,
    only: Option<&BTreeSet<ClipId>>,
) -> Result<Vec<(ClipId, ClipId)>> {
    let mut new_links: HashMap<LinkId, LinkId> = HashMap::new();
    let mut pairs = Vec::new();
    for &tid in tracks {
        ensure_unlocked(seq, tid)?;
        let Some(clip) = seq
            .track(tid)
            .and_then(|tr| tr.clip_at(t))
            .filter(|c| c.start < t)
            .filter(|c| only.is_none_or(|set| set.contains(&c.id)))
            .cloned()
        else {
            continue;
        };
        let mut left = clip.window(clip.start, t);
        left.fade_out = None;
        let mut right = clip.window(t, clip.end());
        right.id = cx.clip_id();
        right.transition_in = None;
        right.fade_in = None;
        right.link = clip
            .link
            .map(|l| *new_links.entry(l).or_insert_with(|| cx.link_id()));
        pairs.push((left.id, right.id));
        let track = seq.track_mut(tid)?;
        track.items.insert(left.start, left);
        track.items.insert(right.start, right);
    }
    Ok(pairs)
}

// ---------------------------------------------------------------------------
// Assets, sequences, tracks
// ---------------------------------------------------------------------------

impl Project {
    pub(crate) fn add_asset(&mut self, new: crate::model::NewAsset) -> AssetId {
        let id = AssetId(self.next_id);
        self.next_id += 1;
        self.assets.insert(
            id,
            Asset {
                id,
                name: new.name,
                path: new.path,
                duration: new.duration,
                video: new.video,
                audio: new.audio,
            },
        );
        id
    }

    pub(crate) fn remove_asset(&mut self, id: AssetId) -> Result<()> {
        if !self.assets.contains_key(&id) {
            return Err(Error::NoSuchAsset(id));
        }
        let used = self
            .sequences
            .values()
            .any(|s| s.clips().any(|(_, c)| c.asset == id));
        if used {
            return Err(Error::AssetInUse(id));
        }
        self.assets.remove(&id);
        Ok(())
    }

    pub(crate) fn relink_asset(&mut self, id: AssetId, path: std::path::PathBuf) -> Result<()> {
        let asset = self.assets.get_mut(&id).ok_or(Error::NoSuchAsset(id))?;
        asset.path = path;
        Ok(())
    }

    pub(crate) fn add_sequence(&mut self, settings: crate::model::SequenceSettings) -> SequenceId {
        let id = SequenceId(self.next_id);
        self.next_id += 1;
        let (video, audio) = (settings.video_tracks, settings.audio_tracks);
        let mut seq = Sequence::new(id, settings);
        for _ in 0..video {
            add_track_to(&mut seq, &mut self.next_id, TrackKind::Video, None);
        }
        for _ in 0..audio {
            add_track_to(&mut seq, &mut self.next_id, TrackKind::Audio, None);
        }
        self.sequences.insert(id, seq);
        id
    }

    pub(crate) fn remove_sequence(&mut self, id: SequenceId) -> Result<()> {
        self.sequences
            .remove(&id)
            .map(|_| ())
            .ok_or(Error::NoSuchSequence(id))
    }
}

fn add_track_to(
    seq: &mut Sequence,
    next_id: &mut u64,
    kind: TrackKind,
    name: Option<String>,
) -> TrackId {
    let id = TrackId(*next_id);
    *next_id += 1;
    let list = seq.tracks_of_mut(kind);
    let name = name.unwrap_or_else(|| {
        let prefix = if kind == TrackKind::Video { "V" } else { "A" };
        format!("{prefix}{}", list.len() + 1)
    });
    list.push_back(Track::new(id, kind, name));
    id
}

pub(crate) fn add_track(
    seq: &mut Sequence,
    cx: &mut Ctx<'_>,
    kind: TrackKind,
    name: Option<String>,
) -> TrackId {
    add_track_to(seq, cx.next_id, kind, name)
}

pub(crate) fn remove_track(seq: &mut Sequence, id: TrackId) -> Result<()> {
    ensure_unlocked(seq, id)?;
    let (kind, index) = seq.track_position(id).ok_or(Error::NoSuchTrack(id))?;
    seq.tracks_of_mut(kind).remove(index);
    Ok(())
}

pub(crate) fn update_track(seq: &mut Sequence, id: TrackId, update: TrackUpdate) -> Result<()> {
    // Changing a track's own settings is allowed while it is locked: that is how it gets unlocked.
    let track = seq.track_mut(id)?;
    if let Some(name) = update.name {
        track.name = name;
    }
    if let Some(v) = update.enabled {
        track.enabled = v;
    }
    if let Some(v) = update.locked {
        track.locked = v;
    }
    if let Some(v) = update.solo {
        track.solo = v;
    }
    if update.gain.is_some() || update.pan.is_some() {
        if track.kind != TrackKind::Audio {
            return Err(Error::Invalid(format!("{id} is not an audio track")));
        }
        if update.gain.is_some() && track.anim.is_animated(Param::Gain) {
            return Err(Error::Animated {
                what: format!("the gain of {id}"),
            });
        }
        if update.pan.is_some() && track.anim.is_animated(Param::Pan) {
            return Err(Error::Animated {
                what: format!("the pan of {id}"),
            });
        }
        if let Some(v) = update.gain {
            track.gain = v;
        }
        if let Some(v) = update.pan {
            track.pan = v;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Placing clips
// ---------------------------------------------------------------------------

/// The whole asset from `source_in` onwards, rounded down to whole frames.
fn default_duration(seq: &Sequence, asset: &Asset, source_in: Time, speed: Speed) -> Result<Time> {
    let limit = asset.duration.ok_or(Error::NeedsDuration)?;
    let frames = speed
        .timeline_len(limit - source_in)
        .to_frames_floor(seq.frame_rate);
    Ok(Time::from_frames(frames.max(0), seq.frame_rate))
}

pub(crate) fn place_clip(
    seq: &mut Sequence,
    cx: &mut Ctx<'_>,
    track_id: TrackId,
    spec: ClipSpec,
    mode: PlaceMode,
    link: Option<LinkId>,
) -> Result<ClipId> {
    let (kind, _) = seq
        .track_position(track_id)
        .ok_or(Error::NoSuchTrack(track_id))?;
    ensure_unlocked(seq, track_id)?;
    let asset = cx
        .assets
        .get(&spec.asset)
        .ok_or(Error::NoSuchAsset(spec.asset))?;
    let streams = match kind {
        TrackKind::Video => asset.video.len(),
        TrackKind::Audio => asset.audio.len(),
    };
    if spec.stream as usize >= streams {
        return Err(Error::WrongTrackKind {
            asset: asset.id,
            wanted: kind.word(),
            stream: spec.stream,
        });
    }

    let id = cx.clip_id();
    let start = seq.snap(spec.start);
    if start < Time::ZERO {
        return Err(Error::BeforeStart(start));
    }
    if spec.source_in < Time::ZERO {
        return Err(Error::OutsideSource {
            clip: id,
            detail: "starts before the media".into(),
        });
    }
    let duration = match spec.duration {
        Some(d) => seq.snap(d),
        None => default_duration(seq, asset, spec.source_in, spec.speed)?,
    };
    if duration < seq.frame_duration() {
        return Err(Error::TooShort);
    }
    if let Some(limit) = asset.duration {
        if spec.source_in + spec.speed.media_len(duration) > limit + SOURCE_SLACK {
            return Err(Error::OutsideSource {
                clip: id,
                detail: "runs past the end of the media".into(),
            });
        }
    }

    let clip = Clip {
        id,
        asset: asset.id,
        stream: spec.stream,
        name: spec.name.unwrap_or_else(|| asset.name.clone()),
        start,
        source_in: spec.source_in,
        duration,
        enabled: true,
        link,
        speed: spec.speed,
        reverse: spec.reverse,
        transition_in: None,
        gain: Gain::UNITY,
        pan: Pan::CENTRE,
        fade_in: None,
        fade_out: None,
        anim: Animation::default(),
        effects: Vec::new(),
    };

    match mode {
        PlaceMode::Overwrite => {
            let range = clip.range();
            clear_range(seq.track_mut(track_id)?, cx, range);
        }
        PlaceMode::Insert { sync } => {
            let tracks = if sync {
                unlocked_tracks(seq)
            } else {
                vec![track_id]
            };
            split_tracks_at(seq, cx, &tracks, start, None)?;
            for tid in tracks {
                shift_from(seq.track_mut(tid)?, start, duration)?;
            }
        }
    }
    put(seq, track_id, clip)?;
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn place_asset(
    seq: &mut Sequence,
    cx: &mut Ctx<'_>,
    asset_id: AssetId,
    start: Time,
    source_in: Time,
    duration: Option<Time>,
    video_track: Option<TrackId>,
    audio_track: Option<TrackId>,
    mode: PlaceMode,
) -> Result<Placed> {
    if video_track.is_none() && audio_track.is_none() {
        return Err(Error::NothingTo("place: no track given"));
    }
    let asset = cx
        .assets
        .get(&asset_id)
        .ok_or(Error::NoSuchAsset(asset_id))?;
    // One length for both halves, so a picture and its sound match exactly.
    let duration = match duration {
        Some(d) => Some(seq.snap(d)),
        None if asset.duration.is_some() => {
            Some(default_duration(seq, asset, source_in, Speed::NORMAL)?)
        }
        None => None,
    };
    let link = (video_track.is_some() && audio_track.is_some()).then(|| cx.link_id());
    let spec = ClipSpec {
        asset: asset_id,
        stream: 0,
        start,
        source_in,
        duration,
        name: None,
        speed: Speed::NORMAL,
        reverse: false,
    };
    let mut placed = Placed {
        video: None,
        audio: None,
    };
    if let Some(track) = video_track {
        placed.video = Some(place_clip(seq, cx, track, spec.clone(), mode, link)?);
    }
    if let Some(track) = audio_track {
        // With a synced insert the video half has already made the room.
        let audio_mode = match mode {
            PlaceMode::Insert { sync: true } if video_track.is_some() => PlaceMode::Overwrite,
            other => other,
        };
        placed.audio = Some(place_clip(seq, cx, track, spec, audio_mode, link)?);
    }
    Ok(placed)
}

// ---------------------------------------------------------------------------
// Splitting
// ---------------------------------------------------------------------------

pub(crate) fn split_clip(
    seq: &mut Sequence,
    cx: &mut Ctx<'_>,
    clip: ClipId,
    time: Time,
    links: LinkMode,
) -> Result<(ClipId, ClipId)> {
    let time = seq.snap(time);
    let group = expand(seq, &[clip], links)?;
    let (_, primary) = locate(seq, clip)?;
    if !(time > primary.start && time < primary.end()) {
        return Err(Error::NotInsideClip { clip, time });
    }
    let mut tracks = Vec::new();
    for id in &group {
        let (tid, _) = locate(seq, *id)?;
        ensure_unlocked(seq, tid)?;
        tracks.push(tid);
    }
    let only: BTreeSet<ClipId> = group.into_iter().collect();
    let pairs = split_tracks_at(seq, cx, &tracks, time, Some(&only))?;
    pairs
        .into_iter()
        .find(|(left, _)| *left == clip)
        .ok_or(Error::NotInsideClip { clip, time })
}

/// Split every clip that crosses `time` on the given tracks (all unlocked tracks if `None`).
pub(crate) fn split_all(
    seq: &mut Sequence,
    cx: &mut Ctx<'_>,
    time: Time,
    tracks: Option<&[TrackId]>,
) -> Result<Vec<(ClipId, ClipId)>> {
    let time = seq.snap(time);
    let tracks: Vec<TrackId> = match tracks {
        Some(list) => list.to_vec(),
        None => unlocked_tracks(seq),
    };
    split_tracks_at(seq, cx, &tracks, time, None)
}

// ---------------------------------------------------------------------------
// Deleting
// ---------------------------------------------------------------------------

pub(crate) fn delete_clips(
    seq: &mut Sequence,
    ids: &[ClipId],
    ripple: Ripple,
    links: LinkMode,
) -> Result<()> {
    let group = expand(seq, ids, links)?;
    let mut removed: Vec<(TrackId, TimeRange)> = Vec::new();
    for id in &group {
        let (tid, clip) = locate(seq, *id)?;
        ensure_unlocked(seq, tid)?;
        removed.push((tid, clip.range()));
    }
    for id in &group {
        let (tid, clip) = locate(seq, *id)?;
        take(seq, tid, clip.start)?;
    }
    match ripple {
        Ripple::Off => {}
        Ripple::Track => {
            // Close each gap on the track it was made on, last gap first so earlier ones stay put.
            removed.sort_by_key(|(_, r)| std::cmp::Reverse(r.start));
            for (tid, range) in removed {
                shift_from(seq.track_mut(tid)?, range.end, -range.duration())?;
            }
        }
        Ripple::Sync => {
            let mut ranges: Vec<TimeRange> = removed.iter().map(|(_, r)| *r).collect();
            ranges.sort_by_key(|r| r.start);
            let mut merged: Vec<TimeRange> = Vec::new();
            for r in ranges {
                match merged.last_mut() {
                    Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                    _ => merged.push(r),
                }
            }
            for range in merged.into_iter().rev() {
                ripple_close(seq, range, &unlocked_tracks(seq))?;
            }
        }
    }
    Ok(())
}

/// Close the gap `range` on the given tracks. Fails with [`Error::Blocked`] if a track
/// still has a clip inside the range.
fn ripple_close(seq: &mut Sequence, range: TimeRange, tracks: &[TrackId]) -> Result<()> {
    for &tid in tracks {
        let track = seq.track(tid).ok_or(Error::NoSuchTrack(tid))?;
        if let Some(c) = track.clips_in(range).first() {
            return Err(Error::Blocked {
                track: tid,
                clip: c.id,
            });
        }
    }
    for &tid in tracks {
        shift_from(seq.track_mut(tid)?, range.end, -range.duration())?;
    }
    Ok(())
}

/// Remove the time `range` from the given tracks (all unlocked tracks if `None`) and close
/// the gap: clips inside are deleted, clips crossing the edges are cut, and everything
/// after moves earlier. This is "ripple delete" and "extract" on a time range.
pub(crate) fn ripple_delete_range(
    seq: &mut Sequence,
    cx: &mut Ctx<'_>,
    range: TimeRange,
    tracks: Option<&[TrackId]>,
) -> Result<()> {
    let start = seq.snap(range.start);
    let end = seq.snap(range.end);
    if end <= start {
        return Ok(());
    }
    let tracks: Vec<TrackId> = match tracks {
        Some(list) => list.to_vec(),
        None => unlocked_tracks(seq),
    };
    for &t in &tracks {
        ensure_unlocked(seq, t)?;
    }
    let range = TimeRange { start, end };
    for &tid in &tracks {
        // Cut at both edges first so linked clips keep their links on each side.
        split_tracks_at(seq, cx, &[tid], start, None)?;
        split_tracks_at(seq, cx, &[tid], end, None)?;
        let inside: Vec<Time> = seq
            .track(tid)
            .map(|t| t.clips_in(range).iter().map(|c| c.start).collect())
            .unwrap_or_default();
        for s in inside {
            take(seq, tid, s)?;
        }
    }
    for &tid in &tracks {
        shift_from(seq.track_mut(tid)?, end, -range.duration())?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Moving
// ---------------------------------------------------------------------------

pub(crate) fn move_clips(
    seq: &mut Sequence,
    cx: &mut Ctx<'_>,
    ids: &[ClipId],
    delta: Time,
    track_delta: i32,
    links: LinkMode,
) -> Result<()> {
    let delta = snap_delta(seq, delta);
    let group = expand(seq, ids, links)?;

    struct Move {
        clip: Clip,
        from: TrackId,
        to: TrackId,
    }
    let mut moves = Vec::new();
    for id in &group {
        let (from, clip) = locate(seq, *id)?;
        ensure_unlocked(seq, from)?;
        let (kind, index) = seq.track_position(from).ok_or(Error::NoSuchTrack(from))?;
        let target = index as i64 + i64::from(track_delta);
        let to = usize::try_from(target)
            .ok()
            .and_then(|i| seq.tracks_of(kind).get(i))
            .map(|t| t.id)
            .ok_or(Error::NoTrackThere(from))?;
        ensure_unlocked(seq, to)?;
        if clip.start + delta < Time::ZERO {
            return Err(Error::BeforeStart(clip.start + delta));
        }
        moves.push(Move { clip, from, to });
    }
    if delta == Time::ZERO && track_delta == 0 {
        return Ok(());
    }
    // Lift every moved clip first, so they never collide with their own old places.
    for m in &moves {
        take(seq, m.from, m.clip.start)?;
    }
    for m in moves {
        let mut clip = m.clip;
        clip.start += delta;
        let range = clip.range();
        clear_range(seq.track_mut(m.to)?, cx, range);
        put(seq, m.to, clip)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Trimming
// ---------------------------------------------------------------------------

struct Change {
    track: TrackId,
    old: Clip,
    new: Clip,
}

fn check_source(assets: &OrdMap<AssetId, Asset>, frame: Time, new: &Clip) -> Result<()> {
    if new.duration < frame {
        return Err(Error::TooShort);
    }
    if new.start < Time::ZERO {
        return Err(Error::BeforeStart(new.start));
    }
    if new.source_in < -SOURCE_SLACK {
        return Err(Error::OutsideSource {
            clip: new.id,
            detail: "before the start of the media".into(),
        });
    }
    let asset = assets
        .get(&new.asset)
        .ok_or(Error::NoSuchAsset(new.asset))?;
    if let Some(limit) = asset.duration {
        if new.source_in + new.source_len() > limit + SOURCE_SLACK {
            return Err(Error::OutsideSource {
                clip: new.id,
                detail: "past the end of the media".into(),
            });
        }
    }
    Ok(())
}

/// Move one edge of a clip by `delta` (positive is later in time).
///
/// Without ripple the clip must not run into its neighbours. With ripple the clip's start
/// stays put and the clips after it move to follow the edge.
pub(crate) fn trim(
    seq: &mut Sequence,
    assets: &OrdMap<AssetId, Asset>,
    clip_id: ClipId,
    edge: Edge,
    delta: Time,
    ripple: Ripple,
    links: LinkMode,
) -> Result<()> {
    let delta = snap_delta(seq, delta);
    if delta == Time::ZERO {
        return Ok(());
    }
    let group = expand(seq, &[clip_id], links)?;
    let frame = seq.frame_duration();
    let rippling = ripple != Ripple::Off;

    let mut changes = Vec::new();
    for id in &group {
        let (track, old) = locate(seq, *id)?;
        ensure_unlocked(seq, track)?;
        let new = match (edge, rippling) {
            (Edge::Head, false) => old.window(old.start + delta, old.end()),
            (Edge::Head, true) => {
                let mut c = old.window(old.start + delta, old.end());
                c.start = old.start;
                c
            }
            (Edge::Tail, _) => old.window(old.start, old.end() + delta),
        };
        check_source(assets, frame, &new)?;
        if !rippling {
            let hit = seq
                .track(track)
                .and_then(|t| t.clips_in(new.range()).into_iter().find(|c| c.id != new.id))
                .map(|c| c.id);
            if let Some(other) = hit {
                return Err(Error::Collision {
                    clip: new.id,
                    other,
                });
            }
        }
        changes.push(Change { track, old, new });
    }

    for ch in &changes {
        take(seq, ch.track, ch.old.start)?;
        put(seq, ch.track, ch.new.clone())?;
    }

    if rippling {
        let shift = match edge {
            Edge::Head => -delta,
            Edge::Tail => delta,
        };
        let primary_end = changes
            .iter()
            .find(|c| c.old.id == clip_id)
            .map(|c| c.old.end())
            .unwrap_or(Time::ZERO);
        let edited: BTreeSet<TrackId> = changes.iter().map(|c| c.track).collect();
        for ch in &changes {
            shift_from(seq.track_mut(ch.track)?, ch.old.end(), shift)?;
        }
        if ripple == Ripple::Sync {
            for tid in unlocked_tracks(seq) {
                if !edited.contains(&tid) {
                    shift_from(seq.track_mut(tid)?, primary_end, shift)?;
                }
            }
        }
    }
    Ok(())
}

/// Move the cut between two neighbouring clips by `delta`, shortening one and lengthening the other.
pub(crate) fn roll(
    seq: &mut Sequence,
    assets: &OrdMap<AssetId, Asset>,
    left: ClipId,
    right: ClipId,
    delta: Time,
) -> Result<()> {
    let delta = snap_delta(seq, delta);
    let (lt, l) = locate(seq, left)?;
    let (rt, r) = locate(seq, right)?;
    ensure_unlocked(seq, lt)?;
    if lt != rt || l.end() != r.start {
        return Err(Error::NotAdjacent { left, right });
    }
    if delta == Time::ZERO {
        return Ok(());
    }
    let frame = seq.frame_duration();
    let new_l = l.window(l.start, l.end() + delta);
    let new_r = r.window(r.start + delta, r.end());
    check_source(assets, frame, &new_l)?;
    check_source(assets, frame, &new_r)?;
    take(seq, rt, r.start)?;
    put(seq, lt, new_l)?;
    put(seq, rt, new_r)?;
    Ok(())
}

/// Change which part of the media a clip shows without moving it or changing its length.
pub(crate) fn slip(
    seq: &mut Sequence,
    assets: &OrdMap<AssetId, Asset>,
    clip: ClipId,
    delta: Time,
    links: LinkMode,
) -> Result<()> {
    let delta = snap_delta(seq, delta);
    let group = expand(seq, &[clip], links)?;
    let frame = seq.frame_duration();
    let mut changes = Vec::new();
    for id in &group {
        let (track, old) = locate(seq, *id)?;
        ensure_unlocked(seq, track)?;
        let mut new = old.clone();
        new.source_in = old.source_in + delta;
        check_source(assets, frame, &new)?;
        changes.push((track, new));
    }
    for (track, new) in changes {
        put(seq, track, new)?;
    }
    Ok(())
}

/// Move a clip along the timeline while the clips on either side stretch and shrink to fill
/// the space. The clip needs a clip touching it on both sides, and its own length and
/// content do not change.
pub(crate) fn slide(
    seq: &mut Sequence,
    assets: &OrdMap<AssetId, Asset>,
    clip: ClipId,
    delta: Time,
) -> Result<()> {
    let delta = snap_delta(seq, delta);
    let (tid, c) = locate(seq, clip)?;
    ensure_unlocked(seq, tid)?;
    let track = seq.track(tid).ok_or(Error::NoSuchTrack(tid))?;
    let prev = track
        .clip_before(c.start)
        .filter(|p| p.end() == c.start)
        .cloned();
    let next = track
        .clip_from(c.end())
        .filter(|n| n.start == c.end())
        .cloned();
    let (Some(prev), Some(next)) = (prev, next) else {
        return Err(Error::NeedsNeighbours(clip));
    };
    if delta == Time::ZERO {
        return Ok(());
    }
    let frame = seq.frame_duration();
    let new_prev = prev.window(prev.start, prev.end() + delta);
    let mut new_c = c.clone();
    new_c.start = c.start + delta;
    let new_next = next.window(next.start + delta, next.end());
    for n in [&new_prev, &new_c, &new_next] {
        check_source(assets, frame, n)?;
    }
    take(seq, tid, c.start)?;
    take(seq, tid, next.start)?;
    put(seq, tid, new_prev)?;
    put(seq, tid, new_c)?;
    put(seq, tid, new_next)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Clip properties, links, markers
// ---------------------------------------------------------------------------

pub(crate) fn set_clips_enabled(
    seq: &mut Sequence,
    ids: &[ClipId],
    enabled: bool,
    links: LinkMode,
) -> Result<()> {
    for id in expand(seq, ids, links)? {
        let (tid, mut clip) = locate(seq, id)?;
        ensure_unlocked(seq, tid)?;
        clip.enabled = enabled;
        put(seq, tid, clip)?;
    }
    Ok(())
}

pub(crate) fn rename_clip(seq: &mut Sequence, id: ClipId, name: String) -> Result<()> {
    let (tid, mut clip) = locate(seq, id)?;
    ensure_unlocked(seq, tid)?;
    clip.name = name;
    put(seq, tid, clip)
}

/// Join the clips into one linked group. Needs at least two.
pub(crate) fn link_clips(seq: &mut Sequence, cx: &mut Ctx<'_>, ids: &[ClipId]) -> Result<LinkId> {
    let unique: BTreeSet<ClipId> = ids.iter().copied().collect();
    if unique.len() < 2 {
        return Err(Error::NothingTo("link: needs at least two clips"));
    }
    let link = cx.link_id();
    for id in unique {
        let (tid, mut clip) = locate(seq, id)?;
        ensure_unlocked(seq, tid)?;
        clip.link = Some(link);
        put(seq, tid, clip)?;
    }
    Ok(link)
}

/// Remove the named clips from whatever group they were in.
pub(crate) fn unlink_clips(seq: &mut Sequence, ids: &[ClipId]) -> Result<()> {
    for id in ids {
        let (tid, mut clip) = locate(seq, *id)?;
        ensure_unlocked(seq, tid)?;
        clip.link = None;
        put(seq, tid, clip)?;
    }
    Ok(())
}

pub(crate) fn add_marker(
    seq: &mut Sequence,
    cx: &mut Ctx<'_>,
    marker: crate::model::Marker,
) -> crate::ids::MarkerId {
    let id = crate::ids::MarkerId(cx.next());
    let mut marker = marker;
    marker.id = id;
    marker.time = seq.snap(marker.time).max(Time::ZERO);
    marker.duration = seq.snap(marker.duration).max(Time::ZERO);
    let at = seq
        .markers
        .iter()
        .position(|m| m.time > marker.time)
        .unwrap_or(seq.markers.len());
    seq.markers.insert(at, marker);
    id
}

pub(crate) fn remove_marker(seq: &mut Sequence, id: crate::ids::MarkerId) -> Result<()> {
    let index = seq
        .markers
        .iter()
        .position(|m| m.id == id)
        .ok_or(Error::NothingTo("remove: no such marker"))?;
    seq.markers.remove(index);
    Ok(())
}

// ---------------------------------------------------------------------------
// Speed, direction and transitions
// ---------------------------------------------------------------------------

/// Change the playback speed of a clip (and the clips linked to it).
///
/// With [`SpeedMode::KeepMedia`] the clip keeps showing the same stretch of media, so its length
/// on the timeline changes (rounded down to whole frames, so it never needs more media than it
/// had). `ripple` then closes or opens the gap, as it does for a trim. With
/// [`SpeedMode::KeepLength`] the clip stays as long as it is and shows more or less media; a
/// reversed clip keeps its first moment, a forward clip keeps its `source_in`.
pub(crate) fn set_speed(
    seq: &mut Sequence,
    assets: &OrdMap<AssetId, Asset>,
    clip_id: ClipId,
    speed: Speed,
    mode: SpeedMode,
    ripple: Ripple,
    links: LinkMode,
) -> Result<()> {
    let group = expand(seq, &[clip_id], links)?;
    let frame = seq.frame_duration();
    let rate = seq.frame_rate;
    let mut changes = Vec::new();
    for id in &group {
        let (track, old) = locate(seq, *id)?;
        ensure_unlocked(seq, track)?;
        if old.is_ramped() {
            return Err(Error::Animated {
                what: format!("the speed of {id}"),
            });
        }
        if old.speed == speed {
            continue;
        }
        let mut new = old.clone();
        new.speed = speed;
        match mode {
            SpeedMode::KeepMedia => {
                let frames = speed.timeline_len(old.source_len()).to_frames_floor(rate);
                new.duration = Time::from_frames(frames, rate);
            }
            SpeedMode::KeepLength => {}
        }
        if new.duration < frame {
            return Err(Error::TooShort);
        }
        if old.reverse {
            // Keep the moment the clip starts playing from.
            new.source_in = old.source_in + old.source_len() - new.source_len();
        }
        check_source(assets, frame, &new)?;
        if ripple == Ripple::Off && new.end() > old.end() {
            let hit = seq
                .track(track)
                .and_then(|t| t.clips_in(new.range()).into_iter().find(|c| c.id != new.id))
                .map(|c| c.id);
            if let Some(other) = hit {
                return Err(Error::Collision {
                    clip: new.id,
                    other,
                });
            }
        }
        changes.push(Change { track, old, new });
    }
    for ch in &changes {
        put(seq, ch.track, ch.new.clone())?;
    }
    if ripple != Ripple::Off {
        let primary = changes.iter().find(|c| c.old.id == clip_id);
        let shift = primary.map_or(Time::ZERO, |c| c.new.end() - c.old.end());
        let primary_end = primary.map_or(Time::ZERO, |c| c.old.end());
        let edited: BTreeSet<TrackId> = changes.iter().map(|c| c.track).collect();
        for ch in &changes {
            shift_from(seq.track_mut(ch.track)?, ch.old.end(), shift)?;
        }
        if ripple == Ripple::Sync {
            for tid in unlocked_tracks(seq) {
                if !edited.contains(&tid) {
                    shift_from(seq.track_mut(tid)?, primary_end, shift)?;
                }
            }
        }
    }
    Ok(())
}

/// Play a clip (and the clips linked to it) forwards or backwards. The clip keeps its place,
/// its length and the stretch of media it shows.
pub(crate) fn set_reverse(
    seq: &mut Sequence,
    clip: ClipId,
    reverse: bool,
    links: LinkMode,
) -> Result<()> {
    for id in expand(seq, &[clip], links)? {
        let (track, mut c) = locate(seq, id)?;
        ensure_unlocked(seq, track)?;
        c.reverse = reverse;
        put(seq, track, c)?;
    }
    Ok(())
}

/// Put a transition into `clip` from the clip before it, or remove it (`None`). With
/// [`LinkMode::Follow`], linked audio clips get a crossfade of the same length and alignment.
pub(crate) fn set_transition(
    seq: &mut Sequence,
    assets: &OrdMap<AssetId, Asset>,
    clip: ClipId,
    transition: Option<Transition>,
    links: LinkMode,
) -> Result<()> {
    let transition = transition.map(|mut t| {
        t.duration = seq.snap(t.duration);
        t
    });
    for id in expand(seq, &[clip], links)? {
        let (tid, mut c) = locate(seq, id)?;
        ensure_unlocked(seq, tid)?;
        let on_audio = seq.track_position(tid).map(|(k, _)| k) == Some(TrackKind::Audio);
        c.transition_in = transition.map(|mut t| {
            if on_audio {
                t.kind = crate::retime::TransitionKind::Dissolve;
            }
            t
        });
        put(seq, tid, c.clone())?;
        if c.transition_in.is_some() {
            let track = seq.track(tid).ok_or(Error::NoSuchTrack(tid))?;
            if let Some(problem) = seq.transition_problem(track, &c, assets) {
                return Err(Error::BadTransition {
                    clip: id,
                    reason: problem,
                });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Sound settings
// ---------------------------------------------------------------------------

/// Change the gain, pan and fades of audio clips (and the clips linked to them: a linked
/// picture is skipped, since it has no sound settings).
pub(crate) fn set_clip_audio(
    seq: &mut Sequence,
    ids: &[ClipId],
    change: ClipAudio,
    links: LinkMode,
) -> Result<()> {
    let mut any = false;
    for id in expand(seq, ids, links)? {
        let (tid, mut c) = locate(seq, id)?;
        let is_audio = seq.track_position(tid).map(|(k, _)| k) == Some(TrackKind::Audio);
        if !is_audio {
            if ids.contains(&id) {
                return Err(Error::NotAudio(id));
            }
            continue;
        }
        ensure_unlocked(seq, tid)?;
        any = true;
        if change.gain.is_some() && c.anim.is_animated(Param::Gain) {
            return Err(Error::Animated {
                what: format!("the gain of {id}"),
            });
        }
        if change.pan.is_some() && c.anim.is_animated(Param::Pan) {
            return Err(Error::Animated {
                what: format!("the pan of {id}"),
            });
        }
        if let Some(g) = change.gain {
            c.gain = g;
        }
        if let Some(p) = change.pan {
            c.pan = p;
        }
        if let Some(f) = change.fade_in {
            c.fade_in = f.filter(|f| f.duration > Time::ZERO);
        }
        if let Some(f) = change.fade_out {
            c.fade_out = f.filter(|f| f.duration > Time::ZERO);
        }
        let total = c.fade_in.map_or(Time::ZERO, |f| f.duration)
            + c.fade_out.map_or(Time::ZERO, |f| f.duration);
        if total > c.duration {
            return Err(Error::FadesTooLong { clip: id });
        }
        put(seq, tid, c)?;
    }
    if any {
        Ok(())
    } else {
        Err(Error::NothingTo("change: no audio clip given"))
    }
}

// ---------------------------------------------------------------------------
// Keyframes
// ---------------------------------------------------------------------------

fn bad_key(msg: impl std::fmt::Display) -> Error {
    Error::BadKeyframe(msg.to_string())
}

/// Put a key in `anim`. A parameter that was not animated yet starts from `fixed` (its fixed
/// value) at time zero, so the value stays as it was until the new key takes over.
fn key_into(
    anim: &mut Animation,
    param: Param,
    off: i64,
    value: f64,
    interp: Interp,
    fixed: f64,
) -> Result<()> {
    if !param.contains(value) {
        let (lo, hi) = param.range();
        return Err(bad_key(format!(
            "{param} must be between {lo} and {hi}{}",
            param.unit()
        )));
    }
    let mut curve = anim.get(param).cloned().unwrap_or_default();
    if curve.is_empty() && off > 0 {
        curve.set(0, fixed, interp).map_err(bad_key)?;
    }
    curve.set(off, value, interp).map_err(bad_key)?;
    anim.set(param, curve);
    Ok(())
}

/// What a keyframe edit needs to know about where its target lives.
enum Place {
    Clip(Vec<ClipId>),
    Track(TrackId),
    Master,
}

fn place_of(seq: &Sequence, target: Target, param: Param, links: LinkMode) -> Result<Place> {
    match target {
        Target::Clip(id) => {
            // A speed ramp has to stay in step between a picture and its linked sound.
            let ids = if param == Param::Speed {
                expand(seq, &[id], links)?
            } else {
                vec![id]
            };
            Ok(Place::Clip(ids))
        }
        Target::Track(id) => {
            let track = seq.track(id).ok_or(Error::NoSuchTrack(id))?;
            if track.kind != TrackKind::Audio || param == Param::Speed {
                return Err(bad_key(format!(
                    "{id}: only audio tracks have animated gain and pan"
                )));
            }
            Ok(Place::Track(id))
        }
        Target::Master => {
            if param != Param::Gain {
                return Err(bad_key("the master only has gain"));
            }
            Ok(Place::Master)
        }
    }
}

/// Check that a clip can take this parameter.
fn clip_can(seq: &Sequence, tid: TrackId, id: ClipId, param: Param) -> Result<()> {
    let is_audio = seq.track_position(tid).map(|(k, _)| k) == Some(TrackKind::Audio);
    if param != Param::Speed && !is_audio {
        return Err(Error::NotAudio(id));
    }
    Ok(())
}

/// Set a keyframe at timeline time `time`, replacing any key already there.
pub(crate) fn set_keyframe(
    seq: &mut Sequence,
    target: Target,
    param: Param,
    time: Time,
    value: f64,
    interp: Interp,
    links: LinkMode,
) -> Result<()> {
    match place_of(seq, target, param, links)? {
        Place::Clip(ids) => {
            for id in ids {
                let (tid, mut c) = locate(seq, id)?;
                clip_can(seq, tid, id, param)?;
                ensure_unlocked(seq, tid)?;
                if time < c.start || time > c.end() {
                    return Err(Error::NotInsideClip { clip: id, time });
                }
                let fixed = match param {
                    Param::Gain => f64::from(c.gain.db()),
                    Param::Pan => f64::from(c.pan.percent()),
                    Param::Speed => c.speed.as_f64(),
                };
                key_into(
                    &mut c.anim,
                    param,
                    (time - c.start).ticks(),
                    value,
                    interp,
                    fixed,
                )?;
                put(seq, tid, c)?;
            }
        }
        Place::Track(id) => {
            if time < Time::ZERO {
                return Err(Error::BeforeStart(time));
            }
            let track = seq.track_mut(id)?;
            let fixed = match param {
                Param::Gain => f64::from(track.gain.db()),
                _ => f64::from(track.pan.percent()),
            };
            key_into(&mut track.anim, param, time.ticks(), value, interp, fixed)?;
        }
        Place::Master => {
            if time < Time::ZERO {
                return Err(Error::BeforeStart(time));
            }
            let fixed = f64::from(seq.master_gain.db());
            key_into(
                &mut seq.master_anim,
                param,
                time.ticks(),
                value,
                interp,
                fixed,
            )?;
        }
    }
    Ok(())
}

/// Remove the keyframe at exactly `time`. When it was the last one the parameter stops being
/// animated and goes back to its fixed value.
pub(crate) fn remove_keyframe(
    seq: &mut Sequence,
    target: Target,
    param: Param,
    time: Time,
    links: LinkMode,
) -> Result<()> {
    let drop_key = |anim: &mut Animation, off: i64| -> bool {
        let Some(mut curve) = anim.get(param).cloned() else {
            return false;
        };
        let found = curve.remove(off);
        anim.set(param, curve);
        found
    };
    let mut found = false;
    match place_of(seq, target, param, links)? {
        Place::Clip(ids) => {
            for id in ids {
                let (tid, mut c) = locate(seq, id)?;
                ensure_unlocked(seq, tid)?;
                found |= drop_key(&mut c.anim, (time - c.start).ticks());
                put(seq, tid, c)?;
            }
        }
        Place::Track(id) => {
            found = drop_key(&mut seq.track_mut(id)?.anim, time.ticks());
        }
        Place::Master => found = drop_key(&mut seq.master_anim, time.ticks()),
    }
    if found {
        Ok(())
    } else {
        Err(Error::NothingTo("remove: no keyframe at that time"))
    }
}

/// Stop animating a parameter: its curve is thrown away and the fixed value applies again.
pub(crate) fn clear_animation(
    seq: &mut Sequence,
    target: Target,
    param: Param,
    links: LinkMode,
) -> Result<()> {
    let mut found = false;
    match place_of(seq, target, param, links)? {
        Place::Clip(ids) => {
            for id in ids {
                let (tid, mut c) = locate(seq, id)?;
                ensure_unlocked(seq, tid)?;
                found |= c.anim.remove(param);
                put(seq, tid, c)?;
            }
        }
        Place::Track(id) => found = seq.track_mut(id)?.anim.remove(param),
        Place::Master => found = seq.master_anim.remove(param),
    }
    if found {
        Ok(())
    } else {
        Err(Error::NothingTo("clear: that value is not animated"))
    }
}

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

/// A picture clip, unlocked, ready to have its effect stack changed.
fn effect_clip(seq: &Sequence, id: ClipId) -> Result<(TrackId, Clip)> {
    let (tid, c) = locate(seq, id)?;
    if seq.track_position(tid).map(|(k, _)| k) != Some(TrackKind::Video) {
        return Err(Error::NotPicture(id));
    }
    ensure_unlocked(seq, tid)?;
    Ok((tid, c))
}

fn find_effect(c: &mut Clip, effect: EffectId) -> Result<&mut Effect> {
    let clip = c.id;
    c.effects
        .iter_mut()
        .find(|e| e.id == effect)
        .ok_or(Error::NoSuchEffect { clip, effect })
}

fn setting_index(e: &Effect, name: &str) -> Result<usize> {
    e.kind.param_index(name).ok_or_else(|| {
        let names: Vec<&str> = e.kind.params().iter().map(|s| s.name).collect();
        Error::BadEffect(format!(
            "{} has no setting '{name}'; it has: {}",
            e.kind,
            names.join(", ")
        ))
    })
}

fn check_setting(e: &Effect, i: usize, value: f64) -> Result<()> {
    let s = e.kind.params()[i];
    if !value.is_finite() || value < s.min || value > s.max {
        return Err(Error::BadEffect(format!(
            "{} {} must be between {} and {} ({})",
            e.kind, s.name, s.min, s.max, s.unit
        )));
    }
    Ok(())
}

/// Add an effect to a picture clip, at `index` in the stack (the end if `None`). Returns its id.
pub(crate) fn add_effect(
    seq: &mut Sequence,
    clip: ClipId,
    kind: EffectKind,
    index: Option<usize>,
) -> Result<EffectId> {
    let (tid, mut c) = effect_clip(seq, clip)?;
    if c.effects.len() >= MAX_EFFECTS {
        return Err(Error::BadEffect(format!(
            "a clip can have at most {MAX_EFFECTS} effects"
        )));
    }
    let id = EffectId(c.effects.iter().map(|e| e.id.0).max().unwrap_or(0) + 1);
    let at = index.unwrap_or(c.effects.len()).min(c.effects.len());
    c.effects.insert(at, Effect::new(id, kind));
    put(seq, tid, c)?;
    Ok(id)
}

pub(crate) fn remove_effect(seq: &mut Sequence, clip: ClipId, effect: EffectId) -> Result<()> {
    let (tid, mut c) = effect_clip(seq, clip)?;
    find_effect(&mut c, effect)?;
    c.effects.retain(|e| e.id != effect);
    put(seq, tid, c)
}

/// Move an effect to position `to` in the stack (0 is applied first).
pub(crate) fn move_effect(
    seq: &mut Sequence,
    clip: ClipId,
    effect: EffectId,
    to: usize,
) -> Result<()> {
    let (tid, mut c) = effect_clip(seq, clip)?;
    find_effect(&mut c, effect)?;
    let from = c.effects.iter().position(|e| e.id == effect).unwrap_or(0);
    let e = c.effects.remove(from);
    c.effects.insert(to.min(c.effects.len()), e);
    put(seq, tid, c)
}

pub(crate) fn set_effect_enabled(
    seq: &mut Sequence,
    clip: ClipId,
    effect: EffectId,
    enabled: bool,
) -> Result<()> {
    let (tid, mut c) = effect_clip(seq, clip)?;
    find_effect(&mut c, effect)?.enabled = enabled;
    put(seq, tid, c)
}

/// Set the fixed value of a setting. Refused while the setting is animated.
pub(crate) fn set_effect_value(
    seq: &mut Sequence,
    clip: ClipId,
    effect: EffectId,
    name: &str,
    value: f64,
) -> Result<()> {
    let (tid, mut c) = effect_clip(seq, clip)?;
    let e = find_effect(&mut c, effect)?;
    let i = setting_index(e, name)?;
    check_setting(e, i, value)?;
    if e.is_animated(i) {
        return Err(Error::Animated {
            what: format!("{name} of {effect} on {clip}"),
        });
    }
    e.values[i] = value;
    put(seq, tid, c)
}

/// Put a key on a setting at timeline time `time`. A setting that was not animated starts from its
/// fixed value at the start of the clip.
pub(crate) fn key_effect(
    seq: &mut Sequence,
    clip: ClipId,
    effect: EffectId,
    name: &str,
    time: Time,
    value: f64,
    interp: Interp,
) -> Result<()> {
    let (tid, mut c) = effect_clip(seq, clip)?;
    if time < c.start || time > c.end() {
        return Err(Error::NotInsideClip { clip, time });
    }
    let off = (time - c.start).ticks();
    let e = find_effect(&mut c, effect)?;
    let i = setting_index(e, name)?;
    check_setting(e, i, value)?;
    let mut curve = e.curves[i].clone().unwrap_or_default();
    if curve.is_empty() && off > 0 {
        curve.set(0, e.values[i], interp).map_err(bad_key)?;
    }
    curve.set(off, value, interp).map_err(bad_key)?;
    e.curves[i] = Some(curve);
    put(seq, tid, c)
}

/// Remove the key at exactly `time`; the last key ending the animation.
pub(crate) fn unkey_effect(
    seq: &mut Sequence,
    clip: ClipId,
    effect: EffectId,
    name: &str,
    time: Time,
) -> Result<()> {
    let (tid, mut c) = effect_clip(seq, clip)?;
    let off = (time - c.start).ticks();
    let e = find_effect(&mut c, effect)?;
    let i = setting_index(e, name)?;
    let mut curve = e.curves[i].clone().unwrap_or_default();
    if !curve.remove(off) {
        return Err(Error::NothingTo("remove: no keyframe at that time"));
    }
    e.curves[i] = if curve.is_empty() { None } else { Some(curve) };
    put(seq, tid, c)
}

/// Stop animating a setting; its fixed value applies again.
pub(crate) fn clear_effect_animation(
    seq: &mut Sequence,
    clip: ClipId,
    effect: EffectId,
    name: &str,
) -> Result<()> {
    let (tid, mut c) = effect_clip(seq, clip)?;
    let e = find_effect(&mut c, effect)?;
    let i = setting_index(e, name)?;
    if e.curves[i].take().is_none() {
        return Err(Error::NothingTo("clear: that setting is not animated"));
    }
    put(seq, tid, c)
}
