// SPDX-License-Identifier: GPL-3.0-or-later
//! End to end with real files: fixtures are written losslessly, edited into a timeline,
//! rendered, exported, and the results are decoded again and checked.
use std::path::{Path, PathBuf};

use media_engine::{
    probe, AudioDecoder, AudioOptions, ExportPreset, ExportSettings, FrameReader, VideoDecoder,
    VideoFrame, WorkingAudio, WorkingFileWriter, WorkingSpec,
};
use render_engine::{
    export_sequence, Error, ExportOptions, FileSource, MediaSource, MissingMedia, Phase,
    RenderSettings, Renderer,
};
use timeline_engine::media::probe_asset;
use timeline_engine::{
    Alignment, AssetId, ClipSpec, Editor, FrameRate, LinkMode, PlaceMode, Ripple, SequenceId,
    SequenceSettings, Speed, SpeedMode, Time, TimeRange, Transition, TransitionKind,
};

fn frame(w: u32, h: u32, rgba: [u8; 4]) -> VideoFrame {
    VideoFrame {
        width: w,
        height: h,
        pts_secs: 0.0,
        data: rgba
            .iter()
            .copied()
            .cycle()
            .take((w * h * 4) as usize)
            .collect(),
    }
}

/// A lossless clip: `seconds` long, every frame coloured by `colour(frame_number)`, with a
/// stereo ramp as sound (value = (sample % 10000) / 10000).
fn make_clip(
    path: &Path,
    (w, h): (u32, u32),
    fps: (u32, u32),
    seconds: u32,
    colour: impl Fn(u32) -> [u8; 4],
    audio: bool,
) {
    let spec = WorkingSpec {
        width: w,
        height: h,
        fps_num: fps.0,
        fps_den: fps.1,
        audio: audio.then_some(WorkingAudio {
            sample_rate: 48000,
            channels: 2,
        }),
    };
    let mut wr = WorkingFileWriter::create(path, spec).unwrap();
    let frames = (u64::from(seconds) * u64::from(fps.0)).div_ceil(u64::from(fps.1)) as u32;
    for n in 0..frames {
        wr.write_video(&frame(w, h, colour(n))).unwrap();
    }
    if audio {
        let samples: Vec<f32> = (0..48000 * seconds as usize)
            .flat_map(|i| {
                let v = (i % 10000) as f32 / 10000.0;
                [v, v]
            })
            .collect();
        wr.write_audio(&samples).unwrap();
    }
    wr.finish().unwrap();
}

fn colour_of(f: &VideoFrame) -> [u8; 3] {
    let i = ((f.height / 2 * f.width + f.width / 2) * 4) as usize;
    [f.data[i], f.data[i + 1], f.data[i + 2]]
}

fn near(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
    a.iter()
        .zip(b)
        .all(|(x, y)| (i32::from(*x) - i32::from(y)).abs() <= tol)
}

struct Show {
    dir: tempfile::TempDir,
    ed: Editor,
    seq: SequenceId,
}

fn show(settings: SequenceSettings) -> Show {
    let dir = tempfile::tempdir().unwrap();
    let mut ed = Editor::new("e2e");
    let seq = ed.add_sequence(settings).unwrap();
    Show { dir, ed, seq }
}

impl Show {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
    fn import(&mut self, path: &Path) -> AssetId {
        self.ed.add_asset(probe_asset(path).unwrap()).unwrap()
    }
    fn lay(&mut self, asset: AssetId, at: f64, from: f64, dur: f64, video: bool, audio: bool) {
        let s = self.ed.project().sequence(self.seq).unwrap();
        let (v, a) = (s.video_tracks()[0].id, s.audio_tracks()[0].id);
        self.ed
            .place_asset(
                self.seq,
                asset,
                Time::from_secs_f64(at),
                Time::from_secs_f64(from),
                Some(Time::from_secs_f64(dur)),
                video.then_some(v),
                audio.then_some(a),
                PlaceMode::Overwrite,
            )
            .unwrap();
    }
}

