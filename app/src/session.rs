// SPDX-License-Identifier: GPL-3.0-or-later
//! One open project: the timeline editor, selection, playhead, caches, and the sync to the UI.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use slint::{ComponentHandle, Image, Model as _, ModelRc, SharedString, VecModel};

use timeline_engine as tl;
use tl::{
    Alignment, AssetId, ClipAudio, ClipId, ClipSpec, Direction, Edge, Editor, EffectKind, Fade, Gain, Interp, LinkMode,
    MarkerColour, MarkerId, Pan, Param, PlaceMode, Project, Ripple, SequenceId, SequenceSettings, Speed, SpeedMode, Target, Time,
    TrackId, TrackKind, TrackUpdate, Transition, TransitionKind,
};

use crate::util;
use crate::{App, AssetItem, ClipItem, EffectItem, KeyItem, MarkerItem, Model, ParamItem, SelectedClip, TrackItem};

pub const TRANSITIONS: [(&str, &str); 11] = [
    ("Cross dissolve", "dissolve"),
    ("Dip to black", "dip-to-black"),
    ("Dip to white", "dip-to-white"),
    ("Wipe left", "wipe-left"),
    ("Wipe right", "wipe-right"),
    ("Wipe up", "wipe-up"),
    ("Wipe down", "wipe-down"),
    ("Push left", "push-left"),
    ("Push right", "push-right"),
    ("Push up", "push-up"),
    ("Push down", "push-down"),
];

pub fn transition_kind(i: i32) -> TransitionKind {
    match i {
        1 => TransitionKind::DipToBlack,
        2 => TransitionKind::DipToWhite,
        3 => TransitionKind::Wipe(Direction::Left),
        4 => TransitionKind::Wipe(Direction::Right),
        5 => TransitionKind::Wipe(Direction::Up),
        6 => TransitionKind::Wipe(Direction::Down),
        7 => TransitionKind::Push(Direction::Left),
        8 => TransitionKind::Push(Direction::Right),
        9 => TransitionKind::Push(Direction::Up),
        10 => TransitionKind::Push(Direction::Down),
        _ => TransitionKind::Dissolve,
    }
}

pub fn transition_index(k: TransitionKind) -> i32 {
    match k {
        TransitionKind::Dissolve => 0,
        TransitionKind::DipToBlack => 1,
        TransitionKind::DipToWhite => 2,
        TransitionKind::Wipe(Direction::Left) => 3,
        TransitionKind::Wipe(Direction::Right) => 4,
        TransitionKind::Wipe(Direction::Up) => 5,
        TransitionKind::Wipe(Direction::Down) => 6,
        TransitionKind::Push(Direction::Left) => 7,
        TransitionKind::Push(Direction::Right) => 8,
        TransitionKind::Push(Direction::Up) => 9,
        TransitionKind::Push(Direction::Down) => 10,
    }
}

pub const BLENDS: [tl::BlendMode; 8] = [
    tl::BlendMode::Normal,
    tl::BlendMode::Multiply,
    tl::BlendMode::Screen,
    tl::BlendMode::Overlay,
    tl::BlendMode::SoftLight,
    tl::BlendMode::Darken,
    tl::BlendMode::Lighten,
    tl::BlendMode::Difference,
];

pub const KF_PARAMS: [Param; 4] = [Param::PositionX, Param::ScaleX, Param::Rotation, Param::Opacity];

pub fn effect_label(kind: EffectKind) -> &'static str {
    use EffectKind::*;
    match kind {
        Brightness => "Brightness",
        Contrast => "Contrast",
        Exposure => "Exposure",
        Gamma => "Gamma",
        Saturation => "Saturation",
        Hue => "Hue",
        Temperature => "Temperature",
        Invert => "Invert",
        Grayscale => "Grayscale",
        Sepia => "Sepia",
        Blur => "Gaussian blur",
        Sharpen => "Sharpen",
        Vignette => "Vignette",
        Pixelate => "Mosaic",
        Balance => "Color balance",
        Curve => "Luma curve",
        #[allow(unreachable_patterns)]
        _ => "Effect",
    }
}

pub struct Peaks {
    pub per_second: f64,
    pub data: Vec<(f32, f32)>,
}

pub struct Session {
    pub editor: Editor,
    pub seq: SequenceId,
    pub path: Option<PathBuf>,
    pub selected: Option<ClipId>,
    pub selected_track: Option<TrackId>,
    pub playhead: f64,
    pub in_point: Option<f64>,
    pub out_point: Option<f64>,
    pub peaks: HashMap<AssetId, Arc<Peaks>>,
    pub thumbs: HashMap<AssetId, Image>,
    pub waveforms: HashMap<(AssetId, i64, i64, u32), Image>,
    pub last_autosave: Instant,
    pub last_saved_revision: u64,
    pub revision: u64,
    /// Sequence aspect choice for the project-settings screen.
    pub aspect: i32,
}

pub type Res<T> = Result<T, String>;

fn e2s<T, E: std::fmt::Display>(r: Result<T, E>) -> Res<T> {
    r.map_err(|e| e.to_string())
}

impl Session {
    pub fn new(editor: Editor, seq: SequenceId, path: Option<PathBuf>) -> Session {
        let aspect = editor
            .project()
            .sequence(seq)
            .map(|s| util::aspect_index(s.width, s.height))
            .unwrap_or(0);
        Session {
            editor,
            seq,
            path,
            selected: None,
            selected_track: None,
            playhead: 0.0,
            in_point: None,
            out_point: None,
            peaks: HashMap::new(),
            thumbs: HashMap::new(),
            waveforms: HashMap::new(),
            last_autosave: Instant::now(),
            last_saved_revision: 0,
            revision: 0,
            aspect,
        }
    }

