// SPDX-License-Identifier: GPL-3.0-or-later
//! Saving and loading projects as `.originfx` files (plain JSON, versioned).
//!
//! Saving writes to a temporary file next to the target and renames it into place, so a
//! crash never leaves half a project. Loading checks every rule of [`Project::validate`],
//! so a hand-edited or damaged file is refused with a clear error rather than loaded broken.

use std::path::{Path, PathBuf};

use im::{OrdMap, Vector};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ids::*;
use crate::model::*;
use crate::retime::{Alignment, Speed, Transition, TransitionKind};
use crate::time::{FrameRate, Time};

/// The file format version this build writes and reads.
pub const FORMAT_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct FileDto {
    format: String,
    version: u32,
    name: String,
    next_id: u64,
    assets: Vec<AssetDto>,
    sequences: Vec<SequenceDto>,
}

#[derive(Serialize, Deserialize)]
struct RateDto(u32, u32);

#[derive(Serialize, Deserialize)]
struct VideoDto {
    width: u32,
    height: u32,
    frame_rate: Option<RateDto>,
}

#[derive(Serialize, Deserialize)]
struct AudioDto {
    sample_rate: u32,
    channels: u16,
}

#[derive(Serialize, Deserialize)]
struct AssetDto {
    id: u64,
    name: String,
    path: PathBuf,
    duration: Option<i64>,
    video: Vec<VideoDto>,
    audio: Vec<AudioDto>,
}

#[derive(Serialize, Deserialize)]
struct ClipDto {
    id: u64,
    asset: u64,
    stream: u32,
    name: String,
    start: i64,
    source_in: i64,
    duration: i64,
    enabled: bool,
    link: Option<u64>,
    /// Added in format version 2; files from version 1 mean normal speed.
    #[serde(default = "one")]
    speed_num: u32,
    #[serde(default = "one")]
    speed_den: u32,
    #[serde(default)]
    reverse: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transition_in: Option<TransitionDto>,
}

fn one() -> u32 {
    1
}

#[derive(Serialize, Deserialize)]
struct TransitionDto {
    kind: String,
    duration: i64,
    alignment: String,
}

#[derive(Serialize, Deserialize)]
struct TrackDto {
    id: u64,
    name: String,
    enabled: bool,
    locked: bool,
    solo: bool,
    clips: Vec<ClipDto>,
}

#[derive(Serialize, Deserialize)]
struct MarkerDto {
    id: u64,
    time: i64,
    duration: i64,
    name: String,
    note: String,
    colour: String,
}

#[derive(Serialize, Deserialize)]
struct SequenceDto {
    id: u64,
    name: String,
    frame_rate: RateDto,
    width: u32,
    height: u32,
    sample_rate: u32,
    drop_frame: bool,
    timecode_start: i64,
    video: Vec<TrackDto>,
    audio: Vec<TrackDto>,
    markers: Vec<MarkerDto>,
}

fn rate_out(r: FrameRate) -> RateDto {
    RateDto(r.num(), r.den())
}

fn rate_in(r: &RateDto) -> Result<FrameRate> {
    FrameRate::new(r.0, r.1).ok_or_else(|| Error::File(format!("bad frame rate {}/{}", r.0, r.1)))
}

fn track_out(t: &Track) -> TrackDto {
    TrackDto {
        id: t.id.0,
        name: t.name.clone(),
        enabled: t.enabled,
        locked: t.locked,
        solo: t.solo,
        clips: t
            .clips()
            .map(|c| ClipDto {
                id: c.id.0,
                asset: c.asset.0,
                stream: c.stream,
                name: c.name.clone(),
                start: c.start.ticks(),
                source_in: c.source_in.ticks(),
                duration: c.duration.ticks(),
                enabled: c.enabled,
                link: c.link.map(|l| l.0),
                speed_num: c.speed.num(),
                speed_den: c.speed.den(),
                reverse: c.reverse,
                transition_in: c.transition_in.map(|t| TransitionDto {
                    kind: t.kind.name(),
                    duration: t.duration.ticks(),
                    alignment: t.alignment.as_str().to_string(),
                }),
            })
            .collect(),
    }
}

fn track_in(kind: TrackKind, d: TrackDto) -> Result<Track> {
    let mut t = Track::new(TrackId(d.id), kind, d.name);
    t.enabled = d.enabled;
    t.locked = d.locked;
    t.solo = d.solo;
    for c in d.clips {
        let clip = Clip {
            id: ClipId(c.id),
            asset: AssetId(c.asset),
            stream: c.stream,
            name: c.name,
            start: Time::from_ticks(c.start),
            source_in: Time::from_ticks(c.source_in),
            duration: Time::from_ticks(c.duration),
            enabled: c.enabled,
            link: c.link.map(LinkId),
            speed: Speed::new(c.speed_num, c.speed_den)
                .ok_or_else(|| Error::File(format!("bad speed {}/{}", c.speed_num, c.speed_den)))?,
            reverse: c.reverse,
            transition_in: match c.transition_in {
                None => None,
                Some(t) => Some(Transition {
                    kind: TransitionKind::parse(&t.kind)
                        .ok_or_else(|| Error::File(format!("unknown transition {:?}", t.kind)))?,
                    duration: Time::from_ticks(t.duration),
                    alignment: Alignment::parse(&t.alignment).ok_or_else(|| {
                        Error::File(format!("unknown transition alignment {:?}", t.alignment))
                    })?,
                }),
            },
        };
        t.items.insert(clip.start, clip);
    }
    Ok(t)
}

