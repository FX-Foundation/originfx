// SPDX-License-Identifier: GPL-3.0-or-later
//! `originfx render` and `originfx render-frame`: turn a timeline project into pictures and files.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use clap::{Args, ValueEnum};
use media_engine::{ExportPreset, ExportSettings};
use render_engine::{
    export_sequence, ExportOptions, FileSource, Fit, MissingMedia, Phase, RenderSettings, Renderer,
};
use timeline_engine::{Project, SequenceId, Time, TimeRange};

use crate::{human_bytes, png, preset_from_extension, CliResult, HwArg, QualityArg};

#[derive(Clone, Copy, ValueEnum)]
pub enum FitArg {
    /// Show all of the picture, with bars if the shape differs.
    Contain,
    /// Fill the frame and cut off the overhang.
    Cover,
    /// Fill the frame by stretching.
    Stretch,
}

impl From<FitArg> for Fit {
    fn from(f: FitArg) -> Fit {
        match f {
            FitArg::Contain => Fit::Contain,
            FitArg::Cover => Fit::Cover,
            FitArg::Stretch => Fit::Stretch,
        }
    }
}

#[derive(Args)]
pub struct RenderArgs {
    /// The project file (`.originfx`).
    project: PathBuf,
    /// The file to make.
    output: PathBuf,
    /// Sequence id number (default: the first sequence).
    #[arg(long)]
    sequence: Option<u64>,
    /// Preset id, see `originfx presets` (guessed from the output extension if omitted).
    #[arg(short, long)]
    preset: Option<String>,
    #[arg(short, long, value_enum, default_value = "standard")]
    quality: QualityArg,
    /// Scale down to this height (keeps aspect ratio, never upscales).
    #[arg(long)]
    height: Option<u32>,
    /// Video bitrate in kbit/s (overrides quality).
    #[arg(long)]
    video_kbps: Option<u64>,
    /// Audio bitrate in kbit/s.
    #[arg(long)]
    audio_kbps: Option<u64>,
    /// Render only from this time (seconds).
    #[arg(long)]
    from: Option<f64>,
    /// Render only up to this time (seconds).
    #[arg(long)]
    to: Option<f64>,
    #[arg(long, value_enum, default_value = "contain")]
    fit: FitArg,
    /// Show missing media as magenta and silence instead of stopping.
    #[arg(long)]
    allow_offline: bool,
    /// GPU video encoding (NVENC): off, auto or cuda.
    #[arg(long, value_enum, default_value = "off")]
    hw: HwArg,
    /// Where to put the large temporary working file (default: the system temp folder).
    #[arg(long)]
    temp_dir: Option<PathBuf>,
    /// Keep the working file (lossless, large) instead of deleting it.
    #[arg(long)]
    keep_working_file: bool,
}

#[derive(Args)]
pub struct FrameArgs {
    /// The project file (`.originfx`).
    project: PathBuf,
    /// Time in seconds.
    #[arg(long, default_value_t = 0.0)]
    at: f64,
    #[arg(short, long, default_value = "frame.png")]
    out: PathBuf,
    #[arg(long)]
    sequence: Option<u64>,
    #[arg(long, value_enum, default_value = "contain")]
    fit: FitArg,
    #[arg(long)]
    allow_offline: bool,
}

fn pick_sequence(p: &Project, wanted: Option<u64>) -> Result<SequenceId, String> {
    match wanted {
        Some(n) => {
            let id = SequenceId(n);
            p.sequence(id)
                .map(|_| id)
                .ok_or_else(|| format!("no sequence {n}"))
        }
        None => p
            .sequences()
            .keys()
            .next()
            .copied()
            .ok_or_else(|| "the project has no sequence".to_string()),
    }
}

fn missing(allow: bool) -> MissingMedia {
    if allow {
        MissingMedia::Offline
    } else {
        MissingMedia::Fail
    }
}

pub fn run(a: RenderArgs) -> CliResult {
    let project = Project::load(&a.project)?;
    let seq = pick_sequence(&project, a.sequence)?;
    let preset = match &a.preset {
        Some(id) => ExportPreset::from_id(id)
            .ok_or_else(|| format!("unknown preset '{id}', try `originfx presets`"))?,
        None => preset_from_extension(&a.output)?,
    };
    let mut s = ExportSettings::new(preset).quality(a.quality.into());
    s.height = a.height;
    s.video_bitrate = a.video_kbps.map(|k| k * 1000);
    s.audio_bitrate = a.audio_kbps.map(|k| k * 1000);
    s.hw_encode = a.hw.into();

    let range = match (a.from, a.to) {
        (None, None) => None,
        (from, to) => {
            let end = match to {
                Some(t) => Time::from_secs_f64(t),
                None => project.sequence(seq).map_or(Time::ZERO, |q| q.duration()),
            };
            Some(
                TimeRange::new(Time::from_secs_f64(from.unwrap_or(0.0)), end)
                    .ok_or("--from must be before --to")?,
            )
        }
    };
    let options = ExportOptions {
        render: RenderSettings {
            fit: a.fit.into(),
            missing_media: missing(a.allow_offline),
            ..Default::default()
        },
        range,
        temp_dir: a.temp_dir.clone(),
        keep_working_file: a.keep_working_file,
    };

    let tty = std::io::stderr().is_terminal();
    let report = export_sequence(&project, seq, &a.output, &s, &options, |p| {
        if tty {
            let label = match p.phase {
                Phase::Rendering => "rendering",
                Phase::Encoding => "encoding ",
            };
            match p.fraction {
                Some(f) => eprint!("\r{label} {:>5.1}%  {} frames   ", f * 100.0, p.frames),
                None => eprint!("\r{label} {} frames   ", p.frames),
            }
            let _ = std::io::stderr().flush();
        }
        true
    })?;
    if tty {
        eprintln!();
    }
    println!(
        "{} -> {}  [{}]  {} frames, {:.1}s, {}  (render {:.1}s, encode {:.1}s)",
        a.project.display(),
        a.output.display(),
        preset.label(),
        report.frames,
        report.duration_secs,
        human_bytes(report.encode.output_bytes),
        report.render_time.as_secs_f64(),
        report.encode.elapsed.as_secs_f64()
    );
    if let Some(enc) = &report.encode.video_encoder {
        println!("video encoder: {enc}");
    }
    if let Some(path) = &report.working_file {
        println!("working file kept: {}", path.display());
    }
    Ok(())
}

pub fn run_frame(a: FrameArgs) -> CliResult {
    let project = Project::load(&a.project)?;
    let seq = pick_sequence(&project, a.sequence)?;
    let settings = RenderSettings {
        fit: a.fit.into(),
        missing_media: missing(a.allow_offline),
        ..Default::default()
    };
    let mut r = Renderer::new(&project, seq, FileSource::new(), settings)?;
    let frame = r.frame_at(Time::from_secs_f64(a.at))?;
    png::write_rgba(&a.out, frame.width, frame.height, &frame.data)?;
    println!(
        "saved {}x{} frame at {:.3}s to {}",
        frame.width,
        frame.height,
        frame.pts_secs,
        a.out.display()
    );
    Ok(())
}
