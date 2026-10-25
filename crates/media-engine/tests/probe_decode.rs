mod common;

use common::*;
use media_engine::{probe, AudioDecoder, AudioOptions, Error, FrameReader, VideoDecoder};

#[test]
fn probe_reports_streams() {
    require_ffmpeg!();
    let dir = scratch("probe");
    let clip = pattern_clip(&dir, "p.mp4", "320x180", 25, 2);
    let info = probe(&clip).unwrap();

    assert_eq!(info.format_name.contains("mp4"), true);
    assert!((info.duration_secs.unwrap() - 2.0).abs() < 0.2);
    let v = info.primary_video().unwrap();
    assert_eq!((v.width, v.height), (320, 180));
    assert_eq!(v.codec, "h264");
    assert!((v.frame_rate.unwrap().as_f64() - 25.0).abs() < 0.01);
    let a = info.primary_audio().unwrap();
    assert_eq!(a.sample_rate, 48000);
    assert_eq!(a.channels, 2);
    assert!(info.size_bytes > 0);
}

#[test]
fn probe_missing_file_is_a_clean_error() {
    assert!(matches!(
        probe("/definitely/not/here.mp4"),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn probe_garbage_file_errors() {
    let dir = scratch("garbage");
    let p = dir.join("junk.mp4");
    std::fs::write(&p, b"this is not a video").unwrap();
    assert!(probe(&p).is_err());
}

#[test]
fn sequential_decode_returns_every_frame_in_order() {
    require_ffmpeg!();
    let clip = id_clip(&scratch("seq"));
    let mut dec = VideoDecoder::open(&clip).unwrap();
    let mut n = 0;
    let mut last_pts = -1.0;
    while let Some(f) = dec.next_frame().unwrap() {
        assert_eq!((f.width, f.height), (64, 64));
        assert_eq!(f.data.len(), 64 * 64 * 4);
        assert_eq!(frame_number(&f.data), n, "frame {n} decoded as wrong content");
        assert!(f.pts_secs > last_pts, "pts must increase");
        last_pts = f.pts_secs;
        n += 1;
    }
    assert_eq!(n, 60);
}

#[test]
fn seek_is_frame_accurate_everywhere() {
    require_ffmpeg!();
    let clip = id_clip(&scratch("seek"));
    let mut dec = VideoDecoder::open(&clip).unwrap();
    // Includes keyframe boundaries (0, 15, 30, 45), just before/after them,
    // backwards jumps, and the final frame.
    for &n in &[0i64, 1, 14, 15, 16, 29, 30, 44, 45, 58, 59, 20, 3, 59, 0, 33] {
        let t = n as f64 / 30.0 + 0.004; // a hair after the frame start
        let f = dec.frame_at(t).unwrap().expect("frame");
        assert_eq!(frame_number(&f.data), n, "seeking to frame {n}");
    }
}

#[test]
fn decoding_continues_after_a_seek() {
    require_ffmpeg!();
    let clip = id_clip(&scratch("cont"));
    let mut dec = VideoDecoder::open(&clip).unwrap();
    dec.seek(40.0 / 30.0 + 0.004).unwrap();
    for n in 40..60 {
        let f = dec.next_frame().unwrap().unwrap();
        assert_eq!(frame_number(&f.data), n);
    }
    assert!(dec.next_frame().unwrap().is_none());
}

#[test]
fn decoder_without_video_reports_no_stream() {
    require_ffmpeg!();
    let dir = scratch("novideo");
    let wav = dir.join("tone.wav");
    if !wav.exists() {
        let ok = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i",
                   "sine=frequency=440:duration=1", wav.to_str().unwrap()])
            .status().unwrap().success();
        assert!(ok);
    }
    assert!(matches!(VideoDecoder::open(&wav), Err(Error::NoStream("video"))));
    assert!(AudioDecoder::open(&wav, AudioOptions::default()).is_ok());
}

#[test]
fn frame_reader_random_access_and_cache() {
    require_ffmpeg!();
    let clip = id_clip(&scratch("reader"));
    let mut r = FrameReader::open(&clip, 64 * 1024 * 1024).unwrap();

    // Random access lands on the right frame.
    for &n in &[10u64, 50, 2, 31, 59, 0] {
        let f = r.frame(n).unwrap();
        assert_eq!(frame_number(&f.data), n as i64);
    }
    // Scrubbing back over frames we already passed is served from cache.
    let before = r.cache_stats();
    for n in (0..10u64).rev() {
        assert_eq!(frame_number(&r.frame(n).unwrap().data), n as i64);
    }
    let after = r.cache_stats();
    assert!(after.hits >= before.hits + 10, "expected cache hits: {before:?} -> {after:?}");

    // Past the end clamps to the last frame.
    assert_eq!(frame_number(&r.frame(10_000).unwrap().data), 59);
    // Time based access.
    assert_eq!(frame_number(&r.frame_at(1.0).unwrap().data), 30);
}

#[test]
fn frame_reader_respects_memory_budget() {
    require_ffmpeg!();
    let clip = id_clip(&scratch("budget"));
    let frame_bytes = 64 * 64 * 4;
    let mut r = FrameReader::open(&clip, frame_bytes * 5).unwrap();
    for n in 0..40u64 {
        r.frame(n).unwrap();
    }
    let s = r.cache_stats();
    assert!(s.entries <= 5, "{s:?}");
    assert!(s.bytes <= frame_bytes * 5, "{s:?}");
}

#[test]
fn audio_decodes_to_requested_format() {
    require_ffmpeg!();
    let clip = id_clip(&scratch("audio"));
    let mut dec = AudioDecoder::open(
        &clip,
        AudioOptions { sample_rate: Some(48000), channels: Some(2) },
    )
    .unwrap();
    let all = dec.read_all().unwrap();
    assert_eq!(all.sample_rate, 48000);
    assert_eq!(all.channels, 2);
    let secs = all.duration_secs();
    assert!((secs - 2.0).abs() < 0.15, "duration {secs}");
    // FFmpeg's sine source peaks near 0.125, so: clearly non-silent, within range.
    let peak = all.samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.05 && peak <= 1.05, "peak {peak}");
}

#[test]
fn audio_seek_skips_ahead() {
    require_ffmpeg!();
    let clip = id_clip(&scratch("aseek"));
    let mut dec = AudioDecoder::open(&clip, AudioOptions::default()).unwrap();
    dec.seek(1.5).unwrap();
    let rest = dec.read_all().unwrap();
    assert!((rest.pts_secs - 1.5).abs() < 0.05, "starts at {}", rest.pts_secs);
    assert!((rest.duration_secs() - 0.5).abs() < 0.15, "len {}", rest.duration_secs());
}
