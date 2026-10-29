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
    Align, Alignment, AssetId, ClipAudio, ClipId, Colour, Edge, Editor, EffectId, EffectKind, Fade,
    FadeCurve, Fill, FrameRate, Gain, Generator, Interp, LinkMode, MarkerColour, Pan, Param,
    PlaceMode, PluginSpec, Project, Ripple, SequenceId, SequenceSettings, Setting, Shadow, Shape,
    ShapeKind, Speed, SpeedMode, Stroke, Target, Text, Time, TrackKind, TrackUpdate, Transition,
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
        /// How many picture tracks the sequence starts with.
        #[arg(long, default_value_t = 2)]
        video_tracks: u32,
        /// How many sound tracks the sequence starts with.
        #[arg(long, default_value_t = 2)]
        audio_tracks: u32,
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
    /// Set a clip's volume in decibels (and its linked sound clips): `-6`, `+3`, `-inf` for silence.
    Gain {
        clip: String,
        #[arg(allow_hyphen_values = true)]
        db: String,
    },
    /// Pan a sound clip: -100 (hard left) to 100 (hard right), 0 for centre.
    Pan {
        clip: String,
        #[arg(allow_hyphen_values = true)]
        percent: i16,
    },
    /// Fade a sound clip in and/or out (seconds; 0 removes a fade).
    Fade {
        clip: String,
        #[arg(long = "in")]
        fade_in: Option<f64>,
        #[arg(long = "out")]
        fade_out: Option<f64>,
        /// linear, equal-power or smooth.
        #[arg(long, default_value = "smooth")]
        curve: String,
    },
    /// Set the level or pan of a whole audio track (track numbers start at 1).
    Track {
        number: usize,
        #[arg(long, allow_hyphen_values = true)]
        gain: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        pan: Option<i16>,
    },
    /// Set the level of the whole mix, or `--normalize` it to a loudness target (like -23 or -16 LUFS).
    Master {
        #[arg(allow_hyphen_values = true)]
        db: Option<String>,
        #[arg(long, allow_hyphen_values = true, conflicts_with = "db")]
        normalize: Option<f64>,
    },
    /// Put a keyframe on a value that changes over time: gain (dB), pan (%), speed (a ratio), or a
    /// picture value (opacity, x, y, scale-x, scale-y, rotation, crop-*, mask-*).
    /// The target is a clip (`clip7`), an audio track (`a1`) or `master`. `at` is in seconds on
    /// the timeline. A speed key on a clip makes a speed ramp.
    Key {
        target: String,
        /// gain, pan or speed.
        param: String,
        at: f64,
        #[arg(allow_hyphen_values = true)]
        value: String,
        /// hold, linear or smooth: how to travel from this key to the next.
        #[arg(long, default_value = "linear")]
        interp: String,
    },
    /// Remove the keyframe at a time (it must be exactly where the key is).
    Unkey {
        target: String,
        param: String,
        at: f64,
    },
    /// Remove all keyframes of a value; its fixed value applies again.
    ClearKeys { target: String, param: String },
    /// Set a picture clip's opacity, position, scale, rotation, crop or mask value, for example
    /// `look clip3 opacity 50` or `look clip3 scale-x 120`. (Use `key` to animate them.)
    Look {
        clip: String,
        /// opacity, x, y, scale-x, scale-y, rotation, crop-left/right/top/bottom,
        /// mask-x, mask-y, mask-width, mask-height, mask-feather.
        param: String,
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Choose how a picture clip's colour combines with the layers beneath, or `list`.
    Blend { clip: String, mode: String },
    /// Mask a picture clip: `rect` or `ellipse` (add `--invert` to show the outside), or `off`.
    Mask {
        clip: String,
        shape: String,
        #[arg(long)]
        invert: bool,
    },
    /// Drawn graphics: solid colours, gradients, shapes and text. They are media like a file is:
    /// add one, then place it with `add` (give a `--length`).
    Graphic {
        #[command(subcommand)]
        cmd: GfxCmd,
    },
    /// Picture effects on a clip: colour, blur, sharpen, vignette and more.
    Effect {
        #[command(subcommand)]
        cmd: FxCmd,
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

#[derive(Subcommand)]
enum GfxCmd {
    /// Add a graphic to the project's media; prints its asset id.
    Add {
        name: String,
        #[command(subcommand)]
        kind: GfxKind,
    },
    /// Replace what a graphic looks like (every clip using it changes).
    Set {
        asset: String,
        #[command(subcommand)]
        kind: GfxKind,
    },
}

#[derive(Subcommand, Clone)]
enum GfxKind {
    /// The whole frame in one colour.
    Solid { colour: String },
    /// The whole frame, blending from one colour to another.
    Gradient {
        from: String,
        to: String,
        /// 0 runs left to right, 90 top to bottom.
        #[arg(long, default_value_t = 90.0, allow_hyphen_values = true)]
        angle: f64,
    },
    /// A shape in the middle of the frame: rect, ellipse, polygon or star. Move, size and turn it
    /// with `look` on the clip.
    Shape {
        shape: String,
        /// Percent of the frame width.
        #[arg(long, default_value_t = 30.0)]
        width: f64,
        /// Percent of the frame height.
        #[arg(long, default_value_t = 30.0)]
        height: f64,
        #[arg(long, default_value = "white")]
        fill: String,
        /// A second colour makes the fill a gradient from `--fill` to this.
        #[arg(long)]
        fill_to: Option<String>,
        #[arg(long, default_value_t = 90.0, allow_hyphen_values = true)]
        angle: f64,
        /// Outline colour.
        #[arg(long)]
        stroke: Option<String>,
        /// Outline width, percent of the frame height.
        #[arg(long, default_value_t = 1.0)]
        stroke_width: f64,
        /// Rect only: corner rounding, percent of the shorter side (100 is a pill).
        #[arg(long, default_value_t = 0.0)]
        corner: f64,
        /// Polygon sides or star points.
        #[arg(long, default_value_t = 5)]
        sides: u32,
        /// Star only: inner radius, percent of the outer.
        #[arg(long, default_value_t = 40.0)]
        inner: f64,
    },
    /// Text in the middle of the frame; `\n` or a real newline starts a new line.
    Text {
        text: String,
        /// A font file (.ttf or .otf). Without one the first usual system font is used.
        #[arg(long)]
        font: Option<String>,
        /// Percent of the frame height.
        #[arg(long, default_value_t = 10.0)]
        size: f64,
        #[arg(long, default_value = "white")]
        colour: String,
        /// left, centre or right (for the lines against each other).
        #[arg(long, default_value = "centre")]
        align: String,
        /// Extra space between lines, percent of the size.
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        line_gap: f64,
        /// Extra space between letters, percent of the size.
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        tracking: f64,
        /// Outline colour.
        #[arg(long)]
        outline: Option<String>,
        /// Outline width, percent of the frame height.
        #[arg(long, default_value_t = 0.5)]
        outline_width: f64,
        /// Drop shadow colour.
        #[arg(long)]
        shadow: Option<String>,
        /// Shadow offset right and down, percent of the frame height.
        #[arg(long, default_value_t = 0.5, allow_hyphen_values = true)]
        shadow_x: f64,
        #[arg(long, default_value_t = 0.5, allow_hyphen_values = true)]
        shadow_y: f64,
    },
}

fn colour(text: &str) -> Result<Colour, String> {
    Colour::parse(text).ok_or_else(|| {
        format!("'{text}' is not a colour; use #rrggbb, #rrggbbaa or a name like red")
    })
}

fn generator(kind: GfxKind) -> Result<Generator, String> {
    Ok(match kind {
        GfxKind::Solid { colour: c } => Generator::Fill(Fill::Solid(colour(&c)?)),
        GfxKind::Gradient { from, to, angle } => Generator::Fill(Fill::Linear {
            from: colour(&from)?,
            to: colour(&to)?,
            angle_deg: angle,
        }),
        GfxKind::Shape {
            shape,
            width,
            height,
            fill,
            fill_to,
            angle,
            stroke,
            stroke_width,
            corner,
            sides,
            inner,
        } => {
            let kind = match shape.to_ascii_lowercase().as_str() {
                "rect" | "rectangle" | "box" => ShapeKind::Rect { corner_pct: corner },
                "ellipse" | "circle" | "oval" => ShapeKind::Ellipse,
                "polygon" => ShapeKind::Polygon { sides },
                "star" => ShapeKind::Star {
                    points: sides,
                    inner_pct: inner,
                },
                other => {
                    return Err(format!(
                        "unknown shape '{other}'; use rect, ellipse, polygon or star"
                    ))
                }
            };
            let from = colour(&fill)?;
            let fill = match fill_to {
                Some(to) => Fill::Linear {
                    from,
                    to: colour(&to)?,
                    angle_deg: angle,
                },
                None => Fill::Solid(from),
            };
            Generator::Shape(Shape {
                kind,
                width_pct: width,
                height_pct: height,
                fill,
                stroke: stroke
                    .map(|c| {
                        Ok::<_, String>(Stroke {
                            colour: colour(&c)?,
                            width_pct: stroke_width,
                        })
                    })
                    .transpose()?,
            })
        }
        GfxKind::Text {
            text,
            font,
            size,
            colour: c,
            align,
            line_gap,
            tracking,
            outline,
            outline_width,
            shadow,
            shadow_x,
            shadow_y,
        } => Generator::Text(Text {
            text: text.replace("\\n", "\n"),
            font,
            size_pct: size,
            colour: colour(&c)?,
            align: Align::parse(&align.to_ascii_lowercase())
                .ok_or("align must be left, centre or right")?,
            line_gap_pct: line_gap,
            tracking_pct: tracking,
            outline: outline
                .map(|c| {
                    Ok::<_, String>(Stroke {
                        colour: colour(&c)?,
                        width_pct: outline_width,
                    })
                })
                .transpose()?,
            shadow: shadow
                .map(|c| {
                    Ok::<_, String>(Shadow {
                        colour: colour(&c)?,
                        dx_pct: shadow_x,
                        dy_pct: shadow_y,
                    })
                })
                .transpose()?,
        }),
    })
}

fn describe(g: &Generator) -> String {
    match g {
        Generator::Fill(Fill::Solid(c)) => format!("solid {}", c.hex()),
        Generator::Fill(Fill::Linear { from, to, .. }) => {
            format!("gradient {} to {}", from.hex(), to.hex())
        }
        Generator::Shape(s) => format!(
            "{} {}x{}%",
            match s.kind {
                ShapeKind::Rect { .. } => "rect",
                ShapeKind::Ellipse => "ellipse",
                ShapeKind::Polygon { .. } => "polygon",
                ShapeKind::Star { .. } => "star",
            },
            s.width_pct,
            s.height_pct
        ),
        Generator::Text(t) => {
            let flat = t.text.replace('\n', " / ");
            let short: String = flat.chars().take(30).collect();
            format!(
                "text \"{short}{}\"",
                if flat.chars().count() > 30 { "..." } else { "" }
            )
        }
    }
}

#[derive(Subcommand)]
enum FxCmd {
    /// List the effects and their settings.
    Kinds,
    /// Add an effect to a picture clip (at the end of its stack, or `--at` a position).
    Add {
        clip: String,
        /// A built-in effect (see `effect kinds`) or `plugin:<id>` for an installed plugin.
        kind: String,
        #[arg(long)]
        at: Option<usize>,
        /// The folder of plugins, for `plugin:` effects. Default: $ORIGINFX_PLUGINS or ./plugins.
        #[arg(long)]
        plugins: Option<PathBuf>,
    },
    /// Remove an effect from a clip.
    Remove { clip: String, effect: String },
    /// Move an effect to a position in the stack (0 is applied first).
    Move {
        clip: String,
        effect: String,
        to: usize,
    },
    /// Turn an effect on, or `--off`.
    On {
        clip: String,
        effect: String,
        #[arg(long)]
        off: bool,
    },
    /// Set a setting to a fixed value, for example `effect set clip8 fx1 radius 3`.
    Set {
        clip: String,
        effect: String,
        name: String,
        #[arg(allow_hyphen_values = true)]
        value: f64,
    },
    /// Keyframe a setting at a time on the timeline (seconds).
    Key {
        clip: String,
        effect: String,
        name: String,
        at: f64,
        #[arg(allow_hyphen_values = true)]
        value: f64,
        /// hold, linear or smooth.
        #[arg(long, default_value = "linear")]
        interp: String,
    },
    /// Remove the keyframe of a setting at exactly this time.
    Unkey {
        clip: String,
        effect: String,
        name: String,
        at: f64,
    },
    /// Remove all keyframes of a setting; its fixed value applies again.
    ClearKeys {
        clip: String,
        effect: String,
        name: String,
    },
}

fn fx_id(text: &str) -> Result<EffectId, String> {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    digits
        .parse()
        .map(EffectId)
        .map_err(|_| format!("'{text}' is not an effect id (like fx1)"))
}

fn id_number(text: &str) -> Result<u64, String> {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    digits.parse().map_err(|_| format!("'{text}' is not an id"))
}

fn parse_gain(text: &str) -> Result<Gain, String> {
    let t = text
        .trim()
        .trim_end_matches("dB")
        .trim_end_matches("db")
        .trim();
    if t.eq_ignore_ascii_case("-inf") {
        return Ok(Gain::SILENT);
    }
    t.parse::<f64>()
        .ok()
        .and_then(Gain::from_db)
        .ok_or_else(|| format!("'{text}' is not a level in decibels (at most +24)"))
}

/// What a keyframe command points at: `clip7`, `a1` (an audio track) or `master`.
fn parse_target(p: &Project, seq: SequenceId, text: &str) -> Result<Target, String> {
    let t = text.trim().to_ascii_lowercase();
    if t == "master" {
        return Ok(Target::Master);
    }
    if let Some(n) = t.strip_prefix('a').and_then(|n| n.parse::<usize>().ok()) {
        return track_id(p, seq, TrackKind::Audio, n).map(Target::Track);
    }
    if t.starts_with('v') && t[1..].parse::<usize>().is_ok() {
        return Err("picture tracks have no animated values; target a clip instead".into());
    }
    Ok(Target::Clip(ClipId(id_number(&t)?)))
}

fn parse_param(text: &str) -> Result<Param, String> {
    Param::parse(&text.trim().to_ascii_lowercase()).ok_or_else(|| {
        let names: Vec<&str> = Param::ALL.iter().map(|p| p.as_str()).collect();
        format!("'{text}' is not a value; try {}", names.join(", "))
    })
}

/// A keyframe value in the unit of its parameter: dB (or `-inf`), percent, or a speed ratio.
fn parse_value(param: Param, text: &str) -> Result<f64, String> {
    let t = text.trim();
    let t = match param {
        Param::Gain => t.trim_end_matches("dB").trim_end_matches("db").trim(),
        Param::Pan => t.trim_end_matches('%').trim(),
        Param::Speed => t.trim_end_matches('x').trim(),
        Param::Rotation => t.trim_end_matches("deg").trim_end_matches('°').trim(),
        _ => t.trim_end_matches('%').trim(),
    };
    if param == Param::Gain && t.eq_ignore_ascii_case("-inf") {
        return Ok(param.range().0);
    }
    t.parse::<f64>()
        .map_err(|_| format!("'{text}' is not a number for {param}"))
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
            video_tracks,
            audio_tracks,
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
                video_tracks: video_tracks as _,
                audio_tracks: audio_tracks as _,
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
        Cmd::Gain { clip, db } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let g = parse_gain(&db)?;
            ed.set_clip_audio(
                seq,
                &[ClipId(id_number(&clip)?)],
                ClipAudio {
                    gain: Some(g),
                    ..Default::default()
                },
                LinkMode::Follow,
            )?;
            save(&ed, &path, before)?;
            println!("gain {g}");
        }
        Cmd::Pan { clip, percent } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let p = Pan::new(percent).ok_or("pan must be between -100 and 100")?;
            ed.set_clip_audio(
                seq,
                &[ClipId(id_number(&clip)?)],
                ClipAudio {
                    pan: Some(p),
                    ..Default::default()
                },
                LinkMode::Follow,
            )?;
            save(&ed, &path, before)?;
            println!("pan {p}");
        }
        Cmd::Fade {
            clip,
            fade_in,
            fade_out,
            curve,
        } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let curve =
                FadeCurve::parse(&curve).ok_or("curve must be linear, equal-power or smooth")?;
            let make = |s: Option<f64>| {
                s.map(|s| {
                    (s > 0.0).then(|| Fade {
                        duration: secs(s),
                        curve,
                    })
                })
            };
            ed.set_clip_audio(
                seq,
                &[ClipId(id_number(&clip)?)],
                ClipAudio {
                    fade_in: make(fade_in),
                    fade_out: make(fade_out),
                    ..Default::default()
                },
                LinkMode::Follow,
            )?;
            save(&ed, &path, before)?;
            println!("faded");
        }
        Cmd::Track { number, gain, pan } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let tid = track_id(ed.project(), seq, TrackKind::Audio, number)?;
            let update = TrackUpdate {
                gain: gain.as_deref().map(parse_gain).transpose()?,
                pan: pan
                    .map(|p| Pan::new(p).ok_or("pan must be between -100 and 100"))
                    .transpose()?,
                ..Default::default()
            };
            ed.update_track(seq, tid, update)?;
            save(&ed, &path, before)?;
            println!("track {number} updated");
        }
        Cmd::Master { db, normalize } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let g = match (db, normalize) {
                (Some(db), _) => parse_gain(&db)?,
                (None, Some(target)) => {
                    eprintln!("measuring...");
                    let l = crate::audio::measure_sequence(ed.project(), seq, |_| {})?;
                    let need = l
                        .gain_to_reach(target)
                        .ok_or("the mix is silent, so there is nothing to normalise")?;
                    let current = ed
                        .project()
                        .sequence(seq)
                        .map_or(0.0, |s| s.master_gain.db());
                    println!(
                        "measured {:.1} LUFS, peak {:.1} dBFS",
                        l.integrated_lufs.unwrap_or(0.0),
                        l.sample_peak_db
                    );
                    if l.sample_peak_db + need > 0.0 {
                        eprintln!(
                            "note: the peaks will reach {:+.1} dBFS; render with --limiter soft or lower the target",
                            l.sample_peak_db + need
                        );
                    }
                    Gain::from_db(f64::from(current + need)).ok_or_else(|| {
                        format!(
                            "that needs {:+.1} dB of master gain, more than the +24 dB maximum",
                            current + need
                        )
                    })?
                }
                (None, None) => return Err("give a level in dB or --normalize <LUFS>".into()),
            };
            ed.set_master_gain(seq, g)?;
            save(&ed, &path, before)?;
            println!("master {g}");
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
        Cmd::Key {
            target,
            param,
            at,
            value,
            interp,
        } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let target = parse_target(ed.project(), seq, &target)?;
            let param = parse_param(&param)?;
            let v = parse_value(param, &value)?;
            let interp = Interp::parse(&interp).ok_or("interp must be hold, linear or smooth")?;
            ed.set_keyframe(seq, target, param, secs(at), v, interp, LinkMode::Follow)?;
            save(&ed, &path, before)?;
            println!("{param} {v}{} at {}", param.unit(), secs(at));
        }
        Cmd::Unkey { target, param, at } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let target = parse_target(ed.project(), seq, &target)?;
            let param = parse_param(&param)?;
            ed.remove_keyframe(seq, target, param, secs(at), LinkMode::Follow)?;
            save(&ed, &path, before)?;
            println!("removed");
        }
        Cmd::ClearKeys { target, param } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let target = parse_target(ed.project(), seq, &target)?;
            let param = parse_param(&param)?;
            ed.clear_animation(seq, target, param, LinkMode::Follow)?;
            save(&ed, &path, before)?;
            println!("cleared");
        }
        Cmd::Look { clip, param, value } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let param = parse_param(&param)?;
            if !param.is_look() {
                return Err(format!(
                    "{param} is not a picture setting; use `clip-audio` or `speed`"
                )
                .into());
            }
            let v = parse_value(param, &value)?;
            ed.set_look_value(seq, ClipId(id_number(&clip)?), param, v)?;
            save(&ed, &path, before)?;
            println!("{param} {v}{}", param.unit());
        }
        Cmd::Blend { clip, mode } => {
            if mode.eq_ignore_ascii_case("list") {
                for m in timeline_engine::BlendMode::ALL {
                    println!("{}", m.name());
                }
                return Ok(());
            }
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let m = timeline_engine::BlendMode::parse(&mode.to_ascii_lowercase())
                .ok_or_else(|| format!("unknown blend mode '{mode}'; try `blend x list`"))?;
            ed.set_blend_mode(seq, ClipId(id_number(&clip)?), m)?;
            save(&ed, &path, before)?;
            println!("{}", m.name());
        }
        Cmd::Mask {
            clip,
            shape,
            invert,
        } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            let seq = first_sequence(ed.project())?;
            let id = ClipId(id_number(&clip)?);
            if shape.eq_ignore_ascii_case("off") {
                ed.remove_mask(seq, id)?;
                println!("mask removed");
            } else {
                let s = timeline_engine::MaskShape::parse(&shape.to_ascii_lowercase())
                    .ok_or("shape must be rect, ellipse or off")?;
                ed.set_mask(seq, id, s, invert)?;
                println!(
                    "{} mask{}",
                    s.name(),
                    if invert { " (inverted)" } else { "" }
                );
            }
            save(&ed, &path, before)?;
        }
        Cmd::Graphic { cmd } => {
            let mut ed = load(&path)?;
            let before = transitions(ed.project());
            match cmd {
                GfxCmd::Add { name, kind } => {
                    let id = ed.add_generated(name, generator(kind)?)?;
                    save(&ed, &path, before)?;
                    println!("{id}");
                }
                GfxCmd::Set { asset, kind } => {
                    let id = AssetId(id_number(&asset)?);
                    ed.set_generated(id, generator(kind)?)?;
                    save(&ed, &path, before)?;
                    println!("updated {id}");
                }
            }
        }
        Cmd::Effect { cmd } => match cmd {
            FxCmd::Kinds => {
                for k in EffectKind::ALL {
                    let settings: Vec<String> = k
                        .params()
                        .iter()
                        .map(|s| {
                            format!(
                                "{} ({} to {}, default {} {})",
                                s.name, s.min, s.max, s.default, s.unit
                            )
                        })
                        .collect();
                    println!("{:<12} {}", k.name(), settings.join("; "));
                }
            }
            other => {
                let mut ed = load(&path)?;
                let before = transitions(ed.project());
                let seq = first_sequence(ed.project())?;
                match other {
                    FxCmd::Kinds => {}
                    FxCmd::Add {
                        clip,
                        kind,
                        at,
                        plugins,
                    } => {
                        let clip = ClipId(id_number(&clip)?);
                        let id = if let Some(pid) = kind.strip_prefix("plugin:") {
                            let host = crate::plugin::host(plugins)?;
                            let p = host.get(pid).map_err(|e| {
                                format!("{e}; see `originfx plugin list` (use --plugins DIR)")
                            })?;
                            let m = p.manifest();
                            let spec = PluginSpec {
                                id: m.id.clone(),
                                name: m.name.clone(),
                                version: m.version.clone(),
                                settings: m
                                    .params
                                    .iter()
                                    .map(|d| Setting {
                                        name: d.name.clone(),
                                        unit: d.unit.clone(),
                                        default: d.default,
                                        min: d.min,
                                        max: d.max,
                                    })
                                    .collect(),
                            };
                            ed.add_plugin_effect(seq, clip, spec, at)?
                        } else {
                            let k =
                                EffectKind::parse(&kind.to_ascii_lowercase()).ok_or_else(|| {
                                    format!("unknown effect '{kind}'; see `effect kinds`")
                                })?;
                            ed.add_effect(seq, clip, k, at)?
                        };
                        println!("{id}");
                    }
                    FxCmd::Remove { clip, effect } => {
                        ed.remove_effect(seq, ClipId(id_number(&clip)?), fx_id(&effect)?)?;
                        println!("removed");
                    }
                    FxCmd::Move { clip, effect, to } => {
                        ed.move_effect(seq, ClipId(id_number(&clip)?), fx_id(&effect)?, to)?;
                        println!("moved");
                    }
                    FxCmd::On { clip, effect, off } => {
                        ed.set_effect_enabled(
                            seq,
                            ClipId(id_number(&clip)?),
                            fx_id(&effect)?,
                            !off,
                        )?;
                        println!("{}", if off { "off" } else { "on" });
                    }
                    FxCmd::Set {
                        clip,
                        effect,
                        name,
                        value,
                    } => {
                        ed.set_effect_value(
                            seq,
                            ClipId(id_number(&clip)?),
                            fx_id(&effect)?,
                            &name,
                            value,
                        )?;
                        println!("{name} {value}");
                    }
                    FxCmd::Key {
                        clip,
                        effect,
                        name,
                        at,
                        value,
                        interp,
                    } => {
                        let interp = Interp::parse(&interp)
                            .ok_or("interp must be hold, linear or smooth")?;
                        ed.key_effect(
                            seq,
                            ClipId(id_number(&clip)?),
                            fx_id(&effect)?,
                            &name,
                            secs(at),
                            value,
                            interp,
                        )?;
                        println!("{name} {value} at {}", secs(at));
                    }
                    FxCmd::Unkey {
                        clip,
                        effect,
                        name,
                        at,
                    } => {
                        ed.unkey_effect(
                            seq,
                            ClipId(id_number(&clip)?),
                            fx_id(&effect)?,
                            &name,
                            secs(at),
                        )?;
                        println!("removed");
                    }
                    FxCmd::ClearKeys { clip, effect, name } => {
                        ed.clear_effect_animation(
                            seq,
                            ClipId(id_number(&clip)?),
                            fx_id(&effect)?,
                            &name,
                        )?;
                        println!("cleared");
                    }
                }
                save(&ed, &path, before)?;
            }
        },
        Cmd::Show => {
            let p = Project::load(&path)?;
            println!("{}", p.name);
            for a in p.assets().values() {
                let len = match (&a.generator, a.duration) {
                    (Some(g), _) => format!("graphic: {}", describe(g)),
                    (None, Some(d)) => d.to_string(),
                    (None, None) => "still".to_string(),
                };
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
                    let anim: String = t
                        .anim
                        .iter()
                        .map(|(p, c)| format!(" {p}:{}keys", c.len()))
                        .collect();
                    println!("  {}{flags}{anim}", t.name);
                    for c in t.clips() {
                        let mut notes = String::new();
                        if !c.speed.is_normal() && !c.is_ramped() {
                            notes += &format!(" {}", c.speed);
                        }
                        for (param, curve) in c.anim.iter() {
                            notes += &format!(" {param}:{}keys", curve.len());
                        }
                        if c.reverse {
                            notes += " reversed";
                        }
                        if !c.transform.is_default() {
                            let t = &c.transform;
                            notes +=
                                &format!(
                                " [look opacity {} x {} y {} scale {}x{} rot {} crop {}/{}/{}/{}]",
                                t.opacity, t.x, t.y, t.scale_x, t.scale_y, t.rotation,
                                t.crop_left, t.crop_right, t.crop_top, t.crop_bottom
                            );
                        }
                        if c.blend != timeline_engine::BlendMode::Normal {
                            notes += &format!(" [blend {}]", c.blend.name());
                        }
                        if let Some(m) = &c.mask {
                            notes += &format!(
                                " [{} mask{} at {},{} size {}x{} feather {}]",
                                m.shape.name(),
                                if m.invert { " inverted" } else { "" },
                                m.x,
                                m.y,
                                m.width,
                                m.height,
                                m.feather
                            );
                        }
                        for e in &c.effects {
                            notes += &format!(
                                " [{} {}{}]",
                                e.id,
                                e.label(),
                                if e.enabled { "" } else { " off" }
                            );
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
                for (param, curve) in s.master_anim.iter() {
                    println!("  master {param}: {} keys", curve.len());
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