fn small() -> SequenceSettings {
    SequenceSettings {
        width: 64,
        height: 36,
        ..Default::default()
    }
}

fn rgb_clip(show: &Show, name: &str, rgba: [u8; 4]) -> PathBuf {
    let p = show.path(name);
    make_clip(&p, (64, 36), (24, 1), 2, |_| rgba, false);
    p
}

#[test]
fn file_source_reads_exact_frames_and_sound() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.nut");
    make_clip(&p, (32, 18), (24, 1), 3, |n| [n as u8 * 3, 0, 0, 255], true);
    let mut ed = Editor::new("x");
    let id = ed.add_asset(probe_asset(&p).unwrap()).unwrap();
    let asset = ed.project().asset(id).unwrap().clone();
    let mut src = FileSource::new();

    let f = src.video_frame(&asset, 0, 1.0).unwrap(); // frame 24
    assert_eq!(f.data[0], 72);
    let f = src.video_frame(&asset, 0, 2.5).unwrap(); // frame 60
    assert_eq!(f.data[0], 180);
    let f = src.video_frame(&asset, 0, 0.0).unwrap();
    assert_eq!(f.data[0], 0);

    let expect = |start: usize, n: usize| -> Vec<f32> {
        (start..start + n)
            .flat_map(|i| [(i % 10000) as f32 / 10000.0; 2])
            .collect()
    };
    // Sequential reads, then jumps forward and back.
    for (start, n) in [
        (0usize, 1000usize),
        (1000, 5000),
        (6000, 777),
        (96_000, 4800),
        (10_000, 3000),
        (143_000, 5000),
    ] {
        let got = src
            .audio(&asset, 0, start as f64 / 48000.0, n, 48000)
            .unwrap();
        let mut want = expect(start, n.min(144_000usize.saturating_sub(start)));
        want.resize(n * 2, 0.0); // silence past the end
        assert_eq!(got.len(), want.len());
        let worst = got
            .iter()
            .zip(&want)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        assert!(worst == 0.0, "start {start}: worst error {worst}");
    }
    // Before the start of the media there is silence.
    let early = src.audio(&asset, 0, 0.0, 10, 48000).unwrap();
    assert_eq!(early.len(), 20);
}

