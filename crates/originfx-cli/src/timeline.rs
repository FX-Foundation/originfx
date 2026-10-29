// SPDX-License-Identifier: GPL-3.0-or-later
//! `originfx timeline`: create and edit project files from the command line.
//!
//! Every command loads the project file, makes one edit through the Timeline Engine and
//! saves it again, so a failed edit leaves the file untouched. Times are in seconds.
//! Track numbers start at 1 (`--video-track 1` is V1).

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use timeline_engine::media::probe_asset;
use timeline_engine::{
    Alignment, AssetId, ClipId, Edge, Editor, FrameRate, LinkMode, MarkerColour, PlaceMode,
    Project, Ripple, SequenceId, SequenceSettings, Speed, SpeedMode, Time, TrackKind, Transition,
    TransitionKind,
};

use crate::CliResult;

#[derive(Args)]
pub struct TimelineArgs {
    /// The project file (`.originfx`).
    project: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new project with one sequence.
    New {
        #[arg(long, default_value = "Untitled")]
        name: String,
        #[arg(long, default_value_t = 24.0)]
        fps: f64,
        /// Picture size, like 1920x1080.
        #[arg(long, default_value = "1920x1080")]
        size: String,
        /// Use drop-frame timecode (29.97 and 59.94 only).
        #[arg(long)]
        drop_frame: bool,
    },
    /// Add a media file to the project and print its id.
    Import { file: PathBuf },
    /// Place an asset on the timeline.
    Add {
        asset: String,
        /// Where on the timeline (seconds).
        #[arg(long, default_value_t = 0.0)]
        at: f64,
        /// Where in the media to start (seconds).
        #[arg(long, default_value_t = 0.0)]
        from: f64,
        /// How much to use (seconds). Default: the rest of the media.
        #[arg(long)]
        length: Option<f64>,
        /// Make room instead of replacing what is there.
        #[arg(long)]
        insert: bool,
        #[arg(long, default_value_t = 1)]
        video_track: usize,
        #[arg(long, default_value_t = 1)]
        audio_track: usize,
        /// Leave out the picture.
        #[arg(long)]
        no_video: bool,
        /// Leave out the sound.
        #[arg(long)]
        no_audio: bool,
    },
    /// Cut a clip in two (and the clips linked to it).
    Split { clip: String, at: f64 },
    /// Delete clips.
    Delete {
        clips: Vec<String>,
        /// Close the gap on every track.
        #[arg(long)]
        ripple: bool,
    },
    /// Move the start or end of a clip.
    Trim {
        clip: String,
        /// Seconds; positive is later in time.
        #[arg(allow_hyphen_values = true)]
        delta: f64,
        #[arg(long, conflicts_with = "tail")]
        head: bool,
        #[arg(long)]
        tail: bool,
        #[arg(long)]
        ripple: bool,
    },
    /// Move a clip in time and/or to another track.
    Move {
        clip: String,
        #[arg(allow_hyphen_values = true)]
        delta: f64,
        /// Tracks to move up (positive) or down.
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        tracks: i32,
    },
    /// Change how fast a clip plays (and the clips linked to it): `2`, `0.5`, `250%`, `3/2`.
    Speed {
        clip: String,
        speed: String,
        /// Keep the clip as long as it is and show more or less of the media, instead of
        /// keeping the same media and changing the length.
        #[arg(long)]
        keep_length: bool,
        /// Move the clips after it to follow, instead of refusing if it would grow into them.
        #[arg(long)]
        ripple: bool,
    },
    /// Play a clip backwards (and the clips linked to it); `--off` plays it forwards again.
    Reverse {
        clip: String,
        #[arg(long)]
        off: bool,
    },
    /// Put a transition into a clip from the clip before it, or `--remove` it.
    Transition {
        /// The clip the transition leads into; it must touch the clip before it.
        clip: String,
        /// dissolve, dip-black, dip-white, wipe-left/right/up/down or push-left/right/up/down.
        #[arg(default_value = "dissolve")]
        kind: String,
        /// Length in seconds.
        #[arg(long, default_value_t = 1.0)]
        length: f64,
        /// center, start (begins at the cut) or end (finishes at the cut).
        #[arg(long, default_value = "center")]
        align: String,
        #[arg(long)]
        remove: bool,
    },
    /// Add a marker.
    Marker {
        at: f64,
        name: String,
        #[arg(long, default_value = "blue")]
        colour: String,
    },
    /// List the sequence, track by track.
    Show,
    /// Check the project against every rule and list missing media.
    Check,
}

fn id_number(text: &str) -> Result<u64, String> {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    digits.parse().map_err(|_| format!("'{text}' is not an id"))
}