    /// A fresh project from a template code (template*100 + res*10 + fps + aspect*1000).
    pub fn create(name: &str, code: i32, default_fps: f64) -> Session {
        let template = (code / 100) % 10;
        let res = (code / 10) % 10;
        let fps_i = code % 10;
        let aspect = code / 1000;
        let (w, h) = util::frame_size(res, aspect);
        let fps = match fps_i {
            0 => 24.0,
            1 => 30.0,
            2 => 60.0,
            _ => default_fps,
        };
        let mut ed = Editor::new(name);
        let (video_names, audio_names): (Vec<&str>, Vec<&str>) = match template {
            0 => (vec!["Video", "Titles"], vec!["Audio", "Music"]),
            2 => (vec!["Video", "Captions", "Overlays"], vec!["Audio", "Music"]),
            3 => (vec!["Video", "Titles", "Overlays"], vec!["Audio", "Sound", "Music"]),
            4 => (vec!["Interview", "Cutaway", "Titles"], vec!["Interview", "Voice", "Music"]),
            _ => (vec!["Video", "Titles", "Overlays"], vec!["Audio", "Music", "Voice"]),
        };
        let seq = ed
            .add_sequence(SequenceSettings {
                name: "Sequence 1".into(),
                frame_rate: tl::FrameRate::from_f64(fps).unwrap_or(tl::FrameRate::FPS_30),
                width: w,
                height: h,
                video_tracks: video_names.len() as u32,
                audio_tracks: audio_names.len() as u32,
                ..Default::default()
            })
            .expect("new sequence");
        let project = ed.project().clone();
        let s = project.sequence(seq).expect("seq");
        let vids: Vec<TrackId> = s.video_tracks().iter().map(|t| t.id).collect();
        let auds: Vec<TrackId> = s.audio_tracks().iter().map(|t| t.id).collect();
        for (t, n) in vids.iter().zip(video_names) {
            let _ = ed.update_track(seq, *t, TrackUpdate { name: Some(n.into()), ..Default::default() });
        }
        for (t, n) in auds.iter().zip(audio_names) {
            let _ = ed.update_track(seq, *t, TrackUpdate { name: Some(n.into()), ..Default::default() });
        }
        // Setup edits are not something to undo.
        let project = ed.project().clone();
        let mut ed = Editor::from_project(project);
        ed.mark_saved();
        let mut s = Session::new(ed, seq, None);
        s.aspect = aspect;
        s
    }

    pub fn open(path: &Path) -> Res<Session> {
        let p = e2s(Project::load(path))?;
        let ed = Editor::from_project(p);
        let seq = ed
            .project()
            .sequences()
            .values()
            .next()
            .map(|s| s.id)
            .ok_or_else(|| "That project has no sequence.".to_string())?;
        Ok(Session::new(ed, seq, Some(path.to_path_buf())))
    }

    pub fn project(&self) -> &Project {
        self.editor.project()
    }

    pub fn sequence(&self) -> &tl::Sequence {
        self.project().sequence(self.seq).expect("sequence exists")
    }

    pub fn name(&self) -> String {
        self.project().name.clone()
    }

    pub fn fps(&self) -> f64 {
        self.sequence().frame_rate.as_f64()
    }

    pub fn frame_rate(&self) -> tl::FrameRate {
        self.sequence().frame_rate
    }

    pub fn frame_secs(&self) -> f64 {
        1.0 / self.fps()
    }

    pub fn duration(&self) -> f64 {
        self.sequence().duration().as_secs_f64()
    }

    pub fn size(&self) -> (u32, u32) {
        let s = self.sequence();
        (s.width, s.height)
    }

    pub fn snap_time(&self, secs: f64) -> Time {
        self.sequence().snap(Time::from_secs_f64(secs.max(0.0)))
    }

    pub fn playhead_time(&self) -> Time {
        self.snap_time(self.playhead)
    }

    pub fn frame_index(&self) -> i64 {
        Time::from_secs_f64(self.playhead).to_frames_round(self.frame_rate())
    }

    pub fn is_dirty(&self) -> bool {
        self.editor.is_dirty()
    }

    pub fn bump(&mut self) {
        self.revision += 1;
    }

    pub fn save(&mut self, path: &Path) -> Res<()> {
        e2s(self.project().save(path))?;
        self.path = Some(path.to_path_buf());
        self.editor.mark_saved();
        self.last_saved_revision = self.revision;
        Ok(())
    }

