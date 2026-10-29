// SPDX-License-Identifier: GPL-3.0-or-later
//! The lossless working file must give back exactly what was written.
use media_engine::*;

fn pattern(w: u32, h: u32, n: u32) -> VideoFrame {
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            data.extend_from_slice(&[
                (x * 7 + n * 13) as u8,
                (y * 11 + n * 3) as u8,
                ((x ^ y) + n) as u8,
                255,
            ]);
        }
    }
    VideoFrame {
        width: w,
        height: h,
        pts_secs: 0.0,
        data,
    }
}

#[test]
fn frames_and_sound_round_trip_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w.nut");
    let spec = WorkingSpec {
        width: 64,
        height: 36,
        fps_num: 24000,
        fps_den: 1001,
        audio: Some(WorkingAudio {
            sample_rate: 48000,
            channels: 2,
        }),
    };
    let mut w = WorkingFileWriter::create(&path, spec).unwrap();
    for n in 0..24 {
        w.write_video(&pattern(64, 36, n)).unwrap();
    }
    let audio: Vec<f32> = (0..48000 * 2)
        .map(|i| ((i as f32) * 0.001).sin() * 0.5)
        .collect();
    // Written in odd-sized pieces on purpose.
    for piece in audio.chunks(7000) {
        w.write_audio(piece).unwrap();
    }
    assert_eq!((w.frames_written(), w.samples_written()), (24, 48000));
    w.finish().unwrap();

    let info = probe(&path).unwrap();
    let v = info.primary_video().unwrap();
    assert_eq!((v.width, v.height), (64, 36));
    let fr = v.frame_rate.unwrap();
    assert_eq!((fr.num, fr.den), (24000, 1001));
    assert_eq!(info.primary_audio().unwrap().sample_rate, 48000);

    let mut dec = VideoDecoder::open(&path).unwrap();
    for n in 0..24 {
        let got = dec.next_frame().unwrap().expect("frame");
        assert_eq!(got.data, pattern(64, 36, n).data, "frame {n} differs");
    }
    assert!(dec.next_frame().unwrap().is_none());

    let mut ad = AudioDecoder::open(&path, AudioOptions::default()).unwrap();
    let back = ad.read_all().unwrap();
    assert_eq!(back.samples, audio);
}

#[test]
fn working_file_exports_with_a_preset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w.nut");
    let out = dir.path().join("o.mp4");
    let spec = WorkingSpec {
        width: 128,
        height: 72,
        fps_num: 25,
        fps_den: 1,
        audio: None,
    };
    let mut w = WorkingFileWriter::create(&path, spec).unwrap();
    for n in 0..25 {
        w.write_video(&pattern(128, 72, n)).unwrap();
    }
    assert!(w.write_audio(&[0.0, 0.0]).is_err());
    w.finish().unwrap();
    export(&path, &out, &ExportSettings::new(ExportPreset::Mp4H264)).unwrap();
    let info = probe(&out).unwrap();
    assert!((info.duration_secs.unwrap() - 1.0).abs() < 0.1);
}

#[test]
fn rejects_wrong_sized_frames_and_bad_specs() {
    let dir = tempfile::tempdir().unwrap();
    let spec = WorkingSpec {
        width: 16,
        height: 16,
        fps_num: 24,
        fps_den: 1,
        audio: None,
    };
    let mut w = WorkingFileWriter::create(dir.path().join("a.nut"), spec).unwrap();
    assert!(w.write_video(&pattern(8, 8, 0)).is_err());
    let bad = WorkingSpec { width: 0, ..spec };
    assert!(WorkingFileWriter::create(dir.path().join("b.nut"), bad).is_err());
}