fn to_dto(p: &Project) -> FileDto {
    FileDto {
        format: "originfx-project".into(),
        version: FORMAT_VERSION,
        name: p.name.clone(),
        next_id: p.next_id(),
        assets: p
            .assets()
            .values()
            .map(|a| AssetDto {
                id: a.id.0,
                name: a.name.clone(),
                path: a.path.clone(),
                duration: a.duration.map(Time::ticks),
                video: a
                    .video
                    .iter()
                    .map(|v| VideoDto {
                        width: v.width,
                        height: v.height,
                        frame_rate: v.frame_rate.map(rate_out),
                    })
                    .collect(),
                audio: a
                    .audio
                    .iter()
                    .map(|s| AudioDto {
                        sample_rate: s.sample_rate,
                        channels: s.channels,
                    })
                    .collect(),
            })
            .collect(),
        sequences: p
            .sequences()
            .values()
            .map(|s| SequenceDto {
                id: s.id.0,
                name: s.name.clone(),
                frame_rate: rate_out(s.frame_rate),
                width: s.width,
                height: s.height,
                sample_rate: s.sample_rate,
                drop_frame: s.drop_frame,
                timecode_start: s.timecode_start.ticks(),
                video: s.video_tracks().iter().map(track_out).collect(),
                audio: s.audio_tracks().iter().map(track_out).collect(),
                markers: s
                    .markers()
                    .iter()
                    .map(|m| MarkerDto {
                        id: m.id.0,
                        time: m.time.ticks(),
                        duration: m.duration.ticks(),
                        name: m.name.clone(),
                        note: m.note.clone(),
                        colour: m.colour.as_str().into(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn from_dto(d: FileDto) -> Result<Project> {
    if d.format != "originfx-project" {
        return Err(Error::File("not an OriginFX project file".into()));
    }
    if d.version > FORMAT_VERSION {
        return Err(Error::UnsupportedVersion {
            found: d.version,
            supported: FORMAT_VERSION,
        });
    }
    let mut assets = OrdMap::new();
    for a in d.assets {
        let mut video = Vec::new();
        for v in &a.video {
            video.push(VideoProps {
                width: v.width,
                height: v.height,
                frame_rate: v.frame_rate.as_ref().map(rate_in).transpose()?,
            });
        }
        let id = AssetId(a.id);
        assets.insert(
            id,
            Asset {
                id,
                name: a.name,
                path: a.path,
                duration: a.duration.map(Time::from_ticks),
                video,
                audio: a
                    .audio
                    .into_iter()
                    .map(|s| AudioProps {
                        sample_rate: s.sample_rate,
                        channels: s.channels,
                    })
                    .collect(),
            },
        );
    }
    let mut sequences = OrdMap::new();
    for s in d.sequences {
        let id = SequenceId(s.id);
        let settings = SequenceSettings {
            name: s.name,
            frame_rate: rate_in(&s.frame_rate)?,
            width: s.width,
            height: s.height,
            sample_rate: s.sample_rate,
            drop_frame: s.drop_frame,
            timecode_start: Time::from_ticks(s.timecode_start),
            video_tracks: 0,
            audio_tracks: 0,
        };
        let mut seq = Sequence::new(id, settings);
        seq.video = s
            .video
            .into_iter()
            .map(|t| track_in(TrackKind::Video, t))
            .collect::<Result<Vector<_>>>()?;
        seq.audio = s
            .audio
            .into_iter()
            .map(|t| track_in(TrackKind::Audio, t))
            .collect::<Result<Vector<_>>>()?;
        for m in s.markers {
            let colour = MarkerColour::parse(&m.colour)
                .ok_or_else(|| Error::File(format!("unknown marker colour {:?}", m.colour)))?;
            seq.markers.push_back(Marker {
                id: MarkerId(m.id),
                time: Time::from_ticks(m.time),
                duration: Time::from_ticks(m.duration),
                name: m.name,
                note: m.note,
                colour,
            });
        }
        sequences.insert(id, seq);
    }
    let project = Project::from_parts(d.name, assets, sequences, d.next_id);
    project.validate()?;
    Ok(project)
}

impl Project {
    /// The project as file text.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(&to_dto(self)).map_err(|e| Error::File(e.to_string()))
    }

    /// Read a project from file text. Fails if it is damaged or breaks a rule.
    pub fn from_json(text: &str) -> Result<Project> {
        let dto: FileDto = serde_json::from_str(text).map_err(|e| Error::File(e.to_string()))?;
        from_dto(dto)
    }

    /// Save to `path`, replacing it atomically.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let text = self.to_json()?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, text)?;
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    /// Load from `path`.
    pub fn load(path: impl AsRef<Path>) -> Result<Project> {
        Project::from_json(&std::fs::read_to_string(path)?)
    }
}
