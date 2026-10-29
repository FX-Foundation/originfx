// SPDX-License-Identifier: GPL-3.0-or-later
//! `originfx loudness`: measure a media file or the mix of a project.

use std::path::{Path, PathBuf};

use audio_engine::{Limiter, Loudness, LoudnessMeter};
use clap::Args;
use media_engine::{AudioDecoder, AudioOptions};
use render_engine::{FileSource, MissingMedia, RenderSettings, Renderer};
use timeline_engine::{Project, SequenceId};

use crate::CliResult;

#[derive(Args)]
pub struct LoudnessArgs {
    /// A media file, or a project file (`.originfx`) to measure the mix of.
    file: PathBuf,
    /// Sequence id number (projects only; default: the first).
    #[arg(long)]
    sequence: Option<u64>,
}

/// Measure the mix of one sequence, with no limiting so a hot mix is measured as it is.
pub fn measure_sequence(
    project: &Project,
    seq: SequenceId,
    mut progress: impl FnMut(f64),
) -> Result<Loudness, Box<dyn std::error::Error>> {
    let s = project.sequence(seq).ok_or("no such sequence")?;
    let (rate, total) = (s.sample_rate, s.duration().to_samples_round(s.sample_rate));
    let mut r = Renderer::new(
        project,
        seq,
        FileSource::new(),
        RenderSettings {
            limiter: Limiter::Off,
            missing_media: MissingMedia::Fail,
            ..Default::default()
        },
    )?;
    let mut meter = LoudnessMeter::new(rate, 2);
    let step = i64::from(rate);
    let mut at = 0i64;
    while at < total {
        let n = step.min(total - at);
        meter.push(&r.audio_samples(at, n as usize)?);
        at += n;
        progress(at as f64 / total.max(1) as f64);
    }
    Ok(meter.result())
}

fn measure_file(path: &Path) -> Result<Loudness, Box<dyn std::error::Error>> {
    let mut dec = AudioDecoder::open(path, AudioOptions::default())?;
    let mut meter: Option<LoudnessMeter> = None;
    while let Some(b) = dec.next_buffer()? {
        meter
            .get_or_insert_with(|| LoudnessMeter::new(b.sample_rate, usize::from(b.channels)))
            .push(&b.samples);
    }
    Ok(meter.ok_or("the file has no sound")?.result())
}

pub fn print(l: &Loudness) {
    match l.integrated_lufs {
        Some(i) => println!("integrated loudness: {i:.1} LUFS"),
        None => println!("integrated loudness: none (silent)"),
    }
    if let Some(m) = l.momentary_max_lufs {
        println!("loudest 400 ms:      {m:.1} LUFS");
    }
    println!("sample peak:         {:.1} dBFS", l.sample_peak_db);
    println!("rms:                 {:.1} dBFS", l.rms_db);
    println!("length:              {:.1} s", l.duration_secs);
}

pub fn run(a: LoudnessArgs) -> CliResult {
    let is_project = a
        .file
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("originfx"));
    let l = if is_project {
        let p = Project::load(&a.file)?;
        let seq = match a.sequence {
            Some(n) => SequenceId(n),
            None => *p
                .sequences()
                .keys()
                .next()
                .ok_or("the project has no sequence")?,
        };
        measure_sequence(&p, seq, |_| {})?
    } else {
        measure_file(&a.file)?
    };
    print(&l);
    Ok(())
}
