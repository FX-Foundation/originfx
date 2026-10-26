//! GPU encoding (NVENC) for the H.264 and H.265 export presets, and the
//! fallback to software. Tests that need NVENC skip themselves without it; the
//! fallback tests run everywhere.

mod common;

use common::*;
use media_engine::{
    export, export_with_progress, probe, Error, ExportPreset, ExportSettings, HwApi, HwPolicy,
    Quality, VideoDecoder, VideoFrame,
};

const NVENC: HwPolicy = HwPolicy::Require(HwApi::Cuda);

/// 256x256 numbered frames (see `gpu_id_clip`), 60 frames at 30 fps.
fn numbered_clip(dir: &std::path::Path) -> std::path::PathBuf {
    gpu_id_clip(
        dir,
        "enc-src.mp4",
        &[
            "-c:v", "libx264", "-qp", "1", "-g", "15", "-pix_fmt", "yuv420p",
        ],
    )
}

fn frames(path: &std::path::Path) -> Vec<VideoFrame> {
    VideoDecoder::open(path)
        .unwrap()
        .collect::<media_engine::Result<Vec<_>>>()
        .unwrap()
}

/// Green at the centre of a frame: the numbered clip's frame id is green / 4.
fn centre_green(f: &VideoFrame) -> i32 {
    let i = ((f.height / 2 * f.width + f.width / 2) * 4 + 1) as usize;
    i32::from(f.data[i])
}

/// Every frame of `out` shows the same picture as `src[first + n]`.
fn assert_same_frames(src: &[VideoFrame], first: usize, out: &[VideoFrame], what: &str) {
    for (n, f) in out.iter().enumerate() {
        let want = centre_green(&src[first + n]);
        let got = centre_green(f);
        // Frames are 4 green levels apart, so within 2 still identifies the frame.
        assert!(
            (got - want).abs() <= 2,
            "{what}: output frame {n} has green {got}, source frame {} has {want}",
            first + n
        );
    }
}

#[test]
fn h264_and_h265_presets_encode_on_nvenc() {
    require_nvenc!();
    let dir = scratch("nvenc-presets");
    let src = numbered_clip(&dir);
    let reference = frames(&src);
    for (preset, encoder, codec) in [
        (ExportPreset::Mp4H264, "h264_nvenc", "h264"),
        (ExportPreset::MovH264, "h264_nvenc", "h264"),
        (ExportPreset::MkvH264, "h264_nvenc", "h264"),
        (ExportPreset::Mp4H265, "hevc_nvenc", "hevc"),
        (ExportPreset::MkvH265, "hevc_nvenc", "hevc"),
    ] {
        let out = dir.join(format!("out.{}", preset.extension()));
        let settings = ExportSettings::new(preset)
            .quality(Quality::High)
            .hw_encode(NVENC);
        let report = export(&src, &out, &settings).unwrap();
        assert_eq!(report.video_encoder.as_deref(), Some(encoder), "{preset:?}");
        assert_eq!(report.hw_fallback_reason, None);
        assert_eq!(report.frames, 60);
        let v = probe(&out).unwrap().video[0].clone();
        assert_eq!(v.codec, codec, "{preset:?}");
        assert_eq!((v.width, v.height), (256, 256));
        let decoded = frames(&out);
        assert_eq!(decoded.len(), 60, "{preset:?}");
        assert_same_frames(&reference, 0, &decoded, preset.id());
    }
}

#[test]
fn nvenc_trims_scales_and_sets_keyframes() {
    require_nvenc!();
    let dir = scratch("nvenc-trim");
    let src = numbered_clip(&dir);
    let reference = frames(&src);
    let out = dir.join("trim.mp4");
    let settings = ExportSettings::new(ExportPreset::Mp4H264)
        .quality(Quality::High)
        .trim(Some(0.5), Some(1.5))
        .height(192)
        .keyint(10)
        .hw_encode(NVENC);
    let report = export(&src, &out, &settings).unwrap();
    assert_eq!(report.video_encoder.as_deref(), Some("h264_nvenc"));
    assert_eq!(report.frames, 30, "1 s at 30 fps");
    let decoded = frames(&out);
    assert_eq!(decoded.len(), 30);
    assert_eq!((decoded[0].width, decoded[0].height), (192, 192));
    assert_same_frames(&reference, 15, &decoded, "trimmed");

    // A keyframe every 10 frames: 0, 10 and 20.
    let probe_out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v",
            "-show_entries",
            "packet=flags",
        ])
        .args(["-of", "csv=p=0", out.to_str().unwrap()])
        .output()
        .expect("ffprobe");
    let flags = String::from_utf8_lossy(&probe_out.stdout);
    let keyframes = flags.lines().filter(|l| l.starts_with('K')).count();
    assert_eq!(keyframes, 3, "packet flags:\n{flags}");
}

