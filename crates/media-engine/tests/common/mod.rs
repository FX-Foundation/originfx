//! Test helpers: generate small clips with the `ffmpeg` command line tool.
//!
//! Tests skip themselves (with a message) when `ffmpeg` is not installed, so
//! `cargo test` never fails just because the helper tool is missing.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Skip the current test unless `ffmpeg` is installed.
#[macro_export]
macro_rules! require_ffmpeg {
    () => {
        if !$crate::common::ffmpeg_available() {
            eprintln!("skipping: `ffmpeg` command line tool not found");
            return;
        }
    };
}

fn run(args: &[&str]) {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .output()
        .expect("failed to run ffmpeg");
    assert!(
        out.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// 64x64, 30 fps, 60 frames. Frame `n` is solid colour with green = `n * 4`,
/// so a decoded frame tells you its own number. Lossless, keyframe every 15.
/// Has a 44.1 kHz mono 440 Hz sine audio track of the same length.
pub fn id_clip(dir: &Path) -> PathBuf {
    let out = dir.join("idclip.mp4");
    if !out.exists() {
        run(&[
            "-f",
            "lavfi",
            "-i",
            "nullsrc=size=64x64:rate=30:duration=2,geq=r='255-N':g='N*4':b='128'",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=44100:duration=2",
            "-c:v",
            "libx264",
            "-qp",
            "0",
            "-g",
            "15",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
            out.to_str().unwrap(),
        ]);
    }
    out
}

/// Colourful moving test pattern with a stereo 48 kHz tone, `secs` seconds long.
pub fn pattern_clip(dir: &Path, name: &str, size: &str, fps: u32, secs: u32) -> PathBuf {
    let out = dir.join(name);
    if !out.exists() {
        let video = format!("testsrc2=size={size}:rate={fps}:duration={secs}");
        let audio = format!("sine=frequency=330:sample_rate=48000:duration={secs}");
        run(&[
            "-f",
            "lavfi",
            "-i",
            &video,
            "-f",
            "lavfi",
            "-i",
            &audio,
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-ac",
            "2",
            "-c:a",
            "aac",
            "-shortest",
            out.to_str().unwrap(),
        ]);
    }
    out
}

/// Scratch directory unique to the calling test file.
pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("originfx-tests-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Which frame number does this RGBA frame claim to be? (See [`id_clip`].)
pub fn frame_number(rgba: &[u8]) -> i64 {
    // The decoder's YUV->RGB conversion lands ~2-3 levels under the nominal
    // value near black, so bias before flooring. Neighbouring frames are 4 apart.
    let g = i64::from(rgba[1]);
    (g + 3) / 4
}
