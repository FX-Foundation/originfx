// SPDX-License-Identifier: GPL-3.0-or-later
//! Questions a player or exporter asks of a timeline: what is on screen and what is heard
//! at a moment, and which media is missing.

use crate::ids::{AssetId, TrackId};
use crate::model::{Clip, Project, Sequence};
use crate::time::Time;

/// One clip contributing to the picture at a moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoLayer {
    pub track: TrackId,
    pub clip: Clip,
    /// Where in the asset to read the frame from.
    pub source_time: Time,
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
    pub fn video_layers_at(&self, t: Time) -> Vec<VideoLayer> {
        self.video_tracks()
            .iter()
            .filter(|tr| tr.enabled)
            .filter_map(|tr| {
                tr.clip_at(t).filter(|c| c.enabled).map(|c| VideoLayer {
                    track: tr.id,
                    clip: c.clone(),
                    source_time: c.source_time_at(t),
                })
            })
            .collect()
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
            .filter(|a| !a.path.exists())
            .map(|a| a.id)
            .collect()
    }
}
