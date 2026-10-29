// SPDX-License-Identifier: GPL-3.0-or-later
//! Turning a sequence into a finished file.
//!
//! Frames and sound are made one frame at a time and handed to the Media Engine's exporter, so
//! every preset, quality setting, GPU encoder and progress report works for rendered timelines.
//!
//! On Unix the hand-over is an in-memory pipe: rendering and encoding run at the same time and
//! nothing is written to disk. Elsewhere, or when `keep_working_file` is set, the frames go to a
//! lossless working file first, which is large; `ExportOptions::temp_dir` can point it at a
//! roomy drive.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use media_engine::{
    export_with_progress, ExportReport, ExportSettings, VideoFrame, WorkingAudio,
    WorkingFileWriter, WorkingSpec,
};
use timeline_engine::{Project, SequenceId, Time, TimeRange};

use crate::error::{Error, Result};
use crate::renderer::{MissingMedia, RenderSettings, Renderer};
use crate::source::{FileSource, MediaSource};

/// What to render and where to put the temporary working file.
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub render: RenderSettings,
    /// Render only this part of the sequence (an in/out range). `None` renders everything.
    /// Times are moved to whole frames.
    pub range: Option<TimeRange>,
    /// Where to put the temporary working file, when one is used. `None` uses the system temp directory.
    pub temp_dir: Option<PathBuf>,
    /// Go through a lossless working file on disk (large) and keep it afterwards, instead of
    /// streaming. For debugging, and for rendering once to export several times.
    pub keep_working_file: bool,
}

/// Which stage an export is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Compositing frames and mixing sound (with the pipe, encoding runs alongside).
    Rendering,
    /// Finishing the file after the last frame has been rendered.
    Encoding,
}

/// Progress, reported separately for each phase (each runs from 0 to 1).
#[derive(Debug, Clone, Copy)]
pub struct RenderProgress {
    pub phase: Phase,
    /// 0.0 to 1.0 within the phase, if known.
    pub fraction: Option<f64>,
    /// Frames rendered so far (rendering) or encoded so far (encoding).
    pub frames: u64,
    /// Frames in the whole export.
    pub frames_total: u64,
    pub elapsed: Duration,
}

/// What an export produced.
#[derive(Debug, Clone)]
pub struct RenderReport {
    pub frames: u64,
    pub duration_secs: f64,
    /// Time from the start of the export to the last frame being handed over.
    pub render_time: Duration,
    pub encode: ExportReport,
    /// Set when `keep_working_file` was on.
    pub working_file: Option<PathBuf>,
}

/// Removes the working file when dropped, whatever happens.
struct WorkingFileGuard {
    path: PathBuf,
    keep: bool,
}