#[test]
fn missing_file_is_reported_by_name() {
    let mut ed = Editor::new("x");
    let id = ed
        .add_asset(timeline_engine::NewAsset {
            name: "gone".into(),
            path: "/definitely/not/here.mp4".into(),
            duration: Some(Time::from_secs_f64(1.0)),
            video: vec![timeline_engine::VideoProps {
                width: 8,
                height: 8,
                frame_rate: Some(FrameRate::FPS_24),
            }],
            audio: vec![],
        })
        .unwrap();
    let asset = ed.project().asset(id).unwrap().clone();
    match FileSource::new().video_frame(&asset, 0, 0.0) {
        Err(Error::MissingMedia { name, .. }) => assert_eq!(name, "gone"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn three_clips_export_to_mp4_with_the_right_pictures_and_length() {
    let mut s = show(small());
    let (r, g, b) = (
        rgb_clip(&s, "r.nut", [220, 0, 0, 255]),
        rgb_clip(&s, "g.nut", [0, 200, 0, 255]),
        rgb_clip(&s, "b.nut", [0, 0, 220, 255]),
    );
    let (r, g, b) = (s.import(&r), s.import(&g), s.import(&b));
    s.lay(r, 0.0, 0.0, 1.0, true, false);
    s.lay(g, 1.0, 0.5, 1.0, true, false);
    s.lay(b, 2.0, 0.0, 1.0, true, false);
    let out = s.path("out.mp4");
    let mut phases = Vec::new();
    let report = export_sequence(
        s.ed.project(),
        s.seq,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264),
        &ExportOptions {
            temp_dir: Some(s.dir.path().to_path_buf()),
            ..Default::default()
        },
        |p| {
            phases.push(p.phase);
            true
        },
    )
    .unwrap();
    assert_eq!(report.frames, 72);
    assert!(phases.contains(&Phase::Rendering) && phases.contains(&Phase::Encoding));
    let info = probe(&out).unwrap();
    let v = info.primary_video().unwrap();
    assert_eq!((v.width, v.height), (64, 36));
    assert!((info.duration_secs.unwrap() - 3.0).abs() < 0.1);
    let mut rd = FrameReader::open(&out, 1 << 24).unwrap();
    assert!(near(colour_of(&rd.frame_at(0.5).unwrap()), [220, 0, 0], 40));
    assert!(near(colour_of(&rd.frame_at(1.5).unwrap()), [0, 200, 0], 40));
    assert!(near(colour_of(&rd.frame_at(2.5).unwrap()), [0, 0, 220], 40));
    // The working file is gone.
    let leftovers: Vec<_> = std::fs::read_dir(s.dir.path())
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("originfx-working")
        })
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn sound_arrives_in_the_export_sample_for_sample() {
    let mut s = show(small());
    let a = s.path("a.nut");
    make_clip(&a, (64, 36), (24, 1), 3, |_| [10, 10, 10, 255], true);
    let id = s.import(&a);
    // 0.5 s of silence, then media seconds 1.0 to 2.0.
    s.lay(id, 0.5, 1.0, 1.0, true, true);
    let out = s.path("out.wav");
    export_sequence(
        s.ed.project(),
        s.seq,
        &out,
        &ExportSettings::new(ExportPreset::Wav),
        &ExportOptions::default(),
        |_| true,
    )
    .unwrap();
    let mut dec = AudioDecoder::open(
        &out,
        AudioOptions {
            sample_rate: Some(48000),
            channels: Some(2),
        },
    )
    .unwrap();
    let all = dec.read_all().unwrap();
    assert_eq!(all.samples.len() / 2, 72_000); // 1.5 s
    let at = |i: usize| all.samples[i * 2];
    assert!(at(1000).abs() < 1e-4); // silence
                                    // sequence sample 24000+k is media sample 48000+k; WAV is 16 bit, so allow its rounding.
    for k in [0usize, 1, 777, 5000, 47_999] {
        let want = ((48_000 + k) % 10000) as f32 / 10000.0;
        assert!(
            (at(24_000 + k) - want).abs() < 2e-4,
            "k={k}: {} vs {want}",
            at(24_000 + k)
        );
    }
}

#[test]
fn ntsc_rates_keep_sound_and_picture_the_same_length() {
    let mut s = show(SequenceSettings {
        width: 64,
        height: 36,
        frame_rate: FrameRate::FPS_29_97,
        ..Default::default()
    });
    let a = s.path("a.nut");
    make_clip(&a, (64, 36), (30000, 1001), 3, |_| [50, 60, 70, 255], true);
    let id = s.import(&a);
    s.lay(id, 0.0, 0.0, 2.0, true, true);
    let duration = s.ed.project().sequence(s.seq).unwrap().duration();
    let frames = duration.to_frames_round(FrameRate::FPS_29_97);
    let out = s.path("out.wav");
    export_sequence(
        s.ed.project(),
        s.seq,
        &out,
        &ExportSettings::new(ExportPreset::Wav),
        &ExportOptions::default(),
        |_| true,
    )
    .unwrap();
    let all = AudioDecoder::open(
        &out,
        AudioOptions {
            sample_rate: Some(48000),
            channels: Some(2),
        },
    )
    .unwrap()
    .read_all()
    .unwrap();
    let want = Time::from_frames(frames, FrameRate::FPS_29_97).to_samples_round(48000) as usize;
    assert_eq!(all.samples.len() / 2, want);
}

#[test]
fn in_out_range_exports_only_that_part() {
    let mut s = show(small());
    let (r, b) = (
        rgb_clip(&s, "r.nut", [220, 0, 0, 255]),
        rgb_clip(&s, "b.nut", [0, 0, 220, 255]),
    );
    let (r, b) = (s.import(&r), s.import(&b));
    s.lay(r, 0.0, 0.0, 1.0, true, false);
    s.lay(b, 1.0, 0.0, 1.0, true, false);
    let out = s.path("part.mp4");
    let report = export_sequence(
        s.ed.project(),
        s.seq,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264),
        &ExportOptions {
            range: TimeRange::new(Time::from_secs_f64(0.5), Time::from_secs_f64(1.5)),
            ..Default::default()
        },
        |_| true,
    )
    .unwrap();
    assert_eq!(report.frames, 24);
    let mut rd = FrameReader::open(&out, 1 << 24).unwrap();
    assert!(near(colour_of(&rd.frame_at(0.2).unwrap()), [220, 0, 0], 40));
    assert!(near(colour_of(&rd.frame_at(0.8).unwrap()), [0, 0, 220], 40));
}

