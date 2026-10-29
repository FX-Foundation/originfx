// SPDX-License-Identifier: GPL-3.0-or-later
//! The [`Editor`]: the one place a project is changed.
//!
//! Every edit runs on a private copy of the project. If it fails, or the result would
//! break a rule of [`Project::validate`], the copy is dropped and the project is exactly
//! as it was. If it works, the old project is pushed on the undo list. Because projects
//! share their unchanged parts, keeping thousands of old versions costs very little, and
//! undo needs no "inverse" of any edit.

use std::path::PathBuf;

use crate::animation::{Interp, Param, Target};
use crate::audio::{ClipAudio, Gain};
use crate::effects::{EffectId, EffectKind};
use crate::error::{Error, Result};
use crate::ids::{AssetId, ClipId, LinkId, MarkerId, SequenceId, TrackId};
use crate::model::{Marker, MarkerColour, NewAsset, Project, SequenceSettings, TrackKind};
use crate::ops::{self, ClipSpec, Edge, LinkMode, PlaceMode, Placed, Ripple, TrackUpdate};
use crate::retime::{Speed, SpeedMode, Transition};
use crate::time::{Time, TimeRange};

/// How many edits can be undone before the oldest are forgotten.
pub const UNDO_LIMIT: usize = 10_000;

#[derive(Debug, Clone)]
struct Entry {
    label: String,
    project: Project,
    version: u64,
}

/// A project plus its undo and redo history.
#[derive(Debug, Clone)]
pub struct Editor {
    project: Project,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    version: u64,
    counter: u64,
    saved: u64,
    group_depth: u32,
}

impl Editor {
    /// An editor on a new empty project.
    pub fn new(name: impl Into<String>) -> Editor {
        Editor::from_project(Project::new(name))
    }

    /// An editor on an existing project (for example one just loaded from disk).
    pub fn from_project(project: Project) -> Editor {
        Editor {
            project,
            undo: Vec::new(),
            redo: Vec::new(),
            version: 0,
            counter: 0,
            saved: 0,
            group_depth: 0,
        }
    }

    /// The current project. Cloning it is cheap and gives a snapshot that never changes,
    /// which is how other threads (a player, an exporter) read the timeline.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Whether the project has changed since it was created, loaded or last marked saved.
    pub fn is_dirty(&self) -> bool {
        self.version != self.saved
    }

