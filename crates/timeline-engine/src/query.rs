// SPDX-License-Identifier: GPL-3.0-or-later
//! Questions a player or exporter asks of a timeline: what is on screen and what is heard
//! at a moment, and which media is missing.

use crate::ids::{AssetId, TrackId};
use crate::model::{Clip, Project, Sequence};
use crate::retime::TransitionKind;
use crate::time::Time;

/// One clip contributing to the picture at a moment.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoLayer {
    pub track: TrackId,
    pub clip: Clip,
    /// Where in the asset to read the frame from.
    pub source_time: Time,
    /// Set while a transition is under way on this track. `clip` is then the clip the
    /// moment belongs to; the picture to draw is the blend described here.
    pub transition: Option<ActiveTransition>,
}

/// One side of a transition: a clip and where to read its picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side {
    pub clip: Clip,
    pub source_time: Time,
}

/// A transition that is under way at the moment asked about.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveTransition {
    pub kind: TransitionKind,
    /// How far along it is, strictly between 0 and 1.
    pub progress: f64,
    /// The clip that is leaving. `None` if it is disabled.
    pub outgoing: Option<Side>,
    /// The clip that is arriving. `None` if it is disabled.
    pub incoming: Option<Side>,
}

/// One clip contributing to the sound at a moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSource {
    pub track: TrackId,
    pub clip: Clip,
    pub source_time: Time,
}

impl Sequence {
    /// The picture layers at time `t`, bottom first (draw them in this order, each over the last).
    /// Disabled tracks and clips are skipped.
    ///
    /// Picture times follow each clip's speed and direction. `t` should be the start of a frame
    /// of this sequence; the source time of a reversed clip is the mirror of the forward frame.
    pub fn video_layers_at(&self, t: Time) -> Vec<VideoLayer> {
        let frame = self.frame_duration();
        let mut layers = Vec::new();
        for tr in self.video_tracks().iter().filter(|tr| tr.enabled) {
            if let Some((a, b, kind, w0, w1)) = tr
                .transition_at(t, frame)
                .map(|(a, b, tr, w0, w1)| (a, b, tr.kind, w0, w1))
            {
                if a.enabled || b.enabled {
                    // The middle of the frame, so the first frame is never a bare outgoing
                    // picture and the last never a bare incoming one.
                    let progress = ((t - w0).ticks() as f64 + frame.ticks() as f64 / 2.0)
                        / (w1 - w0).ticks() as f64;
                    let side = |c: &Clip| {
                        c.enabled.then(|| Side {
                            clip: c.clone(),
                            source_time: c.frame_source_time(t, frame),
                        })
                    };
                    let owner = if t >= b.start { b } else { a };
                    layers.push(VideoLayer {
                        track: tr.id,
                        clip: owner.clone(),
                        source_time: owner.frame_source_time(t, frame),
                        transition: Some(ActiveTransition {
                            kind,
                            progress: progress.clamp(0.0, 1.0),
                            outgoing: side(a),
                            incoming: side(b),
                        }),
                    });
                }
                continue;
            }
            if let Some(c) = tr.clip_at(t).filter(|c| c.enabled) {
                layers.push(VideoLayer {
                    track: tr.id,
                    clip: c.clone(),
                    source_time: c.frame_source_time(t, frame),
                    transition: None,
                });
            }
        }
        layers
    }

    /// The sound sources at time `t`. Muted tracks and disabled clips are skipped, and if any
    /// audio track is soloed only soloed tracks are heard.
    pub fn audio_at(&self, t: Time) -> Vec<AudioSource> {
        let any_solo = self.audio_tracks().iter().any(|tr| tr.solo);
        self.audio_tracks()
            .iter()
            .filter(|tr| tr.enabled && (!any_solo || tr.solo))
            .filter_map(|tr| {
                tr.clip_at(t).filter(|c| c.enabled).map(|c| AudioSource {
                    track: tr.id,
                    clip: c.clone(),
                    source_time: c.source_time_at(t),
                })
            })
            .collect()
    }
}

impl Project {
    /// Assets that at least one clip uses.
    pub fn used_assets(&self) -> Vec<AssetId> {
        let mut v: Vec<AssetId> = self
            .sequences()
            .values()
            .flat_map(|s| s.clips().map(|(_, c)| c.asset).collect::<Vec<_>>())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Assets whose file is not on disk.
    pub fn missing_assets(&self) -> Vec<AssetId> {
        self.assets()
            .values()
            .filter(|a| a.generator.is_none() && !a.path.exists())
            .map(|a| a.id)
            .collect()
    }
}