#[test]
fn cancel_leaves_nothing_behind() {
    let mut s = show(small());
    let r = rgb_clip(&s, "r.nut", [220, 0, 0, 255]);
    let r = s.import(&r);
    s.lay(r, 0.0, 0.0, 1.9, true, false);
    let work = tempfile::tempdir().unwrap();
    let out = s.path("never.mp4");
    let res = export_sequence(
        s.ed.project(),
        s.seq,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264),
        &ExportOptions {
            temp_dir: Some(work.path().to_path_buf()),
            ..Default::default()
        },
        |p| p.phase != Phase::Rendering,
    );
    assert!(matches!(res, Err(Error::Cancelled)));
    assert!(!out.exists());
    assert_eq!(std::fs::read_dir(work.path()).unwrap().count(), 0);
}

#[test]
fn missing_media_stops_before_rendering_unless_offline() {
    let mut s = show(small());
    let r = rgb_clip(&s, "r.nut", [220, 0, 0, 255]);
    let id = s.import(&r);
    s.lay(id, 0.0, 0.0, 1.0, true, false);
    std::fs::remove_file(&r).unwrap();
    let out = s.path("o.mp4");
    let res = export_sequence(
        s.ed.project(),
        s.seq,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264),
        &ExportOptions::default(),
        |_| true,
    );
    assert!(matches!(res, Err(Error::MissingMedia { .. })));
    assert!(!out.exists());
    let offline = ExportOptions {
        render: RenderSettings {
            missing_media: MissingMedia::Offline,
            ..Default::default()
        },
        ..Default::default()
    };
    export_sequence(
        s.ed.project(),
        s.seq,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264),
        &offline,
        |_| true,
    )
    .unwrap();
    let mut rd = FrameReader::open(&out, 1 << 24).unwrap();
    assert!(near(
        colour_of(&rd.frame_at(0.3).unwrap()),
        [255, 0, 255],
        40
    ));
}

#[test]
fn empty_sequence_and_bad_ranges_are_refused() {
    let s = show(small());
    let out = s.path("o.mp4");
    let st = ExportSettings::new(ExportPreset::Mp4H264);
    let res = export_sequence(
        s.ed.project(),
        s.seq,
        &out,
        &st,
        &ExportOptions::default(),
        |_| true,
    );
    assert!(matches!(res, Err(Error::Empty)));
    let res = export_sequence(
        s.ed.project(),
        SequenceId(999),
        &out,
        &st,
        &ExportOptions::default(),
        |_| true,
    );
    assert!(matches!(res, Err(Error::NoSuchSequence(_))));
}

