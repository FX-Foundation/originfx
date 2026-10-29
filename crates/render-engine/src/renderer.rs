// SPDX-License-Identifier: GPL-3.0-or-later
//! The [`Renderer`]: composites the picture and mixes the sound of one sequence.

use std::collections::HashMap;
use std::sync::Arc;

use media_engine::{AudioBuffer, VideoFrame};
use timeline_engine::{
    Asset, AssetId, Clip, Param, Project, Sequence, SequenceId, Time, TimeRange,
    MAX_TRANSITION_SECS,
};

use crate::composite::{blend_over, placement, solid, Fit};
use crate::error::{Error, Result};
use crate::scale::resize;
use crate::source::MediaSource;
use crate::transition::{blend, fade_in_gain, fade_out_gain};
use audio_engine::Limiter;

/// What to do when a clip's media file is not on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissingMedia {
    /// Stop with [`Error::MissingMedia`]. The right choice for a final export.
    #[default]
    Fail,
    /// Show a flat magenta "media offline" picture and silence, so the rest can still be previewed.
    Offline,
}

/// The colour shown for media that is offline.
pub const OFFLINE_COLOUR: [u8; 4] = [255, 0, 255, 255];

/// Choices that change how a sequence looks when rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderSettings {
    /// How pictures of another shape are fitted into the frame.
    pub fit: Fit,
    /// What is behind the bottom layer. Opaque black by default.
    pub background: [u8; 4],
    pub missing_media: MissingMedia,
    /// How the final mix is kept within full scale. Hard clipping by default.
    pub limiter: Limiter,
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings {
            fit: Fit::Contain,
            background: [0, 0, 0, 255],
            missing_media: MissingMedia::Fail,
            limiter: Limiter::HardClip,
        }
    }
}

/// Renders one sequence of a project. It works on a snapshot of the project taken when it was
/// created, so edits made afterwards do not change a render that is under way.
pub struct Renderer<S: MediaSource> {
    project: Project,
    seq: SequenceId,
    source: S,
    settings: RenderSettings,
    /// Scaled copies of still images, which would otherwise be resized again for every frame.
    stills: HashMap<(AssetId, u32, usize, usize), Arc<Vec<u8>>>,
}

impl<S: MediaSource> Renderer<S> {
    pub fn new(
        project: &Project,
        seq: SequenceId,
        source: S,
        settings: RenderSettings,
    ) -> Result<Self> {
        if project.sequence(seq).is_none() {
            return Err(Error::NoSuchSequence(seq));
        }
        Ok(Renderer {
            project: project.clone(),
            seq,
            source,
            settings,
            stills: HashMap::new(),
        })
    }

