// SPDX-License-Identifier: GPL-3.0-or-later
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

pub fn run(args: &[&str]) {
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

/// Variable-frame-rate clip: the same numbered frames as [`id_clip`], but only
/// frames 0-4 of every ten are kept, each at its original time (`n / 30` s).
/// So the clip has 30 frames, gaps of 6 frame-times, and no audio.
pub fn vfr_clip(dir: &Path) -> PathBuf {
    let out = dir.join("vfr.mp4");
    if !out.exists() {
        run(&[
            "-f",
            "lavfi",
            "-i",
            "nullsrc=size=64x64:rate=30:duration=2,geq=r='255-N':g='N*4':b='128',select='lt(mod(n\\,10)\\,5)'",
            "-fps_mode",
            "vfr",
            "-c:v",
            "libx264",
            "-qp",
            "0",
            "-g",
            "15",
            "-pix_fmt",
            "yuv420p",
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

/// One second of a flat colour `(208, 32, 80)`, encoded losslessly with the given
/// YUV `matrix` (`bt709` or `bt601`). `tags` are the ffmpeg flags that label the file
/// (empty = untagged).
pub fn colour_clip(dir: &Path, name: &str, size: &str, matrix: &str, tags: &[&str]) -> PathBuf {
    let out = dir.join(name);
    if !out.exists() {
        let src = format!("color=c=0xD02050:size={size}:rate=10:duration=1");
        let vf = format!("scale=out_color_matrix={matrix}:out_range=tv,format=yuv420p");
        let mut args = vec!["-f", "lavfi", "-i", &src, "-vf", &vf];
        args.extend_from_slice(tags);
        args.extend_from_slice(&["-c:v", "libx264", "-qp", "0"]);
        args.push(out.to_str().unwrap());
        run(&args);
    }
    out
}

pub const COLOUR_EXPECTED: [i32; 3] = [0xD0, 0x20, 0x50];

pub const TAGS_709: &[&str] = &[
    "-colorspace",
    "bt709",
    "-color_primaries",
    "bt709",
    "-color_trc",
    "bt709",
    "-color_range",
    "tv",
];
pub const TAGS_601: &[&str] = &[
    "-colorspace",
    "smpte170m",
    "-color_primaries",
    "smpte170m",
    "-color_trc",
    "smpte170m",
    "-color_range",
    "tv",
];

/// Largest per-channel difference between a decoded RGBA frame's centre pixel
/// and [`COLOUR_EXPECTED`].
pub fn colour_error(frame: &media_engine::VideoFrame) -> i32 {
    let i = ((frame.height / 2 * frame.width + frame.width / 2) * 4) as usize;
    (0..3)
        .map(|c| (i32::from(frame.data[i + c]) - COLOUR_EXPECTED[c]).abs())
        .max()
        .unwrap()
}

/// GPU APIs that can open a device on this machine (empty on CI).
/// Opens only CUDA and VA-API devices: probing every FFmpeg device type
/// (VDPAU, QSV, ...) from parallel test threads can crash some drivers.
pub fn gpu_apis() -> Vec<media_engine::HwApi> {
    static APIS: std::sync::OnceLock<Vec<media_engine::HwApi>> = std::sync::OnceLock::new();
    APIS.get_or_init(|| {
        media_engine::HwApi::ALL
            .into_iter()
            .filter(|api| api.is_usable())
            .collect()
    })
    .clone()
}

/// Skip the current test unless `ffmpeg` is installed and a GPU decoder can be opened.
#[macro_export]
macro_rules! require_gpu {
    () => {{
        $crate::require_ffmpeg!();
        let apis = $crate::common::gpu_apis();
        if apis.is_empty() {
            eprintln!("skipping: no usable GPU (cuda / vaapi) on this machine");
            return;
        }
        apis
    }};
}

/// The numbered frames of [`id_clip`] at 256x256 (above every GPU's minimum
/// size), encoded with `codec_args` in a form GPUs decode: lossy 4:2:0 at very
/// high quality. `id_clip` itself is lossless High 4:4:4, which GPUs reject.
pub fn gpu_id_clip(dir: &Path, name: &str, codec_args: &[&str]) -> PathBuf {
    let out = dir.join(name);
    if !out.exists() {
        let mut args = vec![
            "-f",
            "lavfi",
            "-i",
            "nullsrc=size=256x256:rate=30:duration=2,geq=r='255-N':g='N*4':b='128'",
        ];
        args.extend_from_slice(codec_args);
        args.push(out.to_str().unwrap());
        run(&args);
    }
    out
}

/// Which frame of `reference` (software-decoded, in order) is `frame`, by exact
/// pixel match; -1 when it matches none. Lossy clips do not carry an exact green
/// value, so this both numbers a GPU frame and proves it equals software.
pub fn frame_id(reference: &[media_engine::VideoFrame], frame: &media_engine::VideoFrame) -> i64 {
    reference
        .iter()
        .position(|r| r.data == frame.data)
        .map_or(-1, |i| i as i64)
}

/// True when NVENC can be used here: a CUDA device opens and FFmpeg has `h264_nvenc`.
pub fn nvenc_available() -> bool {
    gpu_apis().contains(&media_engine::HwApi::Cuda)
        && media_engine::hwaccel::hardware_codecs()
            .iter()
            .any(|c| c == "h264_nvenc")
}

/// Skip the current test unless `ffmpeg` is installed and NVENC can be used.
#[macro_export]
macro_rules! require_nvenc {
    () => {
        $crate::require_ffmpeg!();
        if !$crate::common::nvenc_available() {
            eprintln!("skipping: NVENC (CUDA) is not available on this machine");
            return;
        }
    };
}