#[test]
fn gif_preset_has_no_sound_and_audio_only_has_no_picture() {
    let mut s = show(small());
    let a = s.path("a.nut");
    make_clip(&a, (64, 36), (24, 1), 2, |n| [n as u8 * 4, 0, 0, 255], true);
    let id = s.import(&a);
    s.lay(id, 0.0, 0.0, 1.0, true, true);
    let gif = s.path("o.gif");
    export_sequence(
        s.ed.project(),
        s.seq,
        &gif,
        &ExportSettings::new(ExportPreset::Gif),
        &ExportOptions::default(),
        |_| true,
    )
    .unwrap();
    assert!(probe(&gif).unwrap().audio.is_empty());
    let mp3 = s.path("o.mp3");
    export_sequence(
        s.ed.project(),
        s.seq,
        &mp3,
        &ExportSettings::new(ExportPreset::Mp3),
        &ExportOptions::default(),
        |_| true,
    )
    .unwrap();
    let info = probe(&mp3).unwrap();
    assert!(info.video.is_empty() && !info.audio.is_empty());
    let _ = (
        LinkMode::Follow,
        ClipSpec::new(AssetId(0), Time::ZERO),
        VideoDecoder::open(&gif).map(|_| ()),
    );
}

#[test]
fn streaming_and_working_file_routes_give_the_same_result() {
    let mut s = show(small());
    let (r, g) = (
        rgb_clip(&s, "r.nut", [220, 0, 0, 255]),
        rgb_clip(&s, "g.nut", [0, 200, 0, 255]),
    );
    let (r, g) = (s.import(&r), s.import(&g));
    s.lay(r, 0.0, 0.0, 1.0, true, false);
    s.lay(g, 1.0, 0.0, 1.0, true, false);
    let (a, b) = (s.path("stream.mp4"), s.path("file.mp4"));
    let st = ExportSettings::new(ExportPreset::Mp4H264);
    export_sequence(
        s.ed.project(),
        s.seq,
        &a,
        &st,
        &ExportOptions::default(),
        |_| true,
    )
    .unwrap();
    let kept = export_sequence(
        s.ed.project(),
        s.seq,
        &b,
        &st,
        &ExportOptions {
            keep_working_file: true,
            temp_dir: Some(s.dir.path().to_path_buf()),
            ..Default::default()
        },
        |_| true,
    )
    .unwrap();
    let working = kept.working_file.expect("working file kept");
    assert!(working.exists());
    let (mut ra, mut rb) = (
        FrameReader::open(&a, 1 << 24).unwrap(),
        FrameReader::open(&b, 1 << 24).unwrap(),
    );
    for t in [0.2, 0.9, 1.2, 1.8] {
        let (fa, fb) = (ra.frame_at(t).unwrap(), rb.frame_at(t).unwrap());
        assert!(near(colour_of(&fa), colour_of(&fb), 3), "at {t}s");
    }
    for p in [&a, &b] {
        let info = probe(p).unwrap();
        let v = info.primary_video().unwrap();
        let fr = v.frame_rate.unwrap();
        assert_eq!((fr.num, fr.den), (24, 1), "{p:?}");
        assert!((info.duration_secs.unwrap() - 2.0).abs() < 0.05, "{p:?}");
    }
}

#[test]
fn a_png_still_is_held_for_any_length_and_fitted_to_the_frame() {
    // Needs the `ffmpeg` command line tool to make the picture; skipped without it.
    let mut s = show(small());
    let png = s.path("red.png");
    let made = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=40x40",
            "-frames:v",
            "1",
        ])
        .arg(&png)
        .status();
    if !matches!(made, Ok(st) if st.success()) {
        eprintln!("skipping: `ffmpeg` command line tool not found");
        return;
    }
    let id = s.import(&png);
    assert!(s.ed.project().asset(id).unwrap().is_still());
    s.lay(id, 0.0, 0.0, 5.0, true, false);
    let mut r = render_engine::Renderer::new(
        s.ed.project(),
        s.seq,
        FileSource::new(),
        RenderSettings::default(),
    )
    .unwrap();
    for frame in [0, 60, 119] {
        let f = r.frame(frame).unwrap();
        // 40x40 on a 64x36 frame: a 36x36 square in the middle with bars either side.
        assert!(near(colour_of(&f), [255, 0, 0], 2), "frame {frame}");
        let left = f.data[(18 * 64 + 5) * 4..(18 * 64 + 5) * 4 + 3].to_vec();
        assert_eq!(left, [0, 0, 0], "bar at frame {frame}");
    }
}