fn secs(s: f64) -> Time {
    Time::from_secs_f64(s)
}

fn first_sequence(p: &Project) -> Result<SequenceId, String> {
    p.sequences()
        .keys()
        .next()
        .copied()
        .ok_or_else(|| "the project has no sequence".to_string())
}

fn track_id(
    p: &Project,
    seq: SequenceId,
    kind: TrackKind,
    number: usize,
) -> Result<timeline_engine::TrackId, String> {
    let s = p.sequence(seq).ok_or("no sequence")?;
    number
        .checked_sub(1)
        .and_then(|i| s.tracks_of(kind).get(i))
        .map(|t| t.id)
        .ok_or_else(|| format!("there is no {} track {number}", kind_word(kind)))
}

fn kind_word(k: TrackKind) -> &'static str {
    match k {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
    }
}

/// How many transitions the project has. An edit can silently drop one (see the Timeline
/// Engine docs), and the command line says so.
fn transitions(p: &Project) -> usize {
    p.sequences()
        .values()
        .flat_map(|s| s.clips())
        .filter(|(_, c)| c.transition_in.is_some())
        .count()
}

fn save(ed: &Editor, path: &Path, before: usize) -> Result<(), Box<dyn std::error::Error>> {
    ed.project().save(path)?;
    let after = transitions(ed.project());
    if after < before {
        eprintln!(
            "note: {} transition(s) no longer fit after this edit and were removed",
            before - after
        );
    }
    Ok(())
}

fn load(path: &Path) -> Result<Editor, Box<dyn std::error::Error>> {
    Ok(Editor::from_project(Project::load(path)?))
}