#[test]
fn ten_bit_hevc_stays_ten_bit_on_nvenc() {
    require_nvenc!();
    let dir = scratch("nvenc-10bit");
    let src = gpu_id_clip(
        &dir,
        "src10.mkv",
        &[
            "-c:v",
            "libx265",
            "-x265-params",
            "qp=4:log-level=error",
            "-pix_fmt",
            "yuv420p10le",
        ],
    );
    let out = dir.join("out10.mp4");
    let settings = ExportSettings::new(ExportPreset::Mp4H265).hw_encode(NVENC);
    let report = export(&src, &out, &settings).unwrap();
    assert_eq!(report.video_encoder.as_deref(), Some("hevc_nvenc"));
    let v = probe(&out).unwrap().video[0].clone();
    assert_eq!(v.pixel_format, "yuv420p10le");
    assert_same_frames(&frames(&src), 0, &frames(&out), "10-bit");
}

#[test]
fn off_is_software_and_auto_falls_back_where_nvenc_cannot_go() {
    require_ffmpeg!();
    let dir = scratch("nvenc-fallback");
    let src = numbered_clip(&dir);
    let out = dir.join("plain.mp4");

    // Off: exactly as before.
    let r = export(&src, &out, &ExportSettings::new(ExportPreset::Mp4H264)).unwrap();
    assert_eq!(r.video_encoder.as_deref(), Some("libx264"));
    assert_eq!(r.hw_fallback_reason, None);

    // A preset NVENC does not cover.
    let webm = dir.join("auto.webm");
    let s = ExportSettings::new(ExportPreset::WebmVp9)
        .quality(Quality::Draft)
        .hw_encode(HwPolicy::Auto);
    let r = export(&src, &webm, &s).unwrap();
    assert_eq!(r.video_encoder.as_deref(), Some("libvpx-vp9"));
    assert!(r.hw_fallback_reason.unwrap().contains("H.264 and H.265"));

    // 10-bit H.264: NVENC cannot.
    let s = ExportSettings::new(ExportPreset::Mp4H264)
        .bit_depth(10)
        .hw_encode(HwPolicy::Auto);
    let r = export(&src, &out, &s).unwrap();
    assert_eq!(r.video_encoder.as_deref(), Some("libx264"));
    assert!(r.hw_fallback_reason.is_some());

    // Required GPU encoding that cannot happen is an error, and writes nothing.
    let missing = dir.join("never.webm");
    let s = ExportSettings::new(ExportPreset::WebmVp9).hw_encode(NVENC);
    assert!(matches!(
        export(&src, &missing, &s),
        Err(Error::Unsupported(_))
    ));
    let s = ExportSettings::new(ExportPreset::Mp4H264).hw_encode(HwPolicy::Require(HwApi::Vaapi));
    assert!(matches!(
        export(&src, &missing, &s),
        Err(Error::Unsupported(_))
    ));
    assert!(!missing.exists());
}

#[test]
fn nvenc_failure_part_way_restarts_in_software() {
    require_nvenc!();
    let dir = scratch("nvenc-fail");
    let src = numbered_clip(&dir);
    let reference = frames(&src);
    let out = dir.join("retry.mp4");
    let mut settings = ExportSettings::new(ExportPreset::Mp4H264)
        .quality(Quality::High)
        .hw_encode(HwPolicy::Auto);
    settings.simulate_gpu_failure_after = Some(20);
    let r = export(&src, &out, &settings).unwrap();
    assert_eq!(r.video_encoder.as_deref(), Some("libx264"));
    assert!(r.hw_fallback_reason.unwrap().contains("simulated"));
    assert_eq!(r.frames, 60);
    let decoded = frames(&out);
    assert_eq!(decoded.len(), 60);
    assert_same_frames(&reference, 0, &decoded, "after retry");

    // NVENC refuses frames below its minimum size: a real failure when opening.
    let tiny = dir.join("tiny.mp4");
    let s = ExportSettings::new(ExportPreset::Mp4H264)
        .height(64)
        .hw_encode(HwPolicy::Auto);
    let r = export(&src, &tiny, &s).unwrap();
    assert_eq!(r.video_encoder.as_deref(), Some("libx264"));
    assert!(r.hw_fallback_reason.unwrap().contains("h264_nvenc failed"));

    // Required: the failure is returned and the partial file removed.
    settings.hw_encode = NVENC;
    let failed = dir.join("failed.mp4");
    assert!(export(&src, &failed, &settings).is_err());
    assert!(!failed.exists());
}

#[test]
fn cancelling_an_nvenc_export_does_not_retry() {
    require_nvenc!();
    let dir = scratch("nvenc-cancel");
    let src = numbered_clip(&dir);
    let out = dir.join("cancel.mp4");
    let settings = ExportSettings::new(ExportPreset::Mp4H264).hw_encode(HwPolicy::Auto);
    let mut calls = 0;
    let r = export_with_progress(&src, &out, &settings, |p| {
        calls += 1;
        p.frames < 10
    });
    assert!(matches!(r, Err(Error::Cancelled)));
    assert!(!out.exists());
    assert!(
        calls < 60,
        "a cancelled export must not start again in software"
    );
}