    pub fn save_recovery(&self, dir: &Path) -> Res<PathBuf> {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let safe: String = self.name().chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
        let file = dir.join(format!("{safe}-{stamp}.originfx"));
        e2s(self.project().save(&file))?;
        // keep the newest 8 per project
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with(&format!("{safe}-")))
                    .collect()
            })
            .unwrap_or_default();
        files.sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));
        for old in files.into_iter().skip(8) {
            let _ = std::fs::remove_file(old.path());
        }
        Ok(file)
    }

    // ----- lookups --------------------------------------------------------------

    pub fn find_clip(&self, id: ClipId) -> Option<(&tl::Track, &tl::Clip)> {
        self.sequence().find_clip(id)
    }

    pub fn selected_clip(&self) -> Option<(&tl::Track, &tl::Clip)> {
        self.selected.and_then(|id| self.find_clip(id))
    }

    pub fn first_video_track(&self) -> TrackId {
        self.sequence().video_tracks()[0].id
    }

    pub fn first_audio_track(&self) -> Option<TrackId> {
        self.sequence().audio_tracks().iter().next().map(|t| t.id)
    }

    pub fn track_at_index(&self, kind: TrackKind, i: usize) -> Option<TrackId> {
        self.sequence().tracks_of(kind).get(i).map(|t| t.id)
    }

    /// The clip under the playhead on the selected or first video track.
    pub fn clip_at_playhead(&self) -> Option<ClipId> {
        let t = self.playhead_time();
        let seq = self.sequence();
        if let Some(tid) = self.selected_track {
            if let Some(track) = seq.track(tid) {
                if let Some(c) = track.clip_at(t) {
                    return Some(c.id);
                }
            }
        }
        for track in seq.video_tracks().iter().chain(seq.audio_tracks().iter()) {
            if let Some(c) = track.clip_at(t) {
                return Some(c.id);
            }
        }
        None
    }

    pub fn ensure_selection(&mut self) -> Option<ClipId> {
        if self.selected.is_some() && self.selected_clip().is_some() {
            return self.selected;
        }
        self.selected = self.clip_at_playhead();
        self.selected
    }

    pub fn prev_clip(&self, id: ClipId) -> Option<ClipId> {
        let (track, clip) = self.find_clip(id)?;
        track.clip_before(clip.start).filter(|c| c.end() == clip.start).map(|c| c.id)
    }

    pub fn next_clip(&self, id: ClipId) -> Option<ClipId> {
        let (track, clip) = self.find_clip(id)?;
        track.clip_from(clip.end()).filter(|c| c.start == clip.end()).map(|c| c.id)
    }

    // ----- editing ops ---------------------------------------------------------

    pub fn import_asset(&mut self, asset: tl::NewAsset) -> Res<AssetId> {
        let id = e2s(self.editor.add_asset(asset))?;
        self.bump();
        Ok(id)
    }

    pub fn insert_asset(&mut self, asset: AssetId, at: f64, video_track: Option<TrackId>, audio_track: Option<TrackId>) -> Res<()> {
        let seq = self.seq;
        let a = self.project().asset(asset).ok_or("No such media")?.clone();
        let vt = if a.has_video() { video_track.or(Some(self.first_video_track())) } else { None };
        let at_track = audio_track.or_else(|| self.first_audio_track());
        let at_t = if a.has_audio() { at_track } else { None };
        let start = self.snap_time(at);
        let dur = if a.is_generated() || a.is_still() { Some(Time::from_secs_f64(5.0)) } else { None };
        let placed = e2s(self.editor.place_asset(seq, asset, start, Time::ZERO, dur, vt, at_t, PlaceMode::Overwrite))?;
        self.selected = placed.video.or(placed.audio);
        self.bump();
        Ok(())
    }

    pub fn split_at_playhead(&mut self) -> Res<()> {
        let t = self.playhead_time();
        let seq = self.seq;
        if let Some(id) = self.selected {
            if let Some((_, c)) = self.find_clip(id) {
                if c.start < t && t < c.end() {
                    let (_, right) = e2s(self.editor.split_clip(seq, id, t, LinkMode::Follow))?;
                    self.selected = Some(right);
                    self.bump();
                    return Ok(());
                }
            }
        }
        let cuts = e2s(self.editor.split_all(seq, t, None))?;
        if cuts.is_empty() {
            return Err("Nothing to cut at the playhead.".into());
        }
        self.bump();
        Ok(())
    }

    pub fn delete_selected(&mut self, ripple: bool) -> Res<()> {
        let id = self.selected.ok_or("Select a clip first.")?;
        let seq = self.seq;
        e2s(self.editor.delete_clips(seq, &[id], if ripple { Ripple::Track } else { Ripple::Off }, LinkMode::Follow))?;
        self.selected = None;
        self.bump();
        Ok(())
    }

    pub fn move_clip(&mut self, id: ClipId, delta: f64, track_delta: i32, snapping: bool) -> Res<()> {
        let seq = self.seq;
        let mut d = Time::from_secs_f64(delta);
        if let Some((_, c)) = self.find_clip(id) {
            let new_start = (c.start + d).max(Time::ZERO);
            d = new_start - c.start;
            if snapping {
                let points = self.sequence().edit_points();
                let tol = Time::from_secs_f64(0.25);
                for p in points.into_iter().chain([self.playhead_time()]) {
                    if (new_start - p).abs() < tol {
                        d = p - c.start;
                        break;
                    }
                    let new_end = new_start + c.duration;
                    if (new_end - p).abs() < tol {
                        d = p - c.end();
                        break;
                    }
                }
            }
        }
        d = self.sequence().snap(d);
        e2s(self.editor.move_clips(seq, &[id], d, track_delta, LinkMode::Follow))?;
        self.bump();
        Ok(())
    }

    pub fn trim(&mut self, id: ClipId, edge: Edge, delta: f64, ripple: bool) -> Res<()> {
        let seq = self.seq;
        let d = self.sequence().snap(Time::from_secs_f64(delta));
        if d == Time::ZERO {
            return Ok(());
        }
        e2s(self.editor.trim(seq, id, edge, d, if ripple { Ripple::Track } else { Ripple::Off }, LinkMode::Follow))?;
        self.bump();
        Ok(())
    }

    pub fn roll(&mut self, id: ClipId, delta: f64) -> Res<()> {
        let seq = self.seq;
        let left = self.prev_clip(id).ok_or("The clip needs another clip right before it.")?;
        let d = self.sequence().snap(Time::from_secs_f64(delta));
        e2s(self.editor.roll(seq, left, id, d))?;
        self.bump();
        Ok(())
    }

    pub fn slip(&mut self, id: ClipId, delta: f64) -> Res<()> {
        let seq = self.seq;
        let d = self.sequence().snap(Time::from_secs_f64(delta));
        e2s(self.editor.slip(seq, id, d, LinkMode::Follow))?;
        self.bump();
        Ok(())
    }

    pub fn slide(&mut self, id: ClipId, delta: f64) -> Res<()> {
        let seq = self.seq;
        let d = self.sequence().snap(Time::from_secs_f64(delta));
        e2s(self.editor.slide(seq, id, d))?;
        self.bump();
        Ok(())
    }

    pub fn set_speed(&mut self, id: ClipId, percent: f64, reverse: bool) -> Res<()> {
        let seq = self.seq;
        let sp = Speed::from_f64((percent / 100.0).clamp(0.01, 100.0)).ok_or("That speed is out of range.")?;
        e2s(self.editor.set_speed(seq, id, sp, SpeedMode::KeepMedia, Ripple::Off, LinkMode::Follow))?;
        if let Some((_, c)) = self.find_clip(id) {
            if c.reverse != reverse {
                e2s(self.editor.set_reverse(seq, id, reverse, LinkMode::Follow))?;
            }
        }
        self.bump();
        Ok(())
    }

    pub fn set_transition(&mut self, id: ClipId, kind: Option<i32>, secs: f64, align: i32) -> Res<()> {
        let seq = self.seq;
        let fr = self.frame_rate();
        let t = kind.map(|k| Transition {
            kind: transition_kind(k),
            duration: Time::from_frames((secs * fr.as_f64()).round().max(1.0) as i64, fr),
            alignment: match align {
                0 => Alignment::StartAtCut,
                2 => Alignment::EndAtCut,
                _ => Alignment::Center,
            },
        });
        e2s(self.editor.set_transition(seq, id, t, LinkMode::Follow))?;
        self.bump();
        Ok(())
    }

    pub fn set_clip_audio(&mut self, id: ClipId, change: ClipAudio) -> Res<()> {
        let seq = self.seq;
        e2s(self.editor.set_clip_audio(seq, &[id], change, LinkMode::Follow))?;
        self.bump();
        Ok(())
    }

    pub fn set_look(&mut self, id: ClipId, param: Param, value: f64) -> Res<()> {
        let seq = self.seq;
        let (lo, hi) = param.range();
        e2s(self.editor.set_look_value(seq, id, param, value.clamp(lo, hi)))?;
        self.bump();
        Ok(())
    }

    pub fn set_blend(&mut self, id: ClipId, i: i32) -> Res<()> {
        let seq = self.seq;
        let mode = BLENDS.get(i.max(0) as usize).copied().unwrap_or(tl::BlendMode::Normal);
        e2s(self.editor.set_blend_mode(seq, id, mode))?;
        self.bump();
        Ok(())
    }

    pub fn set_mask(&mut self, id: ClipId, shape: i32, invert: bool, feather: f64, x: f64, y: f64, w: f64, h: f64) -> Res<()> {
        let seq = self.seq;
        let sh = if shape == 0 { tl::MaskShape::Rect } else { tl::MaskShape::Ellipse };
        e2s(self.editor.set_mask(seq, id, sh, invert))?;
        for (p, v) in [
            (Param::MaskX, x),
            (Param::MaskY, y),
            (Param::MaskWidth, w),
            (Param::MaskHeight, h),
            (Param::MaskFeather, feather),
        ] {
            let (lo, hi) = p.range();
            e2s(self.editor.set_look_value(seq, id, p, v.clamp(lo, hi)))?;
        }
        self.bump();
        Ok(())
    }

    pub fn remove_mask(&mut self, id: ClipId) -> Res<()> {
        let seq = self.seq;
        e2s(self.editor.remove_mask(seq, id))?;
        self.bump();
        Ok(())
    }

    pub fn add_effect(&mut self, id: ClipId, kind: EffectKind) -> Res<tl::EffectId> {
        let seq = self.seq;
        let r = e2s(self.editor.add_effect(seq, id, kind, None))?;
        self.bump();
        Ok(r)
    }

    pub fn remove_effect(&mut self, id: ClipId, fx: tl::EffectId) -> Res<()> {
        let seq = self.seq;
        e2s(self.editor.remove_effect(seq, id, fx))?;
        self.bump();
        Ok(())
    }

    pub fn set_effect_enabled(&mut self, id: ClipId, fx: tl::EffectId, on: bool) -> Res<()> {
        let seq = self.seq;
        e2s(self.editor.set_effect_enabled(seq, id, fx, on))?;
        self.bump();
        Ok(())
    }

    pub fn set_effect_value(&mut self, id: ClipId, fx: tl::EffectId, name: &str, value: f64) -> Res<()> {
        let seq = self.seq;
        e2s(self.editor.set_effect_value(seq, id, fx, name, value))?;
        self.bump();
        Ok(())
    }

    pub fn move_effect(&mut self, id: ClipId, fx: tl::EffectId, to: usize) -> Res<()> {
        let seq = self.seq;
        e2s(self.editor.move_effect(seq, id, fx, to))?;
        self.bump();
        Ok(())
    }

    /// Find or add a one-setting colour effect on the clip and set its value.
    pub fn set_colour_effect(&mut self, id: ClipId, kind: EffectKind, value: f64) -> Res<()> {
        let seq = self.seq;
        let existing = self
            .find_clip(id)
            .and_then(|(_, c)| c.effects.iter().find(|e| e.kind == kind && e.plugin.is_none()).map(|e| e.id));
        let neutral = kind.params().first().map(|p| p.neutral).unwrap_or(0.0);
        let param = kind.params().first().map(|p| p.name).unwrap_or("amount");
        match existing {
            Some(fx) => {
                if (value - neutral).abs() < 1e-6 {
                    e2s(self.editor.remove_effect(seq, id, fx))?;
                } else {
                    e2s(self.editor.set_effect_value(seq, id, fx, param, value))?;
                }
            }
            None => {
                if (value - neutral).abs() >= 1e-6 {
                    let fx = e2s(self.editor.add_effect(seq, id, kind, None))?;
                    e2s(self.editor.set_effect_value(seq, id, fx, param, value))?;
                }
            }
        }
        self.bump();
        Ok(())
    }

    pub fn colour_value(&self, id: ClipId, kind: EffectKind) -> f64 {
        let neutral = kind.params().first().map(|p| p.neutral).unwrap_or(0.0);
        self.find_clip(id)
            .and_then(|(_, c)| c.effects.iter().find(|e| e.kind == kind && e.plugin.is_none()).map(|e| e.value(0)))
            .unwrap_or(neutral)
    }

    pub fn set_keyframe(&mut self, id: ClipId, param: Param, t: f64, value: f64) -> Res<()> {
        let seq = self.seq;
        let time = self.sequence().snap(Time::from_secs_f64(t.max(0.0)));
        let (lo, hi) = param.range();
        e2s(self.editor.set_keyframe(seq, Target::Clip(id), param, time, value.clamp(lo, hi), Interp::Smooth, LinkMode::Ignore))?;
        self.bump();
        Ok(())
    }

    pub fn remove_keyframe(&mut self, id: ClipId, param: Param, t: f64) -> Res<()> {
        let seq = self.seq;
        let time = self.sequence().snap(Time::from_secs_f64(t.max(0.0)));
        e2s(self.editor.remove_keyframe(seq, Target::Clip(id), param, time, LinkMode::Ignore))?;
        self.bump();
        Ok(())
    }

    pub fn clear_animation(&mut self, id: ClipId, param: Param) -> Res<()> {
        let seq = self.seq;
        e2s(self.editor.clear_animation(seq, Target::Clip(id), param, LinkMode::Ignore))?;
        self.bump();
        Ok(())
    }

    pub fn add_marker(&mut self) -> Res<()> {
        let seq = self.seq;
        let t = self.playhead_time();
        let n = self.sequence().markers().len() + 1;
        e2s(self.editor.add_marker(seq, t, Time::ZERO, format!("Marker {n}"), MarkerColour::Yellow))?;
        self.bump();
        Ok(())
    }

    pub fn remove_marker(&mut self, id: MarkerId) -> Res<()> {
        let seq = self.seq;
        e2s(self.editor.remove_marker(seq, id))?;
        self.bump();
        Ok(())
    }

    pub fn update_track(&mut self, id: TrackId, up: TrackUpdate) -> Res<()> {
        let seq = self.seq;
        e2s(self.editor.update_track(seq, id, up))?;
        self.bump();
        Ok(())
    }

    pub fn add_track(&mut self, kind: TrackKind) -> Res<()> {
        let seq = self.seq;
        e2s(self.editor.add_track(seq, kind, None))?;
        self.bump();
        Ok(())
    }

    pub fn set_master_gain(&mut self, db: f64) -> Res<()> {
        let seq = self.seq;
        let g = Gain::from_db(db.clamp(-120.0, 24.0)).ok_or("Bad level")?;
        e2s(self.editor.set_master_gain(seq, g))?;
        self.bump();
        Ok(())
    }

    pub fn set_enabled(&mut self, id: ClipId, on: bool) -> Res<()> {
        let seq = self.seq;
        e2s(self.editor.set_enabled(seq, &[id], on, LinkMode::Follow))?;
        self.bump();
        Ok(())
    }

    pub fn rename_clip(&mut self, id: ClipId, name: &str) -> Res<()> {
        let seq = self.seq;
        e2s(self.editor.rename_clip(seq, id, name))?;
        self.bump();
        Ok(())
    }

    pub fn undo(&mut self) -> Res<String> {
        let r = e2s(self.editor.undo())?;
        self.bump();
        if self.selected.is_some() && self.selected_clip().is_none() {
            self.selected = None;
        }
        Ok(r)
    }

    pub fn redo(&mut self) -> Res<String> {
        let r = e2s(self.editor.redo())?;
        self.bump();
        if self.selected.is_some() && self.selected_clip().is_none() {
            self.selected = None;
        }
        Ok(r)
    }

    /// Change the sequence frame size / rate by rebuilding the sequence with the same tracks.
    pub fn set_sequence_settings(&mut self, w: u32, h: u32, fps: f64, aspect: i32) -> Res<()> {
        let mut project = self.project().clone();
        let s = project.sequences().get(&self.seq).ok_or("No sequence")?.clone();
        let rate = tl::FrameRate::from_f64(fps).ok_or("Bad frame rate")?;
        let mut json: serde_json::Value = e2s(serde_json::from_str(&e2s(project.to_json())?))?;
        // The file format keeps sequence settings in plain fields; patch them in place.
        if let Some(seqs) = json.get_mut("sequences").and_then(|v| v.as_array_mut()) {
            for sq in seqs.iter_mut() {
                if sq.get("id").and_then(|v| v.as_u64()) == Some(s.id.0 as u64) {
                    sq["width"] = serde_json::json!(w);
                    sq["height"] = serde_json::json!(h);
                    sq["frame_rate"] = serde_json::json!([rate.num(), rate.den()]);
                }
            }
        }
        project = e2s(Project::from_json(&e2s(serde_json::to_string(&json))?))?;
        let _ = s;
        let sel = self.selected;
        self.editor = Editor::from_project(project);
        self.selected = sel;
        self.aspect = aspect;
        self.bump();
        Ok(())
    }

    // ----- UI sync --------------------------------------------------------------

    pub fn asset_kind(a: &tl::Asset) -> i32 {
        if let Some(g) = &a.generator {
            return match g {
                tl::Generator::Text(_) => 3,
                _ => 4,
            };
        }
        if a.has_video() {
            if a.is_still() {
                2
            } else {
                0
            }
        } else {
            1
        }
    }

    pub fn asset_meta(a: &tl::Asset) -> String {
        if let Some(v) = a.video.first() {
            let fps = v.frame_rate.map(|r| format!(" · {} fps", r)).unwrap_or_default();
            format!("{}×{}{}", v.width, v.height, fps)
        } else if let Some(au) = a.audio.first() {
            format!("{} Hz · {}", au.sample_rate, if au.channels == 1 { "Mono" } else { "Stereo" })
        } else {
            "Graphic".into()
        }
    }

    fn clip_item(&mut self, track_kind: TrackKind, c: &tl::Clip) -> ClipItem {
        let asset = self.project().asset(c.asset).cloned();
        let kind = match (&asset, track_kind) {
            (_, TrackKind::Audio) => 1,
            (Some(a), _) if matches!(a.generator, Some(tl::Generator::Text(_))) => 2,
            (Some(a), _) if a.is_generated() => 3,
            _ => 0,
        };
        let offline = asset.as_ref().map(|a| !a.is_generated() && !a.path.exists()).unwrap_or(true);
        let mut waveform = Image::default();
        let mut has_waveform = false;
        if track_kind == TrackKind::Audio {
            if let Some(img) = self.waveform_image(c) {
                waveform = img;
                has_waveform = true;
            }
        }
        ClipItem {
            id: c.id.0 as i32,
            name: c.name.clone().into(),
            start: c.start.as_secs_f64() as f32,
            duration: c.duration.as_secs_f64() as f32,
            kind,
            selected: self.selected == Some(c.id),
            enabled: c.enabled,
            transition_secs: c.transition_in.map(|t| t.duration.as_secs_f64() as f32).unwrap_or(0.0),
            waveform,
            has_waveform,
            speed: c.speed.as_f64() as f32,
            asset: c.asset.0 as i32,
            offline,
        }
    }

    fn waveform_image(&mut self, c: &tl::Clip) -> Option<Image> {
        let peaks = self.peaks.get(&c.asset)?.clone();
        let speed_key = (c.speed.as_f64() * 1000.0) as u32;
        let key = (c.asset, c.source_in.ticks(), c.duration.ticks(), speed_key);
        if let Some(img) = self.waveforms.get(&key) {
            return Some(img.clone());
        }
        let secs = c.duration.as_secs_f64();
        let w = ((secs * 40.0).round() as u32).clamp(4, 2400);
        let h = 20u32;
        let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
        let px = buf.make_mut_bytes();
        let src_len = secs * c.speed.as_f64();
        let start = c.source_in.as_secs_f64();
        for x in 0..w {
            let t0 = start + src_len * x as f64 / w as f64;
            let t1 = start + src_len * (x + 1) as f64 / w as f64;
            let i0 = (t0 * peaks.per_second) as usize;
            let i1 = ((t1 * peaks.per_second) as usize).max(i0 + 1);
            let mut lo = 0.0f32;
            let mut hi = 0.0f32;
            for i in i0..i1.min(peaks.data.len()) {
                lo = lo.min(peaks.data[i].0);
                hi = hi.max(peaks.data[i].1);
            }
            let amp = hi.max(-lo).clamp(0.0, 1.0);
            let bar = ((amp * (h as f32 - 2.0)).round() as u32).max(if amp > 0.001 { 1 } else { 0 });
            let top = (h - bar) / 2;
            for y in top..(top + bar).min(h) {
                let i = ((y * w + x) * 4) as usize;
                px[i] = 255;
                px[i + 1] = 255;
                px[i + 2] = 255;
                px[i + 3] = 255;
            }
        }
        let img = Image::from_rgba8(buf);
        if self.waveforms.len() > 400 {
            self.waveforms.clear();
        }
        self.waveforms.insert(key, img.clone());
        Some(img)
    }

    pub fn selected_item(&self) -> SelectedClip {
        let mut s = SelectedClip::default();
        s.transition = -1;
        let Some((track, c)) = self.selected_clip() else { return s };
        let asset = self.project().asset(c.asset);
        s.valid = true;
        s.id = c.id.0 as i32;
        s.name = c.name.clone().into();
        s.kind = match (asset, track.kind) {
            (_, TrackKind::Audio) => 1,
            (Some(a), _) if matches!(a.generator, Some(tl::Generator::Text(_))) => 2,
            (Some(a), _) if a.is_generated() => 3,
            _ => 0,
        };
        s.start = c.start.as_secs_f64() as f32;
        s.duration = c.duration.as_secs_f64() as f32;
        s.source_in = c.source_in.as_secs_f64() as f32;
        s.speed = c.speed.as_f64() as f32;
        s.reverse = c.reverse;
        s.volume = c.gain.db();
        s.pan = c.pan.value() * 100.0;
        s.opacity = c.transform.opacity as f32;
        s.pos_x = c.transform.x as f32;
        s.pos_y = c.transform.y as f32;
        s.scale = c.transform.scale_x as f32;
        s.rotation = c.transform.rotation as f32;
        s.blend = BLENDS.iter().position(|b| *b == c.blend).unwrap_or(0) as i32;
        if let Some(m) = &c.mask {
            s.has_mask = true;
            s.mask_shape = if m.shape == tl::MaskShape::Rect { 0 } else { 1 };
            s.mask_invert = m.invert;
            s.mask_feather = m.feather as f32;
            s.mask_x = m.x as f32;
            s.mask_y = m.y as f32;
            s.mask_w = m.width as f32;
            s.mask_h = m.height as f32;
        } else {
            s.mask_shape = 1;
            s.mask_x = 50.0;
            s.mask_y = 50.0;
            s.mask_w = 60.0;
            s.mask_h = 60.0;
            s.mask_feather = 10.0;
        }
        if let Some(t) = c.transition_in {
            s.transition = transition_index(t.kind);
            s.transition_secs = t.duration.as_secs_f64() as f32;
            s.transition_align = match t.alignment {
                Alignment::StartAtCut => 0,
                Alignment::Center => 1,
                Alignment::EndAtCut => 2,
            };
        } else {
            s.transition_secs = 1.0;
            s.transition_align = 1;
        }
        s.fade_in = c.fade_in.map(|f| f.duration.as_secs_f64() as f32).unwrap_or(0.0);
        s.fade_out = c.fade_out.map(|f| f.duration.as_secs_f64() as f32).unwrap_or(0.0);
        if let Some(a) = asset {
            s.asset_name = a.name.clone().into();
            let what = if a.has_video() && a.has_audio() {
                "Video and audio"
            } else if a.has_video() {
                if a.is_generated() { "Graphic" } else { "Video" }
            } else {
                "Audio"
            };
            s.asset_meta = format!("{} · {}", util::short_duration(c.duration.as_secs_f64()), what).into();
            if let Some(tl::Generator::Text(t)) = &a.generator {
                s.is_text = true;
                let mut lines = t.text.splitn(2, '\n');
                s.text = lines.next().unwrap_or("").into();
                s.text_sub = lines.next().unwrap_or("").into();
                s.text_size = t.size_pct as f32;
                s.text_align = match t.align {
                    tl::Align::Left => 0,
                    tl::Align::Centre => 1,
                    tl::Align::Right => 2,
                };
                s.text_colour = colour_index(t.colour);
            }
        }
        let effects: Vec<EffectItem> = c
            .effects
            .iter()
            .map(|e| {
                let params: Vec<ParamItem> = e
                    .settings()
                    .iter()
                    .enumerate()
                    .map(|(i, p)| ParamItem {
                        name: p.name.clone().into(),
                        unit: p.unit.clone().into(),
                        value: e.value(i) as f32,
                        min: p.min as f32,
                        max: p.max as f32,
                        default: p.default as f32,
                        keyed: e.is_animated(i),
                    })
                    .collect();
                EffectItem {
                    id: e.id.0 as i32,
                    name: match &e.plugin {
                        Some(p) => p.name.clone().into(),
                        None => effect_label(e.kind).into(),
                    },
                    kind: EffectKind::ALL.iter().position(|k| *k == e.kind).unwrap_or(0) as i32,
                    enabled: e.enabled,
                    params: ModelRc::new(VecModel::from(params)),
                }
            })
            .collect();
        s.effects = ModelRc::new(VecModel::from(effects));
        let mut keys = Vec::new();
        for (param, curve) in c.anim.iter() {
            let pi = KF_PARAMS.iter().position(|p| *p == param).map(|i| i as i32).unwrap_or(match param {
                Param::PositionY => 0,
                Param::ScaleY => 1,
                _ => -1,
            });
            if pi < 0 {
                continue;
            }
            for k in curve.keys() {
                keys.push(KeyItem {
                    param: pi,
                    time: Time::from_ticks(k.t).as_secs_f64() as f32,
                    value: k.value as f32,
                    interp: match k.interp {
                        Interp::Hold => 0,
                        Interp::Linear => 1,
                        _ => 2,
                    },
                });
            }
        }
        s.keys = ModelRc::new(VecModel::from(keys));
        if let Some(p) = self.prev_clip(c.id) {
            s.has_prev = true;
            s.prev_id = p.0 as i32;
            s.prev_name = self.find_clip(p).map(|(_, c)| c.name.clone()).unwrap_or_default().into();
        }
        s.next_name = self.next_clip(c.id).and_then(|n| self.find_clip(n)).map(|(_, c)| c.name.clone()).unwrap_or_default().into();
        s
    }

    /// Push the whole project state to the UI.
    pub fn sync(&mut self, ui: &App, timecode_seconds: bool) {
        let m = ui.global::<Model>();
        let fps = self.fps();
        let (w, h) = self.size();
        m.set_has_project(true);
        m.set_project_name(self.name().into());
        m.set_project_path(self.path.as_ref().map(|p| crate::settings::shorten_home(p)).unwrap_or_default().into());
        m.set_dirty(self.is_dirty());
        m.set_seq_width(w as i32);
        m.set_seq_height(h as i32);
        m.set_fps(fps as f32);
        m.set_fps_text(format!("{}", self.frame_rate()).into());
        m.set_aspect(self.aspect);
        let dur = self.duration();
        m.set_duration(dur as f32);
        m.set_duration_text(util::timecode(dur, fps, timecode_seconds).into());
        m.set_playhead(self.playhead as f32);
        m.set_playhead_text(util::timecode(self.playhead, fps, timecode_seconds).into());
        m.set_in_point(self.in_point.map(|v| v as f32).unwrap_or(-1.0));
        m.set_out_point(self.out_point.map(|v| v as f32).unwrap_or(-1.0));
        m.set_can_undo(self.editor.can_undo());
        m.set_can_redo(self.editor.can_redo());
        m.set_undo_label(self.editor.undo_label().unwrap_or("").into());
        m.set_redo_label(self.editor.redo_label().unwrap_or("").into());
        m.set_master_gain(self.sequence().master_gain.db());
        m.set_selected_track(self.selected_track.map(|t| t.0 as i32).unwrap_or(-1));

        // assets
        let inspector_asset = m.get_inspector_asset();
        let assets: Vec<AssetItem> = self
            .project()
            .assets()
            .values()
            .map(|a| {
                let secs = a.duration.map(|d| d.as_secs_f64()).unwrap_or(0.0);
                let thumb = self.thumbs.get(&a.id).cloned();
                AssetItem {
                    id: a.id.0 as i32,
                    name: a.name.clone().into(),
                    path: a.path.to_string_lossy().to_string().into(),
                    kind: Self::asset_kind(a),
                    duration: if a.duration.is_some() { util::short_duration(secs).into() } else { "Still".into() },
                    secs: secs as f32,
                    meta: Self::asset_meta(a).into(),
                    has_thumb: thumb.is_some(),
                    thumb: thumb.unwrap_or_default(),
                    has_video: a.has_video(),
                    has_audio: a.has_audio(),
                    proxy: 0,
                    proxy_text: "".into(),
                    selected: inspector_asset == a.id.0 as i32,
                }
            })
            .collect();
        let names: Vec<SharedString> = assets.iter().map(|a| a.name.clone()).collect();
        refresh_model(m.get_assets(), assets, |v| m.set_assets(v));
        m.set_clip_names(ModelRc::new(VecModel::from(names)));

        // tracks
        let seq = self.sequence().clone();
        let mut vt = Vec::new();
        for t in seq.video_tracks().iter() {
            let clips: Vec<ClipItem> = t.clips().map(|c| self.clip_item(TrackKind::Video, c)).collect();
            vt.push(TrackItem {
                id: t.id.0 as i32,
                name: t.name.clone().into(),
                kind: 0,
                enabled: t.enabled,
                locked: t.locked,
                solo: t.solo,
                gain: t.gain.db(),
                pan: t.pan.value() * 100.0,
                level: 0.0,
                clips: ModelRc::new(VecModel::from(clips)),
            });
        }
        let mut at = Vec::new();
        for t in seq.audio_tracks().iter() {
            let clips: Vec<ClipItem> = t.clips().map(|c| self.clip_item(TrackKind::Audio, c)).collect();
            at.push(TrackItem {
                id: t.id.0 as i32,
                name: t.name.clone().into(),
                kind: 1,
                enabled: t.enabled,
                locked: t.locked,
                solo: t.solo,
                gain: t.gain.db(),
                pan: t.pan.value() * 100.0,
                level: 0.0,
                clips: ModelRc::new(VecModel::from(clips)),
            });
        }
        refresh_model(m.get_video_tracks(), vt, |v| m.set_video_tracks(v));
        refresh_model(m.get_audio_tracks(), at, |v| m.set_audio_tracks(v));

        let markers: Vec<MarkerItem> = seq
            .markers()
            .iter()
            .map(|mk| MarkerItem {
                id: mk.id.0 as i32,
                time: mk.time.as_secs_f64() as f32,
                name: mk.name.clone().into(),
                colour: match mk.colour {
                    MarkerColour::Green => 1,
                    MarkerColour::Red => 2,
                    _ => 0,
                },
            })
            .collect();
        m.set_markers(ModelRc::new(VecModel::from(markers)));

        // layers at the playhead (video clips, top first)
        let t = self.playhead_time();
        let mut layer_names = Vec::new();
        let mut layer_ids = Vec::new();
        for track in seq.video_tracks().iter().rev() {
            if let Some(c) = track.clip_at(t) {
                layer_names.push(SharedString::from(c.name.as_str()));
                layer_ids.push(c.id.0 as i32);
            }
        }
        m.set_layer_names(ModelRc::new(VecModel::from(layer_names)));
        m.set_layer_ids(ModelRc::new(VecModel::from(layer_ids)));

        let sel = self.selected_item();
        if sel.valid {
            let id = ClipId(sel.id as u64);
            m.set_temperature(self.colour_value(id, EffectKind::Temperature) as f32);
            m.set_tint(self.colour_value(id, EffectKind::Hue) as f32);
            m.set_exposure(self.colour_value(id, EffectKind::Exposure) as f32);
            m.set_contrast(self.colour_value(id, EffectKind::Contrast) as f32);
            m.set_saturation(self.colour_value(id, EffectKind::Saturation) as f32);
        }
        m.set_selected(sel);
    }

    /// Update just the time readouts (cheap, during playback).
    pub fn sync_time(&self, ui: &App, timecode_seconds: bool) {
        let m = ui.global::<Model>();
        m.set_playhead(self.playhead as f32);
        m.set_playhead_text(util::timecode(self.playhead, self.fps(), timecode_seconds).into());
    }

    pub fn aspect_name(&self) -> String {
        let (w, h) = self.size();
        util::aspect_name(w, h).to_string()
    }
}

