// SPDX-License-Identifier: GPL-3.0-or-later
//! Bridge to the Media Engine (cargo feature `media`): turns a probed file into an asset.

use std::path::Path;

use media_engine as media;

use crate::error::{Error, Result};
use crate::model::{AudioProps, NewAsset, VideoProps};
use crate::time::{FrameRate, Time};

/// Describe an already probed file as an asset.
///
/// A file with one video frame and no audio, or with no length at all, is treated as a
/// still image: it gets no duration and can be held for as long as the edit needs.
pub fn asset_from_info(info: &media::MediaInfo) -> NewAsset {
    let video: Vec<VideoProps> = info
        .video
        .iter()
        .map(|v| VideoProps {
            width: v.width,
            height: v.height,
            frame_rate: v
                .frame_rate
                .and_then(|r| {
                    (r.num > 0 && r.den > 0).then(|| FrameRate::new(r.num as u32, r.den as u32))
                })
                .flatten(),
        })
        .collect();
    let single_frame = info.video.len() == 1 && info.video[0].frames == Some(1);
    let still = info.audio.is_empty() && (single_frame || info.duration_secs.is_none());
    let name = info
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "media".to_string());
    NewAsset {
        name,
        path: info.path.clone(),
        duration: if still {
            None
        } else {
            info.duration_secs.map(Time::from_secs_f64)
        },
        video,
        audio: info
            .audio
            .iter()
            .map(|a| AudioProps {
                sample_rate: a.sample_rate,
                channels: a.channels,
            })
            .collect(),
    }
}

/// Probe a file and describe it as an asset.
///
/// The stored path is absolute, so the project still finds the file when it is opened from
/// another folder.
pub fn probe_asset(path: impl AsRef<Path>) -> Result<NewAsset> {
    let path = path.as_ref();
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let info = media::probe(&absolute).map_err(|e| Error::Media(e.to_string()))?;
    Ok(asset_from_info(&info))
}
