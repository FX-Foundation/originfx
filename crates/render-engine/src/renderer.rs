// SPDX-License-Identifier: GPL-3.0-or-later
//! The [`Renderer`]: composites the picture and mixes the sound of one sequence.

use std::collections::HashMap;
use std::sync::Arc;

use media_engine::{AudioBuffer, VideoFrame};
use timeline_engine::{AssetId, Project, Sequence, SequenceId, Time, TimeRange};

use crate::composite::{blend_over, placement, solid, Fit};
use crate::error::{Error, Result};
use crate::scale::resize;
use crate::source::MediaSource;

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
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings {
            fit: Fit::Contain,
            background: [0, 0, 0, 255],
            missing_media: MissingMedia::Fail,
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
            let asset = self
                .project
                .asset(layer.clip.asset)
                .ok_or(timeline_engine::Error::NoSuchAsset(layer.clip.asset))?
                .clone();
            let secs = layer.source_time.as_secs_f64();
            let frame = match self.source.video_frame(&asset, layer.clip.stream, secs) {
                Ok(f) => f,
                Err(Error::MissingMedia { .. })
                    if self.settings.missing_media == MissingMedia::Offline =>
                {
                    blend_over(
                        &mut canvas,
                        (cw, ch),
                        &solid(cw, ch, OFFLINE_COLOUR),
                        (cw, ch),
                        (0, 0),
                    );
                    continue;
                }
                Err(e) => return Err(e),
            };
            let (sw, sh) = (frame.width as usize, frame.height as usize);
            let place = placement(sw, sh, cw, ch, self.settings.fit);
            if asset.is_still() {
                let key = (asset.id, layer.clip.stream, place.width, place.height);
                let pixels = match self.stills.get(&key) {
                    Some(p) => Arc::clone(p),
                    None => {
                        let p = Arc::new(resize(&frame.data, sw, sh, place.width, place.height));
                        self.stills.insert(key, Arc::clone(&p));
                        p
                    }
                };
                blend_over(
                    &mut canvas,
                    (cw, ch),
                    &pixels,
                    (place.width, place.height),
                    (place.x, place.y),
                );
            } else if (place.width, place.height) == (sw, sh) {
                blend_over(
                    &mut canvas,
                    (cw, ch),
                    &frame.data,
                    (sw, sh),
                    (place.x, place.y),
                );
            } else {
                let scaled = resize(&frame.data, sw, sh, place.width, place.height);
                blend_over(
                    &mut canvas,
                    (cw, ch),
                    &scaled,
                    (place.width, place.height),
                    (place.x, place.y),
                );
            }
        }
        Ok(VideoFrame {
            width: seq.width,
            height: seq.height,
            pts_secs: t.as_secs_f64(),
            data: canvas,
        })
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
        for track in seq.audio_tracks().iter() {
            if !track.enabled || (any_solo && !track.solo) {
                continue;
            }
            // One sample of slack each side, since clip edges are rounded to samples.
            let probe = TimeRange {
                start: window.start - Time::from_samples(1, rate),
                end: window.end + Time::from_samples(1, rate),
            };
            for clip in track.clips_in(probe) {
                if !clip.enabled {
                    continue;
                }
                let cs = clip.start.to_samples_round(rate);
                let ce = clip.end().to_samples_round(rate);
                let from = first.max(cs);
                let to = last.min(ce);
                if from >= to {
                    continue;
                }
                let asset = self
                    .project
                    .asset(clip.asset)
                    .ok_or(timeline_engine::Error::NoSuchAsset(clip.asset))?
                    .clone();
                let secs = clip.source_in.as_secs_f64() + (from - cs) as f64 / f64::from(rate);
                let n = (to - from) as usize;
                let part = match self.source.audio(&asset, clip.stream, secs, n, rate) {
                    Ok(p) => p,
                    Err(Error::MissingMedia { .. })
                        if self.settings.missing_media == MissingMedia::Offline =>
                    {
                        continue
                    }
                    Err(e) => return Err(e),
                };
                let at = ((from - first) * 2) as usize;
                for (m, s) in mix[at..at + part.len()].iter_mut().zip(&part) {
                    *m += *s;
                }
            }
        }
        for s in &mut mix {
            *s = s.clamp(-1.0, 1.0);
        }
        Ok(mix)
    }
}