    /// Call after saving the project.
    pub fn mark_saved(&mut self) {
        self.saved = self.version;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The name of the edit that undo would take back.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|e| e.label.as_str())
    }

    /// The name of the edit that redo would put back.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|e| e.label.as_str())
    }

    /// Take back the last edit. Returns its label.
    pub fn undo(&mut self) -> Result<String> {
        let entry = self.undo.pop().ok_or(Error::NothingTo("undo"))?;
        let now = std::mem::replace(&mut self.project, entry.project);
        self.redo.push(Entry {
            label: entry.label.clone(),
            project: now,
            version: self.version,
        });
        self.version = entry.version;
        Ok(entry.label)
    }

    /// Put back the edit that was undone. Returns its label.
    pub fn redo(&mut self) -> Result<String> {
        let entry = self.redo.pop().ok_or(Error::NothingTo("redo"))?;
        let now = std::mem::replace(&mut self.project, entry.project);
        self.undo.push(Entry {
            label: entry.label.clone(),
            project: now,
            version: self.version,
        });
        self.version = entry.version;
        Ok(entry.label)
    }

    /// Run several edits as one undo step. If any fails, they are all taken back.
    pub fn group<R>(&mut self, label: &str, f: impl FnOnce(&mut Editor) -> Result<R>) -> Result<R> {
        let before = self.project.clone();
        let (version, redo_len) = (self.version, self.redo.len());
        self.group_depth += 1;
        let result = f(self);
        self.group_depth -= 1;
        match result {
            Ok(r) => {
                if self.group_depth == 0 && self.project != before {
                    self.push_undo(label, before, version);
                    self.redo.clear();
                }
                Ok(r)
            }
            Err(e) => {
                self.project = before;
                self.version = version;
                debug_assert!(self.redo.len() >= redo_len.min(self.redo.len()));
                Err(e)
            }
        }
    }

    fn push_undo(&mut self, label: &str, before: Project, version: u64) {
        self.undo.push(Entry {
            label: label.to_string(),
            project: before,
            version,
        });
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.counter += 1;
        self.version = self.counter;
    }

    /// The core of every edit: copy, change, check, commit.
    fn apply<R>(
        &mut self,
        label: &str,
        check: Option<SequenceId>,
        f: impl FnOnce(&mut Project) -> Result<R>,
    ) -> Result<R> {
        let mut work = self.project.clone();
        let out = f(&mut work)?;
        if let Some(id) = check {
            // A transition whose clips no longer touch, or no longer have spare media, is dropped
            // rather than failing the edit that made it impossible.
            let assets = work.assets.clone();
            if let Some(seq) = work.sequences.get_mut(&id) {
                seq.fit_fades();
                seq.prune_transitions(&assets);
            }
            let seq = work.sequence(id).ok_or(Error::NoSuchSequence(id))?;
            seq.validate(work.assets())?;
        }
        if work == self.project {
            return Ok(out);
        }
        let before = std::mem::replace(&mut self.project, work);
        if self.group_depth == 0 {
            let version = self.version;
            self.push_undo(label, before, version);
            self.redo.clear();
        } else {
            self.counter += 1;
            self.version = self.counter;
        }
        Ok(out)
    }

    fn edit_seq<R>(
        &mut self,
        label: &str,
        seq: SequenceId,
        f: impl FnOnce(&mut crate::model::Sequence, &mut ops::Ctx<'_>) -> Result<R>,
    ) -> Result<R> {
        self.apply(label, Some(seq), |p| p.with_seq(seq, f))
    }

    // ----- assets and sequences -------------------------------------------------

    /// Add a media file to the project.
    pub fn add_asset(&mut self, new: NewAsset) -> Result<AssetId> {
        self.apply("Import media", None, |p| Ok(p.add_asset(new)))
    }

    /// Remove a media file that no clip uses.
    pub fn remove_asset(&mut self, id: AssetId) -> Result<()> {
        self.apply("Remove media", None, |p| p.remove_asset(id))
    }

    /// Point an asset at a different file (after the file was moved).
    pub fn relink_asset(&mut self, id: AssetId, path: impl Into<PathBuf>) -> Result<()> {
        let path = path.into();
        self.apply("Relink media", None, |p| p.relink_asset(id, path))
    }

    /// Create a sequence with its starting tracks.
    pub fn add_sequence(&mut self, settings: SequenceSettings) -> Result<SequenceId> {
        self.apply("New sequence", None, |p| Ok(p.add_sequence(settings)))
    }

    pub fn remove_sequence(&mut self, id: SequenceId) -> Result<()> {
        self.apply("Delete sequence", None, |p| p.remove_sequence(id))
    }

    pub fn rename_sequence(&mut self, id: SequenceId, name: impl Into<String>) -> Result<()> {
        let name = name.into();
        self.edit_seq("Rename sequence", id, |s, _| {
            s.name = name;
            Ok(())
        })
    }

    // ----- tracks ---------------------------------------------------------------

    pub fn add_track(
        &mut self,
        seq: SequenceId,
        kind: TrackKind,
        name: Option<String>,
    ) -> Result<TrackId> {
        self.edit_seq("Add track", seq, |s, cx| {
            Ok(ops::add_track(s, cx, kind, name))
        })
    }

    /// Remove a track and the clips on it. The track must not be locked.
    pub fn remove_track(&mut self, seq: SequenceId, track: TrackId) -> Result<()> {
        self.edit_seq("Remove track", seq, |s, _| ops::remove_track(s, track))
    }

    pub fn update_track(
        &mut self,
        seq: SequenceId,
        track: TrackId,
        update: TrackUpdate,
    ) -> Result<()> {
        self.edit_seq("Track settings", seq, |s, _| {
            ops::update_track(s, track, update)
        })
    }

    // ----- placing --------------------------------------------------------------

    /// Put one clip on a track.
    pub fn place_clip(
        &mut self,
        seq: SequenceId,
        track: TrackId,
        spec: ClipSpec,
        mode: PlaceMode,
    ) -> Result<ClipId> {
        let label = match mode {
            PlaceMode::Overwrite => "Overwrite",
            PlaceMode::Insert { .. } => "Insert",
        };
        self.edit_seq(label, seq, |s, cx| {
            ops::place_clip(s, cx, track, spec, mode, None)
        })
    }

    /// Put an asset on the timeline as a linked picture and sound pair (or just one of them
    /// if only one track is given). `duration: None` uses the rest of the asset.
    #[allow(clippy::too_many_arguments)]
    pub fn place_asset(
        &mut self,
        seq: SequenceId,
        asset: AssetId,
        start: Time,
        source_in: Time,
        duration: Option<Time>,
        video_track: Option<TrackId>,
        audio_track: Option<TrackId>,
        mode: PlaceMode,
    ) -> Result<Placed> {
        let label = match mode {
            PlaceMode::Overwrite => "Overwrite",
            PlaceMode::Insert { .. } => "Insert",
        };
        self.edit_seq(label, seq, |s, cx| {
            ops::place_asset(
                s,
                cx,
                asset,
                start,
                source_in,
                duration,
                video_track,
                audio_track,
                mode,
            )
        })
    }

    // ----- cutting and removing -------------------------------------------------

    /// Cut a clip in two at `time`. Returns the left and right parts.
    pub fn split_clip(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        time: Time,
        links: LinkMode,
    ) -> Result<(ClipId, ClipId)> {
        self.edit_seq("Split", seq, |s, cx| {
            ops::split_clip(s, cx, clip, time, links)
        })
    }

    /// Cut every clip under `time` on the given tracks (all unlocked tracks if `None`).
    pub fn split_all(
        &mut self,
        seq: SequenceId,
        time: Time,
        tracks: Option<&[TrackId]>,
    ) -> Result<Vec<(ClipId, ClipId)>> {
        self.edit_seq("Split all", seq, |s, cx| {
            ops::split_all(s, cx, time, tracks)
        })
    }

    pub fn delete_clips(
        &mut self,
        seq: SequenceId,
        clips: &[ClipId],
        ripple: Ripple,
        links: LinkMode,
    ) -> Result<()> {
        let label = if ripple == Ripple::Off {
            "Delete"
        } else {
            "Ripple delete"
        };
        self.edit_seq(label, seq, |s, _| {
            ops::delete_clips(s, clips, ripple, links)
        })
    }

    /// Cut out a span of time and close the gap.
    pub fn ripple_delete_range(
        &mut self,
        seq: SequenceId,
        range: TimeRange,
        tracks: Option<&[TrackId]>,
    ) -> Result<()> {
        self.edit_seq("Ripple delete range", seq, |s, cx| {
            ops::ripple_delete_range(s, cx, range, tracks)
        })
    }

    // ----- moving and trimming --------------------------------------------------

    /// Move clips later (positive) or earlier, and optionally up or down by whole tracks.
    /// Whatever they land on is overwritten.
    pub fn move_clips(
        &mut self,
        seq: SequenceId,
        clips: &[ClipId],
        delta: Time,
        track_delta: i32,
        links: LinkMode,
    ) -> Result<()> {
        self.edit_seq("Move", seq, |s, cx| {
            ops::move_clips(s, cx, clips, delta, track_delta, links)
        })
    }

    /// Move the head or tail of a clip by `delta`.
    pub fn trim(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        edge: Edge,
        delta: Time,
        ripple: Ripple,
        links: LinkMode,
    ) -> Result<()> {
        let label = if ripple == Ripple::Off {
            "Trim"
        } else {
            "Ripple trim"
        };
        self.apply(label, Some(seq), |p| {
            let assets = p.assets.clone();
            let s = p
                .sequences
                .get_mut(&seq)
                .ok_or(Error::NoSuchSequence(seq))?;
            ops::trim(s, &assets, clip, edge, delta, ripple, links)
        })
    }

    /// Move the cut between two touching clips.
    pub fn roll(
        &mut self,
        seq: SequenceId,
        left: ClipId,
        right: ClipId,
        delta: Time,
    ) -> Result<()> {
        self.apply("Roll", Some(seq), |p| {
            let assets = p.assets.clone();
            let s = p
                .sequences
                .get_mut(&seq)
                .ok_or(Error::NoSuchSequence(seq))?;
            ops::roll(s, &assets, left, right, delta)
        })
    }

    /// Show a different part of the media without moving the clip.
    pub fn slip(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        delta: Time,
        links: LinkMode,
    ) -> Result<()> {
        self.apply("Slip", Some(seq), |p| {
            let assets = p.assets.clone();
            let s = p
                .sequences
                .get_mut(&seq)
                .ok_or(Error::NoSuchSequence(seq))?;
            ops::slip(s, &assets, clip, delta, links)
        })
    }

    /// Move a clip between its neighbours, which stretch and shrink to fill.
    pub fn slide(&mut self, seq: SequenceId, clip: ClipId, delta: Time) -> Result<()> {
        self.apply("Slide", Some(seq), |p| {
            let assets = p.assets.clone();
            let s = p
                .sequences
                .get_mut(&seq)
                .ok_or(Error::NoSuchSequence(seq))?;
            ops::slide(s, &assets, clip, delta)
        })
    }

    // ----- speed, direction, transitions ------------------------------------------

    /// Change how fast a clip plays. See [`SpeedMode`] for what happens to its length, and
    /// `ripple` for what happens to the clips after it.
    pub fn set_speed(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        speed: Speed,
        mode: SpeedMode,
        ripple: Ripple,
        links: LinkMode,
    ) -> Result<()> {
        self.apply("Change speed", Some(seq), |p| {
            let assets = p.assets.clone();
            let s = p
                .sequences
                .get_mut(&seq)
                .ok_or(Error::NoSuchSequence(seq))?;
            ops::set_speed(s, &assets, clip, speed, mode, ripple, links)
        })
    }

    /// Play a clip backwards (or forwards again).
    pub fn set_reverse(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        reverse: bool,
        links: LinkMode,
    ) -> Result<()> {
        let label = if reverse { "Reverse" } else { "Play forwards" };
        self.edit_seq(label, seq, |s, _| ops::set_reverse(s, clip, reverse, links))
    }

    /// Put a transition into `clip` from the clip before it (`None` removes it). The clips must
    /// touch and have enough spare media; otherwise this fails with
    /// [`Error::BadTransition`] and nothing changes. With [`LinkMode::Follow`] linked audio
    /// clips get a crossfade to match.
    pub fn set_transition(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        transition: Option<Transition>,
        links: LinkMode,
    ) -> Result<()> {
        let label = if transition.is_some() {
            "Add transition"
        } else {
            "Remove transition"
        };
        self.apply(label, Some(seq), |p| {
            let assets = p.assets.clone();
            let s = p
                .sequences
                .get_mut(&seq)
                .ok_or(Error::NoSuchSequence(seq))?;
            ops::set_transition(s, &assets, clip, transition, links)
        })
    }

    // ----- sound ----------------------------------------------------------------

    /// Change the gain, pan or fades of audio clips. A fade longer than its clip is refused.
    /// With [`LinkMode::Follow`], linked pictures are skipped rather than refused.
    pub fn set_clip_audio(
        &mut self,
        seq: SequenceId,
        clips: &[ClipId],
        change: ClipAudio,
        links: LinkMode,
    ) -> Result<()> {
        self.edit_seq("Change sound", seq, |s, _| {
            ops::set_clip_audio(s, clips, change, links)
        })
    }

    /// Change the level of the whole mix of a sequence.
    pub fn set_master_gain(&mut self, seq: SequenceId, gain: Gain) -> Result<()> {
        self.edit_seq("Master level", seq, |s, _| {
            if s.master_anim.is_animated(Param::Gain) {
                return Err(Error::Animated {
                    what: "the master gain".into(),
                });
            }
            s.master_gain = gain;
            Ok(())
        })
    }

    // ----- keyframes -------------------------------------------------------------

    /// Put a keyframe on a clip, an audio track or the master at timeline time `time`, replacing
    /// any key already there. `value` is in the parameter's unit (dB, percent, speed ratio).
    /// A parameter that was not animated starts from its fixed value, which stays in force until
    /// the new key. Speed keys (speed ramps) follow links so picture and sound stay in step.
    #[allow(clippy::too_many_arguments)]
    pub fn set_keyframe(
        &mut self,
        seq: SequenceId,
        target: Target,
        param: Param,
        time: Time,
        value: f64,
        interp: Interp,
        links: LinkMode,
    ) -> Result<()> {
        self.edit_seq("Keyframe", seq, |s, _| {
            ops::set_keyframe(s, target, param, time, value, interp, links)
        })
    }

    /// Remove the keyframe at exactly `time`.
    pub fn remove_keyframe(
        &mut self,
        seq: SequenceId,
        target: Target,
        param: Param,
        time: Time,
        links: LinkMode,
    ) -> Result<()> {
        self.edit_seq("Remove keyframe", seq, |s, _| {
            ops::remove_keyframe(s, target, param, time, links)
        })
    }

    /// Stop animating a value; its fixed value applies again.
    pub fn clear_animation(
        &mut self,
        seq: SequenceId,
        target: Target,
        param: Param,
        links: LinkMode,
    ) -> Result<()> {
        self.edit_seq("Clear keyframes", seq, |s, _| {
            ops::clear_animation(s, target, param, links)
        })
    }

    // ----- effects ---------------------------------------------------------------

    /// Add an effect to a picture clip at `index` in its stack (the end if `None`), with its
    /// default settings. Returns the effect's id, which is unique within the clip.
    pub fn add_effect(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        kind: EffectKind,
        index: Option<usize>,
    ) -> Result<EffectId> {
        self.edit_seq("Add effect", seq, |s, _| {
            ops::add_effect(s, clip, kind, index)
        })
    }

    pub fn remove_effect(&mut self, seq: SequenceId, clip: ClipId, effect: EffectId) -> Result<()> {
        self.edit_seq("Remove effect", seq, |s, _| {
            ops::remove_effect(s, clip, effect)
        })
    }

    /// Move an effect to position `to` in the clip's stack (0 is applied first).
    pub fn move_effect(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        effect: EffectId,
        to: usize,
    ) -> Result<()> {
        self.edit_seq("Move effect", seq, |s, _| {
            ops::move_effect(s, clip, effect, to)
        })
    }

    pub fn set_effect_enabled(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        effect: EffectId,
        enabled: bool,
    ) -> Result<()> {
        self.edit_seq("Enable effect", seq, |s, _| {
            ops::set_effect_enabled(s, clip, effect, enabled)
        })
    }

    /// Set the fixed value of a picture setting: opacity, x, y, scale-x, scale-y, rotation,
    /// crop-*, or a mask value. Refused while the value is animated.
    pub fn set_look_value(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        param: Param,
        value: f64,
    ) -> Result<()> {
        self.edit_seq("Picture setting", seq, |s, _| {
            ops::set_look_value(s, clip, param, value)
        })
    }

    /// Choose how a picture clip's colour combines with what is beneath.
    pub fn set_blend_mode(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        mode: crate::compositing::BlendMode,
    ) -> Result<()> {
        self.edit_seq("Blend mode", seq, |s, _| ops::set_blend_mode(s, clip, mode))
    }

    /// Add a mask to a picture clip, or change the shape and inversion of its mask.
    pub fn set_mask(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        shape: crate::compositing::MaskShape,
        invert: bool,
    ) -> Result<()> {
        self.edit_seq("Mask", seq, |s, _| ops::set_mask(s, clip, shape, invert))
    }

    pub fn remove_mask(&mut self, seq: SequenceId, clip: ClipId) -> Result<()> {
        self.edit_seq("Remove mask", seq, |s, _| ops::remove_mask(s, clip))
    }

    /// Set the fixed value of an effect setting (by name; see `EffectKind::params`). Refused
    /// while the setting is animated.
    pub fn set_effect_value(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        effect: EffectId,
        name: &str,
        value: f64,
    ) -> Result<()> {
        self.edit_seq("Effect setting", seq, |s, _| {
            ops::set_effect_value(s, clip, effect, name, value)
        })
    }

    /// Put a keyframe on an effect setting at timeline time `time`.
    #[allow(clippy::too_many_arguments)]
    pub fn key_effect(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        effect: EffectId,
        name: &str,
        time: Time,
        value: f64,
        interp: crate::animation::Interp,
    ) -> Result<()> {
        self.edit_seq("Effect keyframe", seq, |s, _| {
            ops::key_effect(s, clip, effect, name, time, value, interp)
        })
    }

    pub fn unkey_effect(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        effect: EffectId,
        name: &str,
        time: Time,
    ) -> Result<()> {
        self.edit_seq("Remove effect keyframe", seq, |s, _| {
            ops::unkey_effect(s, clip, effect, name, time)
        })
    }

    pub fn clear_effect_animation(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        effect: EffectId,
        name: &str,
    ) -> Result<()> {
        self.edit_seq("Clear effect keyframes", seq, |s, _| {
            ops::clear_effect_animation(s, clip, effect, name)
        })
    }

    // ----- clip properties, links, markers --------------------------------------

    pub fn set_enabled(
        &mut self,
        seq: SequenceId,
        clips: &[ClipId],
        enabled: bool,
        links: LinkMode,
    ) -> Result<()> {
        self.edit_seq(if enabled { "Enable" } else { "Disable" }, seq, |s, _| {
            ops::set_clips_enabled(s, clips, enabled, links)
        })
    }

    pub fn rename_clip(
        &mut self,
        seq: SequenceId,
        clip: ClipId,
        name: impl Into<String>,
    ) -> Result<()> {
        let name = name.into();
        self.edit_seq("Rename clip", seq, |s, _| ops::rename_clip(s, clip, name))
    }

    /// Make the clips a linked group, so they move and trim together.
    pub fn link(&mut self, seq: SequenceId, clips: &[ClipId]) -> Result<LinkId> {
        self.edit_seq("Link", seq, |s, cx| ops::link_clips(s, cx, clips))
    }

    pub fn unlink(&mut self, seq: SequenceId, clips: &[ClipId]) -> Result<()> {
        self.edit_seq("Unlink", seq, |s, _| ops::unlink_clips(s, clips))
    }

    pub fn add_marker(
        &mut self,
        seq: SequenceId,
        time: Time,
        duration: Time,
        name: impl Into<String>,
        colour: MarkerColour,
    ) -> Result<MarkerId> {
        let marker = Marker {
            id: MarkerId(0),
            time,
            duration,
            name: name.into(),
            note: String::new(),
            colour,
        };
        self.edit_seq("Add marker", seq, |s, cx| {
            Ok(ops::add_marker(s, cx, marker))
        })
    }

    pub fn remove_marker(&mut self, seq: SequenceId, marker: MarkerId) -> Result<()> {
        self.edit_seq("Remove marker", seq, |s, _| ops::remove_marker(s, marker))
    }
}
