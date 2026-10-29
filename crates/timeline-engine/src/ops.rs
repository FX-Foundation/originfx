// SPDX-License-Identifier: GPL-3.0-or-later
//! The edit operations, written as plain functions on a sequence.
//!
//! These functions change the project they are given and may leave it half
//! changed when they fail. They are only ever called by [`crate::Editor`] on a
//! private copy of the project, which is thrown away on error, so every public
//! edit is all-or-nothing.

use std::collections::{BTreeSet, HashMap};

use im::OrdMap;

use crate::error::{Error, Result};
use crate::ids::{AssetId, ClipId, LinkId, SequenceId, TrackId};
use crate::model::{Asset, Clip, Project, Sequence, Track, TrackKind};
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
            let mut left = c.clone();
            left.duration = range.start - c.start;
            track.items.insert(left.start, left);
            kept_left = true;
        }
        if c.end() > range.end {
            let mut right = c.clone();
            right.start = range.end;
            right.source_in = c.source_in + (range.end - c.start);
            right.duration = c.end() - range.end;
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
        let mut left = clip.clone();
        left.duration = t - clip.start;
        let mut right = clip.clone();
        right.id = cx.clip_id();
        right.start = t;
        right.source_in = clip.source_in + (t - clip.start);
        right.duration = clip.end() - t;
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
    Ok(())
}

// ---------------------------------------------------------------------------
// Placing clips
// ---------------------------------------------------------------------------

/// The whole asset from `source_in` onwards, rounded down to whole frames.
fn default_duration(seq: &Sequence, asset: &Asset, source_in: Time) -> Result<Time> {
    let limit = asset.duration.ok_or(Error::NeedsDuration)?;
    let frames = (limit - source_in).to_frames_floor(seq.frame_rate);
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
        None => default_duration(seq, asset, spec.source_in)?,
    };
    if duration < seq.frame_duration() {
        return Err(Error::TooShort);
    }
    if let Some(limit) = asset.duration {
        if spec.source_in + duration > limit {
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
        None if asset.duration.is_some() => Some(default_duration(seq, asset, source_in)?),
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
    if new.source_in < Time::ZERO {
        return Err(Error::OutsideSource {
            clip: new.id,
            detail: "before the start of the media".into(),
        });
    }
    let asset = assets
        .get(&new.asset)
        .ok_or(Error::NoSuchAsset(new.asset))?;
    if let Some(limit) = asset.duration {
        if new.source_in + new.duration > limit {
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
        let mut new = old.clone();
        match (edge, rippling) {
            (Edge::Head, false) => {
                new.start = old.start + delta;
                new.source_in = old.source_in + delta;
                new.duration = old.duration - delta;
            }
            (Edge::Head, true) => {
                new.source_in = old.source_in + delta;
                new.duration = old.duration - delta;
            }
            (Edge::Tail, _) => new.duration = old.duration + delta,
        }
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
    let mut new_l = l.clone();
    new_l.duration = l.duration + delta;
    let mut new_r = r.clone();
    new_r.start = r.start + delta;
    new_r.source_in = r.source_in + delta;
    new_r.duration = r.duration - delta;
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
    let mut new_prev = prev.clone();
    new_prev.duration = prev.duration + delta;
    let mut new_c = c.clone();
    new_c.start = c.start + delta;
    let mut new_next = next.clone();
    new_next.start = next.start + delta;
    new_next.source_in = next.source_in + delta;
    new_next.duration = next.duration - delta;
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