pub const TEXT_COLOURS: [(&str, [u8; 3]); 6] = [
    ("White", [255, 255, 255]),
    ("Blue", [46, 140, 255]),
    ("Gold", [217, 169, 46]),
    ("Black", [8, 9, 13]),
    ("Red", [229, 72, 77]),
    ("Green", [34, 196, 122]),
];

pub fn colour_index(c: tl::Colour) -> i32 {
    TEXT_COLOURS
        .iter()
        .position(|(_, rgb)| rgb[0] == c.r && rgb[1] == c.g && rgb[2] == c.b)
        .map(|i| i as i32)
        .unwrap_or(0)
}

pub fn colour_of(i: i32) -> tl::Colour {
    let rgb = TEXT_COLOURS.get(i.max(0) as usize).map(|c| c.1).unwrap_or([255, 255, 255]);
    tl::Colour { r: rgb[0], g: rgb[1], b: rgb[2], a: 255 }
}

/// A convenience so screens can borrow the shared session.
pub type Shared = Rc<std::cell::RefCell<Option<Session>>>;

pub fn fade(secs: f64) -> Option<Fade> {
    if secs <= 0.0 {
        None
    } else {
        Some(Fade { duration: Time::from_secs_f64(secs), curve: Default::default() })
    }
}

pub fn gain_db(db: f64) -> Option<Gain> {
    Gain::from_db(db.clamp(-120.0, 24.0))
}

pub fn pan_pct(p: f64) -> Option<Pan> {
    Pan::new(p.clamp(-100.0, 100.0).round() as i16)
}

pub fn asset_spec(asset: AssetId, start: Time) -> ClipSpec {
    ClipSpec::new(asset, start)
}

/// Update a list model in place when the row count is unchanged, so the
/// widgets built from it (and any click or drag in progress) survive a refresh.
fn refresh_model<T: Clone + PartialEq + 'static>(old: ModelRc<T>, new: Vec<T>, set: impl FnOnce(ModelRc<T>)) {
    use slint::Model as _;
    if old.row_count() == new.len() && !new.is_empty() {
        for (i, row) in new.into_iter().enumerate() {
            if old.row_data(i).as_ref() != Some(&row) {
                old.set_row_data(i, row);
            }
        }
    } else {
        set(ModelRc::new(VecModel::from(new)));
    }
}