#[test]
fn a_fast_reversed_clip_exports_with_the_right_frames_and_length() {
    let mut s = show(small());
    let a = s.path("a.nut");
    // 4 s; frame n is red level 2n.
    make_clip(
        &a,
        (64, 36),
        (24, 1),
        4,
        |n| [(n * 2) as u8, 0, 0, 255],
        true,
    );
    let id = s.import(&a);
    s.lay(id, 0.0, 0.0, 3.5, true, true);
    let seq = s.seq;
    let v = s.ed.project().sequence(seq).unwrap().video_tracks()[0]
        .clips()
        .next()
        .unwrap()
        .id;
    s.ed.set_speed(
        seq,
        v,
        Speed::new(2, 1).unwrap(),
        SpeedMode::KeepMedia,
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    s.ed.set_reverse(seq, v, true, LinkMode::Follow).unwrap();

    // 3.5 s of media at 2x is 42 frames; frame k is source frame 82 - 2k.
    let mut r = Renderer::new(
        s.ed.project(),
        seq,
        FileSource::new(),
        RenderSettings::default(),
    )
    .unwrap();
    for k in [0i64, 1, 20, 41] {
        let want = (2 * (82 - 2 * k)) as u8;
        let got = colour_of(&r.frame(k).unwrap());
        assert_eq!(got, [want, 0, 0], "frame {k}");
    }

    let out = s.path("out.mp4");
    let report = export_sequence(
        s.ed.project(),
        seq,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264),
        &ExportOptions {
            temp_dir: Some(s.dir.path().to_path_buf()),
            ..Default::default()
        },
        |_| true,
    )
    .unwrap();
    assert_eq!(report.frames, 42);
    let info = probe(&out).unwrap();
    assert!((info.duration_secs.unwrap() - 1.75).abs() < 0.1);
}

#[test]
fn a_dissolve_between_real_files_blends_their_colours() {
    let mut s = show(small());
    let (red, blue) = (
        rgb_clip(&s, "r.nut", [200, 0, 0, 255]),
        rgb_clip(&s, "b.nut", [0, 0, 200, 255]),
    );
    let (red, blue) = (s.import(&red), s.import(&blue));
    s.lay(red, 0.0, 0.0, 1.0, true, false);
    s.lay(blue, 1.0, 0.5, 1.0, true, false);
    let seq = s.seq;
    let second = s.ed.project().sequence(seq).unwrap().video_tracks()[0]
        .clips()
        .nth(1)
        .unwrap()
        .id;
    s.ed.set_transition(
        seq,
        second,
        Some(Transition {
            kind: TransitionKind::Dissolve,
            duration: Time::from_secs_f64(0.5),
            alignment: Alignment::Center,
        }),
        LinkMode::Follow,
    )
    .unwrap();
    let mut r = Renderer::new(
        s.ed.project(),
        seq,
        FileSource::new(),
        RenderSettings::default(),
    )
    .unwrap();
    // Window is 0.75 s to 1.25 s (frames 18..30); frame 24 is the cut.
    assert_eq!(colour_of(&r.frame(17).unwrap()), [200, 0, 0]);
    let mid = colour_of(&r.frame(24).unwrap());
    assert!(
        mid[0] > 80 && mid[0] < 120 && mid[2] > 80 && mid[2] < 120,
        "{mid:?}"
    );
    assert_eq!(colour_of(&r.frame(30).unwrap()), [0, 0, 200]);
}