impl Drop for WorkingFileGuard {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn working_path(dir: Option<&Path>) -> PathBuf {
    let dir = dir.map_or_else(std::env::temp_dir, Path::to_path_buf);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    dir.join(format!(
        "originfx-working-{}-{nanos}.nut",
        std::process::id()
    ))
}

/// Export a sequence from files on disk. Fails before rendering anything if a used media file is
/// missing (unless `options.render.missing_media` is `Offline`).
pub fn export_sequence(
    project: &Project,
    seq: SequenceId,
    output: impl AsRef<Path>,
    settings: &ExportSettings,
    options: &ExportOptions,
    on_progress: impl FnMut(&RenderProgress) -> bool,
) -> Result<RenderReport> {
    if options.render.missing_media == MissingMedia::Fail {
        let s = project.sequence(seq).ok_or(Error::NoSuchSequence(seq))?;
        for (_, clip) in s.clips() {
            if let Some(a) = project.asset(clip.asset) {
                if !a.path.exists() {
                    return Err(Error::MissingMedia {
                        asset: a.id,
                        name: a.name.clone(),
                        path: a.path.clone(),
                    });
                }
            }
        }
    }
    export_with_source(
        project,
        seq,
        FileSource::new(),
        output,
        settings,
        options,
        on_progress,
    )
}

/// Everything fixed before the first frame is made.
struct Plan {
    first: i64,
    total: u64,
    want_video: bool,
    want_audio: bool,
    spec: WorkingSpec,
    rate: timeline_engine::FrameRate,
    sample_rate: u32,
}

/// Why feeding frames stopped.
enum Stop {
    /// Making a picture or the sound failed.
    Render(Error),
    /// Handing data over failed (the other end may have a better explanation).
    Write(Error),
    /// The caller cancelled, or the exporter finished early.
    Halt(Error),
}

/// Render every frame and hand it to `writer`. `after_frame(done)` runs after each frame.
fn feed<S: MediaSource>(
    renderer: &mut Renderer<S>,
    writer: &mut WorkingFileWriter,
    plan: &Plan,
    mut after_frame: impl FnMut(u64) -> std::result::Result<(), Error>,
) -> std::result::Result<(), Stop> {
    let blank = VideoFrame {
        width: plan.spec.width,
        height: plan.spec.height,
        pts_secs: 0.0,
        data: crate::composite::solid(
            plan.spec.width as usize,
            plan.spec.height as usize,
            [0, 0, 0, 255],
        ),
    };
    // Sample positions come from the absolute frame times, so rounding never accumulates.
    let sample_at = |frame: i64| {
        timeline_engine::Time::from_frames(frame, plan.rate).to_samples_round(plan.sample_rate)
    };
    for i in 0..plan.total {
        let frame_no = plan.first + i as i64;
        if plan.want_video {
            let f = renderer.frame(frame_no).map_err(Stop::Render)?;
            writer.write_video(&f).map_err(|e| Stop::Write(e.into()))?;
        } else {
            writer
                .write_video(&blank)
                .map_err(|e| Stop::Write(e.into()))?;
        }
        if plan.want_audio {
            let a0 = sample_at(frame_no);
            let a1 = sample_at(frame_no + 1);
            let samples = renderer
                .audio_samples(a0, (a1 - a0).max(0) as usize)
                .map_err(Stop::Render)?;
            writer
                .write_audio(&samples)
                .map_err(|e| Stop::Write(e.into()))?;
        }
        after_frame(i + 1).map_err(Stop::Halt)?;
    }
    Ok(())
}

/// Export a sequence using any [`MediaSource`].
///
/// The callback is called as the export proceeds; return `false` from it to cancel (the output
/// file is not left behind and [`Error::Cancelled`] is returned).
pub fn export_with_source<S: MediaSource>(
    project: &Project,
    seq: SequenceId,
    source: S,
    output: impl AsRef<Path>,
    settings: &ExportSettings,
    options: &ExportOptions,
    mut on_progress: impl FnMut(&RenderProgress) -> bool,
) -> Result<RenderReport> {
    let output = output.as_ref();
    let mut renderer = Renderer::new(project, seq, source, options.render)?;
    let s = renderer.sequence().clone();
    let rate = s.frame_rate;

    // The frames to render: whole frames from `first` for `total` frames.
    let range = options.range.unwrap_or(TimeRange {
        start: Time::ZERO,
        end: s.duration(),
    });
    let first = range.start.max(Time::ZERO).to_frames_round(rate);
    let last = range.end.to_frames_round(rate);
    if last <= first {
        return Err(Error::Empty);
    }
    let total = (last - first) as u64;

    let preset = settings.preset;
    let want_video = !preset.audio_only();
    let want_audio = preset.has_audio();
    let (w, h) = if want_video {
        (s.width, s.height)
    } else {
        (16, 16)
    };
    let plan = Plan {
        first,
        total,
        want_video,
        want_audio,
        spec: WorkingSpec {
            width: w,
            height: h,
            fps_num: rate.num(),
            fps_den: rate.den(),
            audio: want_audio.then_some(WorkingAudio {
                sample_rate: s.sample_rate,
                channels: 2,
            }),
        },
        rate,
        sample_rate: s.sample_rate,
    };
    let duration_secs =
        (Time::from_frames(last, rate) - Time::from_frames(first, rate)).as_secs_f64();

    #[cfg(unix)]
    if !options.keep_working_file {
        return export_streaming(
            &mut renderer,
            &plan,
            output,
            settings,
            duration_secs,
            &mut on_progress,
        );
    }
    export_through_file(
        &mut renderer,
        &plan,
        output,
        settings,
        options,
        duration_secs,
        &mut on_progress,
    )
}

#[cfg(unix)]
fn export_streaming<S: MediaSource>(
    renderer: &mut Renderer<S>,
    plan: &Plan,
    output: &Path,
    settings: &ExportSettings,
    duration_secs: f64,
    on_progress: &mut dyn FnMut(&RenderProgress) -> bool,
) -> Result<RenderReport> {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;

    struct Shared {
        encoded: AtomicU64,
        cancel: AtomicBool,
    }
    let started = Instant::now();
    let (mut writer, pipe) = WorkingFileWriter::create_pipe(plan.spec)?;
    let shared = Arc::new(Shared {
        encoded: AtomicU64::new(0),
        cancel: AtomicBool::new(false),
    });
    let exporter = {
        let shared = Arc::clone(&shared);
        let (out, st) = (output.to_path_buf(), settings.clone());
        std::thread::spawn(move || {
            media_engine::export_pipe_with_progress(pipe, out, &st, Some(duration_secs), |p| {
                shared.encoded.store(p.frames, Ordering::Relaxed);
                !shared.cancel.load(Ordering::Relaxed)
            })
        })
    };

    let mut last_report = Instant::now();
    let fed = feed(renderer, &mut writer, plan, |done| {
        if exporter.is_finished() {
            return Err(Error::Cancelled);
        }
        if done == plan.total || last_report.elapsed() >= Duration::from_millis(100) {
            last_report = Instant::now();
            let keep_going = on_progress(&RenderProgress {
                phase: Phase::Rendering,
                fraction: Some(done as f64 / plan.total as f64),
                frames: done,
                frames_total: plan.total,
                elapsed: started.elapsed(),
            });
            if !keep_going {
                return Err(Error::Cancelled);
            }
        }
        Ok(())
    });

    let finished = match fed {
        Ok(()) => writer.finish().map_err(|e| Stop::Write(e.into())),
        Err(stop) => {
            shared.cancel.store(true, Ordering::Relaxed);
            drop(writer); // the exporter sees the end of the data and stops
            Err(stop)
        }
    };
    let render_time = started.elapsed();

    if finished.is_ok() {
        // Everything is handed over; wait for the encoder to finish the file.
        while !exporter.is_finished() {
            std::thread::sleep(Duration::from_millis(50));
            let encoded = shared.encoded.load(Ordering::Relaxed);
            let keep_going = on_progress(&RenderProgress {
                phase: Phase::Encoding,
                fraction: Some((encoded as f64 / plan.total as f64).min(1.0)),
                frames: encoded,
                frames_total: plan.total,
                elapsed: started.elapsed(),
            });
            if !keep_going {
                shared.cancel.store(true, Ordering::Relaxed);
            }
        }
    }
    let exported = exporter
        .join()
        .map_err(|_| Error::Unsupported("the encoder thread panicked".into()))?;

    match (finished, exported) {
        (Ok(()), Ok(encode)) => Ok(RenderReport {
            frames: plan.total,
            duration_secs,
            render_time,
            encode,
            working_file: None,
        }),
        (Ok(()), Err(e)) => Err(e.into()),
        (Err(Stop::Render(e)), _) => {
            let _ = std::fs::remove_file(output);
            Err(e)
        }
        // The exporter stopping first explains a failed hand-over better than the broken pipe does.
        (Err(Stop::Write(_)), Err(why)) if !matches!(why, media_engine::Error::Cancelled) => {
            let _ = std::fs::remove_file(output);
            Err(why.into())
        }
        (Err(Stop::Write(e)), _) => {
            let _ = std::fs::remove_file(output);
            Err(e)
        }
        (Err(Stop::Halt(e)), exported) => {
            let _ = std::fs::remove_file(output);
            match (e, exported) {
                (Error::Cancelled, Err(why)) if !matches!(why, media_engine::Error::Cancelled) => {
                    Err(why.into())
                }
                (e, _) => Err(e),
            }
        }
    }
}

/// Render into a lossless working file, then export that.
fn export_through_file<S: MediaSource>(
    renderer: &mut Renderer<S>,
    plan: &Plan,
    output: &Path,
    settings: &ExportSettings,
    options: &ExportOptions,
    duration_secs: f64,
    on_progress: &mut dyn FnMut(&RenderProgress) -> bool,
) -> Result<RenderReport> {
    let working = WorkingFileGuard {
        path: working_path(options.temp_dir.as_deref()),
        keep: options.keep_working_file,
    };
    let started = Instant::now();
    let mut writer = WorkingFileWriter::create(&working.path, plan.spec)?;
    let mut last_report = Instant::now();
    let fed = feed(renderer, &mut writer, plan, |done| {
        if done == plan.total || last_report.elapsed() >= Duration::from_millis(100) {
            last_report = Instant::now();
            let keep_going = on_progress(&RenderProgress {
                phase: Phase::Rendering,
                fraction: Some(done as f64 / plan.total as f64),
                frames: done,
                frames_total: plan.total,
                elapsed: started.elapsed(),
            });
            if !keep_going {
                return Err(Error::Cancelled);
            }
        }
        Ok(())
    });
    match fed {
        Ok(()) => {}
        Err(Stop::Render(e) | Stop::Write(e) | Stop::Halt(e)) => return Err(e),
    }
    writer.finish()?;
    let render_time = started.elapsed();

    let encode_started = Instant::now();
    let encode = export_with_progress(&working.path, output, settings, |p| {
        on_progress(&RenderProgress {
            phase: Phase::Encoding,
            fraction: p.fraction(),
            frames: p.frames,
            frames_total: plan.total,
            elapsed: encode_started.elapsed(),
        })
    })?;
    Ok(RenderReport {
        frames: plan.total,
        duration_secs,
        render_time,
        encode,
        working_file: options.keep_working_file.then(|| working.path.clone()),
    })
}
