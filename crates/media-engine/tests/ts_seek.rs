//! Seeking in MPEG-TS clips that use B-frames.

mod common;
use common::*;
use media_engine::VideoDecoder;

fn bframe_ts(name: &str, gop: &str, extra: &str) -> std::path::PathBuf {
    let dir = scratch(name);
    let ts = dir.join("bf.ts");
    if !ts.exists() {
        run(&[
            "-f",
            "lavfi",
            "-i",
            "nullsrc=size=64x64:rate=30:duration=2,geq=r='255-N':g='N*4':b='128'",
            "-c:v",
            "libx264",
            "-crf",
            "6",
            "-g",
            gop,
            "-bf",
            "3",
            "-x264-params",
            extra,
            "-pix_fmt",
            "yuv420p",
            "-f",
            "mpegts",
            ts.to_str().unwrap(),
        ]);
    }
    ts
}

#[test]
fn ts_with_b_frames_seeks_to_the_right_frame_near_the_start() {
    require_ffmpeg!();
    let variants = [
        ("a", "30", "log-level=error"),
        ("b", "15", "log-level=error"),
        ("c", "15", "open-gop=1:b-pyramid=normal:log-level=error"),
        (
            "d",
            "60",
            "open-gop=1:bframes=5:b-pyramid=strict:log-level=error",
        ),
        ("e", "8", "bframes=4:b-adapt=0:log-level=error"),
    ];
    let mut bad = Vec::new();
    let mut clips: Vec<(String, String, std::path::PathBuf)> = Vec::new();
    for (tag, gop, extra) in variants {
        clips.push((
            tag.into(),
            gop.into(),
            bframe_ts(&format!("ts-bf-{tag}"), gop, extra),
        ));
    }
    // MP4 stream-copied into TS: carries the MP4 start-time offset caused by B-frame delay.
    for (tag, gop) in [("m15", "15"), ("m30", "30")] {
        let dir = scratch(&format!("ts-bf-{tag}"));
        let mp4 = dir.join("bf.mp4");
        let ts = dir.join("bf-copy.ts");
        if !ts.exists() {
            run(&[
                "-f",
                "lavfi",
                "-i",
                "nullsrc=size=64x64:rate=30:duration=2,geq=r='255-N':g='N*4':b='128'",
                "-c:v",
                "libx264",
                "-crf",
                "6",
                "-g",
                gop,
                "-bf",
                "3",
                "-pix_fmt",
                "yuv420p",
                mp4.to_str().unwrap(),
            ]);
            run(&[
                "-i",
                mp4.to_str().unwrap(),
                "-c",
                "copy",
                "-f",
                "mpegts",
                ts.to_str().unwrap(),
            ]);
        }
        clips.push((tag.into(), gop.into(), ts));
    }
    // With an audio track too (the demuxer's default stream may then be audio).
    for (tag, gop) in [("a15", "15"), ("a30", "30")] {
        let dir = scratch(&format!("ts-bf-{tag}"));
        let ts = dir.join("bf-audio.ts");
        if !ts.exists() {
            run(&[
                "-f",
                "lavfi",
                "-i",
                "nullsrc=size=256x256:rate=30:duration=2,geq=r='255-N':g='N*4':b='128'",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=44100:duration=2",
                "-c:v",
                "libx264",
                "-crf",
                "6",
                "-g",
                gop,
                "-bf",
                "3",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
                "-f",
                "mpegts",
                ts.to_str().unwrap(),
            ]);
        }
        clips.push((tag.into(), gop.into(), ts));
    }
    for (tag, gop, ts) in clips {
        let mut dec = VideoDecoder::open(&ts).unwrap();
        for &n in &[0i64, 1, 2, 5, 14, 15, 29, 30, 31, 59, 3, 0] {
            match dec.frame_at(n as f64 / 30.0 + 0.004).unwrap() {
                Some(f) => {
                    let got = frame_number(&f.data);
                    if got != n {
                        bad.push(format!("variant {tag} (gop {gop}): seek to {n} gave {got}"));
                    }
                }
                None => bad.push(format!(
                    "variant {tag} (gop {gop}): seek to {n} gave no frame"
                )),
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