#[test]
fn audio_seeks_land_on_the_exact_sample() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w.nut");
    let spec = WorkingSpec {
        width: 16,
        height: 16,
        fps_num: 24,
        fps_den: 1,
        audio: Some(WorkingAudio {
            sample_rate: 48000,
            channels: 2,
        }),
    };
    let mut w = WorkingFileWriter::create(&path, spec).unwrap();
    for _ in 0..72 {
        w.write_video(&pattern(16, 16, 0)).unwrap();
    }
    let ramp: Vec<f32> = (0..144_000usize)
        .flat_map(|i| [(i % 10000) as f32 / 10000.0; 2])
        .collect();
    w.write_audio(&ramp).unwrap();
    w.finish().unwrap();
    let mut dec = AudioDecoder::open(
        &path,
        AudioOptions {
            sample_rate: Some(48000),
            channels: Some(2),
        },
    )
    .unwrap();
    for target in [
        143_000usize,
        140_000,
        100_000,
        10_000,
        77_777,
        1,
        0,
        143_999,
    ] {
        dec.seek(target as f64 / 48000.0).unwrap();
        let b = dec.next_buffer().unwrap().unwrap();
        assert_eq!(
            b.samples[0],
            (target % 10000) as f32 / 10000.0,
            "seek to sample {target}"
        );
    }
}

#[cfg(unix)]
mod pipe {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn spec(audio: bool) -> WorkingSpec {
        WorkingSpec {
            width: 96,
            height: 54,
            fps_num: 25,
            fps_den: 1,
            audio: audio.then_some(WorkingAudio {
                sample_rate: 48000,
                channels: 2,
            }),
        }
    }

    #[test]
    fn frames_flow_straight_into_an_export() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("o.mp4");
        let (mut w, pipe) = WorkingFileWriter::create_pipe(spec(true)).unwrap();
        let out2 = out.clone();
        let exporter = std::thread::spawn(move || {
            export_pipe_with_progress(
                pipe,
                &out2,
                &ExportSettings::new(ExportPreset::Mp4H264),
                Some(2.0),
                |_| true,
            )
        });
        for n in 0..50 {
            w.write_video(&pattern(96, 54, n)).unwrap();
            w.write_audio(&vec![0.1f32; 1920 * 2]).unwrap();
        }
        w.finish().unwrap();
        let report = exporter.join().unwrap().unwrap();
        assert_eq!(report.frames, 50);
        let info = probe(&out).unwrap();
        let fr = info.primary_video().unwrap().frame_rate.unwrap();
        assert_eq!((fr.num, fr.den), (25, 1));
        assert!((info.duration_secs.unwrap() - 2.0).abs() < 0.1);
        assert!(info.primary_audio().is_some());
    }

    #[test]
    fn a_failing_reader_makes_the_writer_fail_not_hang() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut w, pipe) = WorkingFileWriter::create_pipe(spec(false)).unwrap();
            // Output folder does not exist, so the export fails at once.
            let bad = dir.path().join("no/such/dir/o.mp4");
            let exporter = std::thread::spawn(move || {
                export_pipe_with_progress(
                    pipe,
                    bad,
                    &ExportSettings::new(ExportPreset::Mp4H264),
                    None,
                    |_| true,
                )
            });
            let mut failed = false;
            for n in 0..200 {
                if w.write_video(&pattern(96, 54, n)).is_err() {
                    failed = true;
                    break;
                }
            }
            let _ = tx.send((failed, exporter.join().unwrap().is_err()));
        });
        let (writer_failed, export_failed) = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("writer hung");
        assert!(writer_failed && export_failed);
    }

    #[test]
    fn a_writer_that_stops_early_ends_the_export() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("o.mp4");
        let (mut w, pipe) = WorkingFileWriter::create_pipe(spec(false)).unwrap();
        let out2 = out.clone();
        let exporter = std::thread::spawn(move || {
            export_pipe_with_progress(
                pipe,
                &out2,
                &ExportSettings::new(ExportPreset::Mp4H264),
                None,
                |_| true,
            )
        });
        for n in 0..5 {
            w.write_video(&pattern(96, 54, n)).unwrap();
        }
        drop(w); // never finished
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(exporter.join().is_ok());
        });
        assert!(rx
            .recv_timeout(Duration::from_secs(30))
            .expect("export hung"));
    }
}