    pub fn sequence(&self) -> &Sequence {
        self.project.sequence(self.seq).expect("checked in new")
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Give the media source back (to reuse its open files for another render).
    pub fn into_source(self) -> S {
        self.source
    }

    /// Picture size of the sequence.
    pub fn size(&self) -> (u32, u32) {
        let s = self.sequence();
        (s.width, s.height)
    }

    /// The picture at frame number `index` (0 is the first frame).
    pub fn frame(&mut self, index: i64) -> Result<VideoFrame> {
        let rate = self.sequence().frame_rate;
        self.frame_at(Time::from_frames(index, rate))
    }

    /// The picture at time `t` (moved to the frame it falls in).
    pub fn frame_at(&mut self, t: Time) -> Result<VideoFrame> {
        let seq = self.sequence().clone();
        let t = Time::from_frames(t.to_frames_floor(seq.frame_rate), seq.frame_rate);
        let (cw, ch) = (seq.width as usize, seq.height as usize);
        let mut canvas = solid(cw, ch, self.settings.background);

        for layer in seq.video_layers_at(t) {
            match layer.transition {
                None => {
                    self.draw_clip(&mut canvas, (cw, ch), &layer.clip, layer.source_time)?;
                }
                Some(tr) => {
                    // Each side is drawn on its own transparent canvas, blended, and the result
                    // goes over what is underneath like any other layer.
                    let mut sides = Vec::with_capacity(2);
                    for side in [&tr.outgoing, &tr.incoming] {
                        let mut buf = solid(cw, ch, [0, 0, 0, 0]);
                        if let Some(side) = side {
                            self.draw_clip(&mut buf, (cw, ch), &side.clip, side.source_time)?;
                        }
                        sides.push(buf);
                    }
                    let mixed = blend(tr.kind, tr.progress, &sides[0], &sides[1], cw, ch);
                    blend_over(&mut canvas, (cw, ch), &mixed, (cw, ch), (0, 0));
                }
            }
        }
        Ok(VideoFrame {
            width: seq.width,
            height: seq.height,
            pts_secs: t.as_secs_f64(),
            data: canvas,
        })
    }

    /// Draw one clip's picture at `source_time` onto `canvas`, fitted to the frame.
    fn draw_clip(
        &mut self,
        canvas: &mut [u8],
        (cw, ch): (usize, usize),
        clip: &Clip,
        source_time: Time,
    ) -> Result<()> {
        let asset = self
            .project
            .asset(clip.asset)
            .ok_or(timeline_engine::Error::NoSuchAsset(clip.asset))?
            .clone();
        let secs = source_time.as_secs_f64();
        let frame = match self.source.video_frame(&asset, clip.stream, secs) {
            Ok(f) => f,
            Err(Error::MissingMedia { .. })
                if self.settings.missing_media == MissingMedia::Offline =>
            {
                blend_over(
                    canvas,
                    (cw, ch),
                    &solid(cw, ch, OFFLINE_COLOUR),
                    (cw, ch),
                    (0, 0),
                );
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let (sw, sh) = (frame.width as usize, frame.height as usize);
        let place = placement(sw, sh, cw, ch, self.settings.fit);
        if asset.is_still() {
            let key = (asset.id, clip.stream, place.width, place.height);
            let pixels = match self.stills.get(&key) {
                Some(p) => Arc::clone(p),
                None => {
                    let p = Arc::new(resize(&frame.data, sw, sh, place.width, place.height));
                    self.stills.insert(key, Arc::clone(&p));
                    p
                }
            };
            blend_over(
                canvas,
                (cw, ch),
                &pixels,
                (place.width, place.height),
                (place.x, place.y),
            );
        } else if (place.width, place.height) == (sw, sh) {
            blend_over(canvas, (cw, ch), &frame.data, (sw, sh), (place.x, place.y));
        } else {
            let scaled = resize(&frame.data, sw, sh, place.width, place.height);
            blend_over(
                canvas,
                (cw, ch),
                &scaled,
                (place.width, place.height),
                (place.x, place.y),
            );
        }
        Ok(())
    }

    /// The mixed sound of `range`, as stereo `f32` at the sequence's sample rate.
    pub fn audio(&mut self, range: TimeRange) -> Result<AudioBuffer> {
        let rate = self.sequence().sample_rate;
        let first = range.start.to_samples_round(rate);
        let last = range.end.to_samples_round(rate);
        let count = (last - first).max(0) as usize;
        let samples = self.audio_samples(first, count)?;
        Ok(AudioBuffer {
            sample_rate: rate,
            channels: 2,
            pts_secs: range.start.as_secs_f64(),
            samples,
        })
    }

    /// `count` stereo sample frames of the mix, starting at sequence sample number `first`.
    /// Consecutive calls join without gaps or overlaps. Levels above full scale are clipped.
    pub fn audio_samples(&mut self, first: i64, count: usize) -> Result<Vec<f32>> {
        let seq = self.sequence().clone();
        let rate = seq.sample_rate;
        let mut mix = vec![0f32; count * 2];
        if count == 0 {
            return Ok(mix);
        }
        let last = first + count as i64;
        let window = TimeRange {
            start: Time::from_samples(first, rate),
            end: Time::from_samples(last, rate),
        };
        let any_solo = seq.audio_tracks().iter().any(|t| t.solo);
        let frame = seq.frame_duration();
        for track in seq.audio_tracks().iter() {
            if !track.enabled || (any_solo && !track.solo) {
                continue;
            }
            // What this track's clips add up to, before the track's own level and pan.
            let mut bus = vec![0f32; count * 2];
            // Clips can be heard a little before and after their own span while a transition
            // is under way, and clip edges are rounded to samples, so look wider.
            let pad = Time::from_secs_f64(MAX_TRANSITION_SECS) + Time::from_samples(1, rate);
            let probe = TimeRange {
                start: window.start - pad,
                end: window.end + pad,
            };
            for clip in track.clips_in(probe) {
                if !clip.enabled {
                    continue;
                }
                let cs = clip.start.to_samples_round(rate);
                let ce = clip.end().to_samples_round(rate);
                // The spans of the transition into this clip and the one out of it.
                let tin = clip.transition_in.map(|t| {
                    let (a, b) = t.window(clip.start, frame);
                    (a.to_samples_round(rate), b.to_samples_round(rate))
                });
                let tout = track
                    .clip_from(clip.end())
                    .filter(|n| n.start == clip.end())
                    .and_then(|n| {
                        n.transition_in.map(|t| {
                            let (a, b) = t.window(n.start, frame);
                            (a.to_samples_round(rate), b.to_samples_round(rate))
                        })
                    });
                let heard_from = tin.map_or(cs, |(a, _)| a);
                let heard_to = tout.map_or(ce, |(_, b)| b);
                let from = first.max(heard_from);
                let to = last.min(heard_to);
                if from >= to {
                    continue;
                }
                let asset = self
                    .project
                    .asset(clip.asset)
                    .ok_or(timeline_engine::Error::NoSuchAsset(clip.asset))?
                    .clone();
                let mut part = match self.clip_audio(clip, &asset, (cs, ce - cs), from, to, rate) {
                    Ok(p) => p,
                    Err(Error::MissingMedia { .. })
                        if self.settings.missing_media == MissingMedia::Offline =>
                    {
                        continue
                    }
                    Err(e) => return Err(e),
                };
                let fade_in = clip
                    .fade_in
                    .map(|f| (f.curve, f.duration.to_samples_round(rate)));
                let fade_out = clip
                    .fade_out
                    .map(|f| (f.curve, f.duration.to_samples_round(rate)));
                if tin.is_some() || tout.is_some() || fade_in.is_some() || fade_out.is_some() {
                    for (j, pair) in part.chunks_exact_mut(2).enumerate() {
                        let s = from + j as i64;
                        let mut g = 1.0f32;
                        if let Some((curve, n)) = fade_in {
                            if s >= cs && s < cs + n {
                                g *= audio_engine::fade_in_gain(curve, progress(s, cs, cs + n));
                            }
                        }
                        if let Some((curve, n)) = fade_out {
                            if s >= ce - n && s < ce {
                                g *= audio_engine::fade_out_gain(curve, progress(s, ce - n, ce));
                            }
                        }
                        if let Some((a, b)) = tin {
                            if s < b {
                                g *= fade_in_gain(progress(s, a, b));
                            }
                        }
                        if let Some((a, b)) = tout {
                            if s >= a {
                                g *= fade_out_gain(progress(s, a, b));
                            }
                        }
                        pair[0] *= g;
                        pair[1] *= g;
                    }
                }
                if clip.anim.is_animated(Param::Gain) || clip.anim.is_animated(Param::Pan) {
                    audio_engine::gain::apply_gain_pan_with(&mut part, |j| {
                        let t = Time::from_samples(from + j as i64, rate);
                        (
                            if clip.anim.is_animated(Param::Gain) {
                                audio_engine::db_to_gain(clip.gain_db_at(t) as f32)
                            } else {
                                clip.gain.linear()
                            },
                            if clip.anim.is_animated(Param::Pan) {
                                (clip.pan_at(t) / 100.0) as f32
                            } else {
                                clip.pan.value()
                            },
                        )
                    });
                } else {
                    audio_engine::gain::apply_gain_pan(
                        &mut part,
                        clip.gain.linear(),
                        clip.pan.value(),
                    );
                }
                let at = ((from - first) * 2) as usize;
                for (m, s) in bus[at..at + part.len()].iter_mut().zip(&part) {
                    *m += *s;
                }
            }
            if track.anim.is_empty() {
                audio_engine::gain::apply_gain_pan(
                    &mut bus,
                    track.gain.linear(),
                    track.pan.value(),
                );
            } else {
                audio_engine::gain::apply_gain_pan_with(&mut bus, |j| {
                    let t = Time::from_samples(first + j as i64, rate);
                    (
                        if track.anim.is_animated(Param::Gain) {
                            audio_engine::db_to_gain(track.gain_db_at(t) as f32)
                        } else {
                            track.gain.linear()
                        },
                        if track.anim.is_animated(Param::Pan) {
                            (track.pan_at(t) / 100.0) as f32
                        } else {
                            track.pan.value()
                        },
                    )
                });
            }
            for (m, s) in mix.iter_mut().zip(&bus) {
                *m += *s;
            }
        }
        if seq.master_anim.is_animated(Param::Gain) {
            audio_engine::gain::apply_gain_pan_with(&mut mix, |j| {
                let t = Time::from_samples(first + j as i64, rate);
                (
                    audio_engine::db_to_gain(seq.master_gain_db_at(t) as f32),
                    0.0,
                )
            });
        } else {
            audio_engine::gain::apply_gain_pan(&mut mix, seq.master_gain.linear(), 0.0);
        }
        audio_engine::apply_limiter(&mut mix, self.settings.limiter);
        Ok(mix)
    }

    /// The clip's sound for sequence samples `from..to`, following its speed and direction.
    /// `(cs, n)` is the clip's first sample number on the timeline and its length in samples.
    /// Samples outside the clip (during a transition) continue the clip's media.
    fn clip_audio(
        &mut self,
        clip: &Clip,
        asset: &Asset,
        (cs, n): (i64, i64),
        from: i64,
        to: i64,
        rate: u32,
    ) -> Result<Vec<f32>> {
        let count = (to - from) as usize;
        let sr = f64::from(rate);
        if clip.speed.is_normal() && !clip.reverse && !clip.is_ramped() {
            let secs = clip.source_in.as_secs_f64() + (from - cs) as f64 / sr;
            return self.source.audio(asset, clip.stream, secs, count, rate);
        }
        // Where each output sample reads from, in media samples (fractional).
        let speed = clip.speed.as_f64();
        let origin = clip.source_in.as_secs_f64() * sr;
        let ramp_origin = clip.source_in.as_secs_f64();
        let ramp_len = clip.source_len().as_secs_f64();
        let position = |k: i64| -> f64 {
            if clip.is_ramped() {
                // A speed ramp: how much media has played is the area under the speed curve.
                if clip.reverse {
                    let played = clip
                        .media_len(Time::from_samples(k + 1, rate))
                        .as_secs_f64();
                    (ramp_origin + ramp_len - played) * sr
                } else {
                    let played = clip.media_len(Time::from_samples(k, rate)).as_secs_f64();
                    (ramp_origin + played) * sr
                }
            } else if clip.reverse {
                origin + (n - 1 - k) as f64 * speed
            } else {
                origin + k as f64 * speed
            }
        };
        let (k0, k1) = (from - cs, to - 1 - cs);
        let (p0, p1) = (position(k0), position(k1));
        let (lo, hi) = (p0.min(p1), p0.max(p1));
        let base = lo.floor() as i64 - 1;
        let len = (hi.floor() as i64 + 2 - base + 1) as usize;
        let block = self
            .source
            .audio(asset, clip.stream, base as f64 / sr, len, rate)?;
        let at = |i: i64, c: usize| -> f32 {
            let idx = (i - base).clamp(0, len as i64 - 1) as usize;
            block[idx * 2 + c]
        };
        let mut out = vec![0f32; count * 2];
        for (j, pair) in out.chunks_exact_mut(2).enumerate() {
            let m = position(k0 + j as i64);
            let i = m.floor() as i64;
            let f = (m - i as f64) as f32;
            for (c, o) in pair.iter_mut().enumerate() {
                *o = if f == 0.0 {
                    at(i, c)
                } else {
                    catmull_rom(at(i - 1, c), at(i, c), at(i + 1, c), at(i + 2, c), f)
                };
            }
        }
        Ok(out)
    }
}

/// How far `s` is through the span `a..b`, measured to the middle of the sample.
fn progress(s: i64, a: i64, b: i64) -> f64 {
    if b <= a {
        return 1.0;
    }
    ((s - a) as f64 + 0.5) / (b - a) as f64
}

/// Smooth interpolation between `p1` and `p2` at fraction `t`, using the points either side.
fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t)
}
