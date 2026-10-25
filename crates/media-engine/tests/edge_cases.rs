mod common;

use common::*;
use media_engine::{export, probe, AudioDecoder, AudioOptions, ExportPreset, ExportSettings, FrameReader, Quality, VideoDecoder};
use std::process::Command;

fn ff(args: &[&str]) {
    let out = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn transport_stream_with_nonzero_start_time_is_rebased() {
    require_ffmpeg!();
    let dir = scratch("ts");
    let src = id_clip(&dir);
    let ts = dir.join("id.ts");
    if !ts.exists() {
        ff(&["-i", src.to_str().unwrap(), "-c", "copy", "-f", "mpegts", ts.to_str().unwrap()]);
    }

    let mut dec = VideoDecoder::open(&ts).unwrap();
    let first = dec.next_frame().unwrap().unwrap();
    assert!(first.pts_secs.abs() < 0.1, "first frame should be at ~0s, got {}", first.pts_secs);

    // Seeking by media time still finds the right frame despite the container offset.
    for &n in &[0i64, 17, 30, 59] {
        let f = dec.frame_at(n as f64 / 30.0 + 0.004).unwrap().unwrap();
        assert_eq!(frame_number(&f.data), n, "ts seek to frame {n}");
    }
}

#[test]
fn ten_bit_hevc_decodes_and_converts() {
    require_ffmpeg!();
    let dir = scratch("hevc10");
    let clip = dir.join("hevc10.mkv");
    if !clip.exists() {
        ff(&["-f", "lavfi", "-i", "testsrc2=size=256x144:rate=24:duration=1", "-c:v", "libx265", "-preset", "ultrafast",
             "-pix_fmt", "yuv420p10le", "-x265-params", "log-level=error", clip.to_str().unwrap()]);
    }
    let info = probe(&clip).unwrap();
    let v = info.primary_video().unwrap();
    assert_eq!(v.codec, "hevc");
    assert!(v.pixel_format.contains("10"), "{}", v.pixel_format);

    let mut n = 0;
    let mut d = VideoDecoder::open(&clip).unwrap();
    while let Some(f) = d.next_frame().unwrap() {
        assert_eq!((f.width, f.height), (256, 144));
        n += 1;
    }
    assert_eq!(n, 24);

    // And it can be exported to a normal 8-bit H.264 file.
    let out = dir.join("from10bit.mp4");
    export(&clip, &out, &ExportSettings::new(ExportPreset::Mp4H264).quality(Quality::Draft)).unwrap();
    assert_eq!(probe(&out).unwrap().primary_video().unwrap().pixel_format, "yuv420p");
}

#[test]
fn odd_sized_video_exports_with_even_dimensions() {
    require_ffmpeg!();
    let dir = scratch("odd");
    let clip = dir.join("odd.mp4");
    if !clip.exists() {
        ff(&["-f", "lavfi", "-i", "testsrc2=size=322x182:rate=25:duration=1", "-c:v", "libx264", "-pix_fmt", "yuv420p", clip.to_str().unwrap()]);
    }
    let src_info = probe(&clip).unwrap();
    assert_eq!((src_info.video[0].width, src_info.video[0].height), (322, 182));

    // A truly odd source size (321x181) can only be produced by a codec without chroma
    // subsampling constraints; export must still yield even output sizes.
    let odd = dir.join("odd321.mkv");
    if !odd.exists() {
        ff(&["-f", "lavfi", "-i", "testsrc2=size=321x181:rate=25:duration=1", "-c:v", "libx264", "-pix_fmt", "yuv444p", odd.to_str().unwrap()]);
    }
    let out = dir.join("odd-out.mp4");
    export(&odd, &out, &ExportSettings::new(ExportPreset::Mp4H264).quality(Quality::Draft)).unwrap();
    let v = probe(&out).unwrap().video[0].clone();
    assert_eq!(v.width % 2, 0);
    assert_eq!(v.height % 2, 0);
    assert!((i64::from(v.width) - 321).abs() <= 3 && (i64::from(v.height) - 181).abs() <= 3);
}

#[test]
fn mono_audio_stays_mono_and_resamples() {
    require_ffmpeg!();
    let dir = scratch("mono");
    let clip = dir.join("mono.wav");
    if !clip.exists() {
        ff(&["-f", "lavfi", "-i", "sine=frequency=500:sample_rate=22050:duration=1", "-ac", "1", clip.to_str().unwrap()]);
    }
    let mp3 = dir.join("mono.mp3");
    export(&clip, &mp3, &ExportSettings::new(ExportPreset::Mp3)).unwrap();
    let a = probe(&mp3).unwrap().audio[0].clone();
    assert_eq!(a.channels, 1);

    let mut dec = AudioDecoder::open(&clip, AudioOptions { sample_rate: Some(48000), channels: Some(2) }).unwrap();
    let all = dec.read_all().unwrap();
    assert_eq!((all.sample_rate, all.channels), (48000, 2));
    assert!((all.duration_secs() - 1.0).abs() < 0.1);
}

#[test]
fn truncated_file_never_panics() {
    require_ffmpeg!();
    let dir = scratch("trunc");
    let src = id_clip(&dir);
    let bytes = std::fs::read(&src).unwrap();
    let cut = dir.join("cut.mp4");
    // Keep half the file; for a non-fragmented MP4 the index (moov) may be missing.
    std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();

    // Any of these may fail, but they must fail with an Err, not a crash or hang.
    let _ = probe(&cut);
    if let Ok(mut d) = VideoDecoder::open(&cut) {
        let mut n = 0;
        while let Ok(Some(_)) = d.next_frame() {
            n += 1;
            if n > 1000 {
                panic!("runaway decode");
            }
        }
    }
    let _ = export(&cut, dir.join("cut-out.mp4"), &ExportSettings::new(ExportPreset::Mp4H264));
}

#[test]
fn frame_reader_handles_single_frame_and_short_clips() {
    require_ffmpeg!();
    let dir = scratch("short");
    let clip = dir.join("one.mp4");
    if !clip.exists() {
        ff(&["-f", "lavfi", "-i", "testsrc2=size=64x64:rate=30", "-frames:v", "1", "-c:v", "libx264", "-pix_fmt", "yuv420p", clip.to_str().unwrap()]);
    }
    let mut r = FrameReader::open(&clip, 1 << 20).unwrap();
    let a = r.frame(0).unwrap();
    let b = r.frame(5).unwrap(); // clamps to the only frame
    assert_eq!(a.data, b.data);
    assert!(r.frame_at(100.0).is_ok());
}

#[test]
fn mkv_and_webm_inputs_decode() {
    require_ffmpeg!();
    let dir = scratch("containers");
    let src = pattern_clip(&dir, "src.mp4", "128x72", 25, 1);
    for (name, preset) in [("c.mkv", ExportPreset::MkvH264), ("c.webm", ExportPreset::WebmVp9), ("c.mov", ExportPreset::MovH264)] {
        let p = dir.join(name);
        export(&src, &p, &ExportSettings::new(preset).quality(Quality::Draft)).unwrap();
        let mut d = VideoDecoder::open(&p).unwrap();
        let mut n = 0;
        while d.next_frame().unwrap().is_some() {
            n += 1;
        }
        assert_eq!(n, 25, "{name}");
        let mut a = AudioDecoder::open(&p, AudioOptions::default()).unwrap();
        assert!(a.read_all().unwrap().duration_secs() > 0.8, "{name} audio");
    }
}
