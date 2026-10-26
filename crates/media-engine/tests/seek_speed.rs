//! Seeking in long-GOP footage with B-frames: fast, and always the right frame.

mod common;

use common::*;
use media_engine::{FrameReader, VideoDecoder};
use std::path::PathBuf;

const FRAMES: i64 = 150;

/// 5 s, 30 fps, one keyframe every 100 frames, B-frames with a pyramid, so seeks
/// have to run a long way from the keyframe and some B-frames are referenced.
/// Frame `n` is painted with r = (n % 32) * 8 and g = (n / 32) * 8.
fn long_gop_clip(name: &str, bframes: &str) -> PathBuf {
    let dir = scratch(name);
    let out = dir.join("long-gop.mp4");
    if !out.exists() {
        run(&[
            "-f",
            "lavfi",
            "-i",
            "nullsrc=size=64x64:rate=30:duration=5,geq=r='mod(N,32)*8':g='floor(N/32)*8':b='128'",
            "-c:v",
            "libx264",
            // Not lossless (-qp 0): x264 then writes no B-frames at all.
            "-crf",
            "6",
            "-g",
            "100",
            "-bf",
            bframes,
            "-x264-params",
            "b-adapt=0:b-pyramid=normal:keyint=100:min-keyint=100:scenecut=0",
            "-pix_fmt",
            "yuv420p",
            out.to_str().unwrap(),
        ]);
    }
    out
}

fn id_of(rgba: &[u8]) -> i64 {
    let r = i64::from(rgba[0]);
    let g = i64::from(rgba[1]);
    ((r + 4) / 8) % 32 + 32 * ((g + 4) / 8)
}

/// A fixed pseudo-random order, so failures repeat.
fn shuffled_targets() -> Vec<i64> {
    let mut x: u64 = 12345;
    (0..60)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((x >> 33) % FRAMES as u64) as i64
        })
        .collect()
}

#[test]
fn random_seeks_in_long_gop_footage_are_frame_exact() {
    require_ffmpeg!();
    for bf in ["0", "2", "3"] {
        let clip = long_gop_clip(&format!("seek-speed-bf{bf}"), bf);
        let mut dec = VideoDecoder::open(&clip).unwrap();
        for n in shuffled_targets() {
            let f = dec.frame_at(n as f64 / 30.0 + 0.004).unwrap().unwrap();
            assert_eq!(id_of(&f.data), n, "bframes={bf}: seek to frame {n}");
        }
        // After a seek, plain decoding continues in order with nothing skipped.
        dec.seek(40.0 / 30.0 + 0.004).unwrap();
        for n in 40..FRAMES {
            let f = dec.next_frame().unwrap().unwrap();
            assert_eq!(id_of(&f.data), n, "bframes={bf}: after seek, frame {n}");
        }
        assert!(dec.next_frame().unwrap().is_none());
    }
}

#[test]
fn scrubbing_backwards_is_exact_and_mostly_cache_hits() {
    require_ffmpeg!();
    let clip = long_gop_clip("seek-speed-scrub", "3");
    // Room for plenty of 64x64 frames, so the backward prefetch is at full depth.
    let mut reader = FrameReader::open(&clip, 64 * 1024 * 1024).unwrap();
    let top = 140i64;
    assert_eq!(id_of(&reader.frame(top as u64).unwrap().data), top);
    let before = reader.cache_stats();
    for n in (60..top).rev() {
        let f = reader.frame(n as u64).unwrap();
        assert_eq!(id_of(&f.data), n, "stepping back to {n}");
    }
    let after = reader.cache_stats();
    let steps = (top - 60) as u64;
    let hits = after.hits - before.hits;
    // Without the backward prefetch every step is a miss; with it, only about
    // one step in sixteen is.
    assert!(
        hits >= steps * 3 / 4,
        "only {hits} of {steps} backward steps were cache hits"
    );

    // Jumping around afterwards is still exact.
    for n in shuffled_targets() {
        let f = reader.frame(n as u64).unwrap();
        assert_eq!(id_of(&f.data), n, "jump to {n}");
    }
}