pub fn run(args: TimelineArgs) -> CliResult {
    let path = args.project;
    match args.cmd {
        Cmd::New {
            name,
            fps,
            size,
            drop_frame,
        } => {
            if path.exists() {
                return Err(format!("{} already exists", path.display()).into());
            }
            let (w, h) = size
                .split_once('x')
                .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                .ok_or("size must look like 1920x1080")?;
            let rate = FrameRate::from_f64(fps).ok_or("unsupported frame rate")?;
            let mut ed = Editor::new(name.clone());
            ed.add_sequence(SequenceSettings {
                name: "Sequence 1".into(),
                frame_rate: rate,
                width: w,
                height: h,
                drop_frame: drop_frame && rate.supports_drop_frame(),
                ..Default::default()
            })?;
            ed.project().save(&path)?;
            println!("created {} ({name}, {rate} fps, {w}x{h})", path.display());
        }
        Cmd::Import { file } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let id = ed.add_asset(probe_asset(&file)?)?;
            save(&ed, &path, before)?;
            println!("{id}");
        }
        Cmd::Add {
            asset,
            at,
            from,
            length,
            insert,
            video_track,
            audio_track,
            no_video,
            no_audio,
        } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let asset = AssetId(id_number(&asset)?);
            let a = ed.project().asset(asset).ok_or("no such asset")?.clone();
            let v = (!no_video && a.has_video())
                .then(|| track_id(ed.project(), seq, TrackKind::Video, video_track))
                .transpose()?;
            let au = (!no_audio && a.has_audio())
                .then(|| track_id(ed.project(), seq, TrackKind::Audio, audio_track))
                .transpose()?;
            let mode = if insert {
                PlaceMode::Insert { sync: false }
            } else {
                PlaceMode::Overwrite
            };
            let placed = ed.place_asset(
                seq,
                asset,
                secs(at),
                secs(from),
                length.map(secs),
                v,
                au,
                mode,
            )?;
            save(&ed, &path, before)?;
            for c in [placed.video, placed.audio].into_iter().flatten() {
                println!("{c}");
            }
        }
        Cmd::Split { clip, at } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let (l, r) =
                ed.split_clip(seq, ClipId(id_number(&clip)?), secs(at), LinkMode::Follow)?;
            save(&ed, &path, before)?;
            println!("{l} {r}");
        }
        Cmd::Delete { clips, ripple } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let ids = clips
                .iter()
                .map(|c| id_number(c).map(ClipId))
                .collect::<Result<Vec<_>, _>>()?;
            let r = if ripple { Ripple::Sync } else { Ripple::Off };
            ed.delete_clips(seq, &ids, r, LinkMode::Follow)?;
            save(&ed, &path, before)?;
            println!("deleted");
        }
        Cmd::Trim {
            clip,
            delta,
            head,
            tail: _,
            ripple,
        } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let edge = if head { Edge::Head } else { Edge::Tail };
            let r = if ripple { Ripple::Sync } else { Ripple::Off };
            ed.trim(
                seq,
                ClipId(id_number(&clip)?),
                edge,
                secs(delta),
                r,
                LinkMode::Follow,
            )?;
            save(&ed, &path, before)?;
            println!("trimmed");
        }
        Cmd::Move {
            clip,
            delta,
            tracks,
        } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            ed.move_clips(
                seq,
                &[ClipId(id_number(&clip)?)],
                secs(delta),
                tracks,
                LinkMode::Follow,
            )?;
            save(&ed, &path, before)?;
            println!("moved");
        }
        Cmd::Speed {
            clip,
            speed,
            keep_length,
            ripple,
        } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let speed = Speed::parse(&speed)
                .ok_or("speed must look like 2, 0.5, 250% or 3/2, between 1% and 10000%")?;
            let mode = if keep_length {
                SpeedMode::KeepLength
            } else {
                SpeedMode::KeepMedia
            };
            let r = if ripple { Ripple::Sync } else { Ripple::Off };
            ed.set_speed(
                seq,
                ClipId(id_number(&clip)?),
                speed,
                mode,
                r,
                LinkMode::Follow,
            )?;
            save(&ed, &path, before)?;
            println!("speed {speed}");
        }
        Cmd::Reverse { clip, off } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            ed.set_reverse(seq, ClipId(id_number(&clip)?), !off, LinkMode::Follow)?;
            save(&ed, &path, before)?;
            println!("{}", if off { "forwards" } else { "reversed" });
        }
        Cmd::Transition {
            clip,
            kind,
            length,
            align,
            remove,
        } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let tr = if remove {
                None
            } else {
                Some(Transition {
                    kind: TransitionKind::parse(&kind).ok_or_else(|| {
                        format!(
                            "unknown transition; try one of: {}",
                            TransitionKind::all_names().join(", ")
                        )
                    })?,
                    duration: secs(length),
                    alignment: Alignment::parse(&align)
                        .ok_or("align must be center, start or end")?,
                })
            };
            ed.set_transition(seq, ClipId(id_number(&clip)?), tr, LinkMode::Follow)?;
            save(&ed, &path, before)?;
            println!("{}", if remove { "removed" } else { "added" });
        }
        Cmd::Marker { at, name, colour } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let colour = MarkerColour::parse(&colour).ok_or("unknown colour")?;
            let id = ed.add_marker(seq, secs(at), Time::ZERO, name, colour)?;
            save(&ed, &path, before)?;
            println!("{id}");
        }
        Cmd::Show => {
            let p = Project::load(&path)?;
            println!("{}", p.name);
            for a in p.assets().values() {
                let len = a.duration.map_or("still".to_string(), |d| d.to_string());
                println!("  {} {} ({len})", a.id, a.name);
            }
            for s in p.sequences().values() {
                println!(
                    "{} {} {}x{} @ {} fps, length {}",
                    s.id,
                    s.name,
                    s.width,
                    s.height,
                    s.frame_rate,
                    s.timecode_at(s.duration())
                        .map_or_else(|| s.duration().to_string(), |t| t.to_string())
                );
                for t in s.video_tracks().iter().rev().chain(s.audio_tracks().iter()) {
                    let flags = format!(
                        "{}{}",
                        if t.locked { " locked" } else { "" },
                        if t.enabled { "" } else { " off" }
                    );
                    println!("  {}{flags}", t.name);
                    for c in t.clips() {
                        let mut notes = String::new();
                        if !c.speed.is_normal() {
                            notes += &format!(" {}", c.speed);
                        }
                        if c.reverse {
                            notes += " reversed";
                        }
                        if let Some(t) = c.transition_in {
                            notes += &format!(
                                " [{} {} in, {}]",
                                t.kind.name(),
                                t.duration,
                                t.alignment.as_str()
                            );
                        }
                        if !c.enabled {
                            notes += " (disabled)";
                        }
                        println!(
                            "    {} {} {} -> {}{}",
                            c.id,
                            c.name,
                            c.start,
                            c.end(),
                            notes
                        );
                    }
                }
                for m in s.markers() {
                    println!(
                        "  marker {} at {}: {} ({})",
                        m.id,
                        m.time,
                        m.name,
                        m.colour.as_str()
                    );
                }
            }
        }
        Cmd::Check => {
            let p = Project::load(&path)?;
            p.validate()?;
            let missing = p.missing_assets();
            if missing.is_empty() {
                println!("ok: the project is valid and all media is present");
            } else {
                println!("valid, but {} media file(s) are missing:", missing.len());
                for id in missing {
                    if let Some(a) = p.asset(id) {
                        println!("  {id} {}", a.path.display());
                    }
                }
            }
        }
    }
    Ok(())
}
