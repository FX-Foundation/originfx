//! GPU decoding (CUDA / VA-API). Every test skips itself when no GPU can be
//! opened, so these pass trivially on CI. On a machine with a GPU they check
//! that GPU frames are byte-identical to software frames, in order, after
//! seeks, through the cache, and across a fallback to software.

mod common;

use common::*;
use media_engine::{DecodeOptions, Error, FrameReader, HwApi, HwPolicy, VideoDecoder};

fn on(api: HwApi) -> DecodeOptions {
    DecodeOptions {
        hw: HwPolicy::Require(api),
    }
}

const AUTO: DecodeOptions = DecodeOptions { hw: HwPolicy::Auto };

const H264: &[&str] = &[
    "-c:v",
    "libx264",
    "-qp",
    "1",
    "-profile:v",
    "high",
    "-g",
    "15",
    "-pix_fmt",
    "yuv420p",
];

fn h264_clip(dir: &std::path::Path) -> std::path::PathBuf {
    gpu_id_clip(dir, "gpu-id256.mp4", H264)
}

/// Every frame of `path`, decoded in software.
fn software_frames(path: &std::path::Path) -> Vec<media_engine::VideoFrame> {
    VideoDecoder::open(path)
        .unwrap()
        .collect::<media_engine::Result<Vec<_>>>()
        .unwrap()
}

/// Decode all of `path` with `api` and compare with software, frame by frame.
/// Returns `Err(reason)` when this GPU cannot decode the clip at all.
fn compare_full_decode(path: &std::path::Path, api: HwApi) -> Result<(), String> {
    let reference = software_frames(path);
    let mut dec = VideoDecoder::open_with(path, on(api)).map_err(|e| e.to_string())?;
    let mut n = 0;
    loop {
        let frame = match dec.next_frame() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) if n == 0 => return Err(e.to_string()),
            Err(e) => panic!("{api}: error at frame {n}: {e}"),
        };
        assert_eq!(dec.hw_backend(), Some(api));
        let want = &reference[n];
        assert_eq!(frame.pts_secs, want.pts_secs, "{api}: pts of frame {n}");
        assert_eq!(
            (frame.width, frame.height),
            (want.width, want.height),
            "{api}: size of frame {n}"
        );
        assert!(
            frame.data == want.data,
            "{api}: frame {n} differs from software"
        );
        n += 1;
    }
    assert_eq!(n, reference.len(), "{api}: frame count");
    Ok(())
}

#[test]
fn default_and_off_are_software() {
    require_ffmpeg!();
    assert_eq!(DecodeOptions::default().hw, HwPolicy::Off);
    let clip = id_clip(&scratch("gpu-off"));
    let mut dec = VideoDecoder::open(&clip).unwrap();
    dec.next_frame().unwrap();
    assert_eq!(dec.hw_backend(), None);
    assert_eq!(dec.hw_fallback_reason(), None);
    let mut off = VideoDecoder::open_with(&clip, DecodeOptions::default()).unwrap();
    off.next_frame().unwrap();
    assert_eq!(off.hw_backend(), None);
    assert_eq!(off.hw_fallback_reason(), None);
}

#[test]
fn api_names_round_trip() {
    for api in HwApi::ALL {
        assert_eq!(HwApi::from_name(api.name()), Some(api));
    }
    assert_eq!(HwApi::from_name("metal"), None);
}

#[test]
fn h264_on_gpu_matches_software_frame_for_frame() {
    let apis = require_gpu!();
    let clip = h264_clip(&scratch("gpu-h264"));
    // The fixture's frames are all different, so `frame_id` is unambiguous.
    let greens: Vec<u8> = software_frames(&clip).iter().map(|f| f.data[1]).collect();
    assert!(
        greens.windows(2).all(|w| w[0] < w[1]),
        "fixture: {greens:?}"
    );
    for api in apis {
        // 8-bit 4:2:0 H.264 is supported by every NVDEC and VA-API device.
        compare_full_decode(&clip, api).unwrap_or_else(|e| panic!("{api}: {e}"));
    }
}

#[test]
fn other_codecs_on_gpu_match_software() {
    let apis = require_gpu!();
    let dir = scratch("gpu-codecs");
    let clips = [
        (
            "hevc 10-bit",
            gpu_id_clip(
                &dir,
                "gpu-hevc10-256.mkv",
                &[
                    "-c:v",
                    "libx265",
                    "-x265-params",
                    "qp=4:keyint=15:min-keyint=15:scenecut=0:log-level=error",
                    "-pix_fmt",
                    "yuv420p10le",
                ],
            ),
        ),
        (
            "vp9",
            gpu_id_clip(
                &dir,
                "gpu-vp9-256.webm",
                &[
                    "-c:v",
                    "libvpx-vp9",
                    "-crf",
                    "4",
                    "-b:v",
                    "0",
                    "-g",
                    "15",
                    "-pix_fmt",
                    "yuv420p",
                ],
            ),
        ),
        (
            "av1",
            gpu_id_clip(
                &dir,
                "gpu-av1-256.mkv",
                &[
                    "-c:v",
                    "libsvtav1",
                    "-crf",
                    "10",
                    "-g",
                    "15",
                    "-svtav1-params",
                    "lp=2",
                    "-pix_fmt",
                    "yuv420p",
                ],
            ),
        ),
        (
            "h264 full range",
            gpu_id_clip(
                &dir,
                "gpu-h264-jpeg.mp4",
                &[H264, &["-pix_fmt", "yuvj420p"]].concat(),
            ),
        ),
    ];
    for (name, clip) in &clips {
        for &api in &apis {
            // Not every GPU decodes every codec; say so instead of failing.
            if let Err(why) = compare_full_decode(clip, api) {
                eprintln!("note: {api} cannot decode {name} here: {why}");
            }
        }
    }
}

#[test]
fn gpu_seek_is_frame_accurate_including_mpeg_ts() {
    let apis = require_gpu!();
    let dir = scratch("gpu-seek");
    let mp4 = h264_clip(&dir);
    // No B-frames in the TS copy: with B-frames the first keyframe decodes before
    // the stream's start time, and seeking near 0 in MPEG-TS lands one GOP late in
    // software too (a known limit of the shared seek logic, not of GPU decoding).
    let no_b = gpu_id_clip(&dir, "gpu-id256-nob.mp4", &[H264, &["-bf", "0"]].concat());
    let ts = dir.join("gpu-id256.ts");
    if !ts.exists() {
        run(&[
            "-i",
            no_b.to_str().unwrap(),
            "-c",
            "copy",
            "-f",
            "mpegts",
            ts.to_str().unwrap(),
        ]);
    }
    for clip in [&mp4, &ts] {
        let reference = software_frames(clip);
        for &api in &apis {
            let mut dec = VideoDecoder::open_with(clip, on(api)).unwrap();
            for &n in &[
                0usize, 1, 14, 15, 16, 29, 30, 44, 45, 58, 59, 20, 3, 59, 0, 33,
            ] {
                let f = dec
                    .frame_at(n as f64 / 30.0 + 0.004)
                    .unwrap()
                    .expect("frame");
                assert_eq!(
                    frame_id(&reference, &f),
                    n as i64,
                    "{api} {clip:?}: seek to {n}"
                );
            }
            // Decoding continues in order after a seek.
            dec.seek(40.0 / 30.0 + 0.004).unwrap();
            for n in 40..60 {
                let f = dec.next_frame().unwrap().unwrap();
                assert_eq!(frame_id(&reference, &f), n, "{api} {clip:?}: after seek");
            }
            assert!(dec.next_frame().unwrap().is_none());
            assert_eq!(dec.hw_backend(), Some(api));
        }
    }
}

#[test]
fn frame_reader_on_gpu() {
    let apis = require_gpu!();
    let clip = h264_clip(&scratch("gpu-reader"));
    let reference = software_frames(&clip);
    for api in apis {
        let mut r = FrameReader::open_with(&clip, 64 * 1024 * 1024, on(api)).unwrap();
        for &n in &[10u64, 50, 2, 31, 59, 0, 11, 12, 45] {
            assert_eq!(
                frame_id(&reference, &r.frame(n).unwrap()),
                n as i64,
                "{api}"
            );
        }
        assert_eq!(frame_id(&reference, &r.frame(10_000).unwrap()), 59);
        assert_eq!(r.decoder().hw_backend(), Some(api));

        let mut exact = FrameReader::open_exact_with(&clip, 64 * 1024 * 1024, on(api)).unwrap();
        assert_eq!(exact.frame_count(), Some(60));
        assert_eq!(frame_id(&reference, &exact.frame(37).unwrap()), 37);
    }
}

#[test]
fn rotation_applies_to_gpu_frames() {
    let apis = require_gpu!();
    let dir = scratch("gpu-rotation");
    let src = h264_clip(&dir);
    let rotated = dir.join("gpu-rot90-256.mp4");
    if !rotated.exists() {
        run(&[
            "-display_rotation",
            "90",
            "-i",
            src.to_str().unwrap(),
            "-c",
            "copy",
            rotated.to_str().unwrap(),
        ]);
    }
    let reference = software_frames(&rotated);
    for api in apis {
        let mut dec = VideoDecoder::open_with(&rotated, on(api)).unwrap();
        let f = dec.frame_at(0.5).unwrap().unwrap();
        assert_eq!(dec.info().rotation, reference_rotation(&rotated));
        assert!(f.data == reference[15].data, "{api}: rotated frame differs");
    }
}

fn reference_rotation(path: &std::path::Path) -> u32 {
    VideoDecoder::open(path).unwrap().info().rotation
}

#[test]
fn gpu_failure_mid_stream_continues_in_software() {
    let apis = require_gpu!();
    let clip = h264_clip(&scratch("gpu-fail"));
    let reference = software_frames(&clip);
    for api in apis {
        let auto = DecodeOptions {
            hw: if api == HwApi::Cuda {
                HwPolicy::Auto
            } else {
                HwPolicy::Require(api)
            },
        };
        // Sequential: fail after 20 GPU frames; output must still be 0..59 in order.
        let mut dec = VideoDecoder::open_with(&clip, auto).unwrap();
        dec.simulate_gpu_failure_after(20);
        if matches!(auto.hw, HwPolicy::Require(_)) {
            for _ in 0..20 {
                dec.next_frame().unwrap().unwrap();
            }
            // Required GPU: the failure is an error, not a silent switch.
            assert!(matches!(dec.next_frame(), Err(Error::Unsupported(_))));
            continue;
        }
        let mut n = 0;
        while let Some(f) = dec.next_frame().unwrap() {
            assert_eq!(frame_id(&reference, &f), n, "{api}: frame after fallback");
            n += 1;
        }
        assert_eq!(n, 60);
        assert_eq!(dec.hw_backend(), None);
        assert!(dec.hw_fallback_reason().unwrap().contains("simulated"));

        // Just after a seek, before the target frame was reached.
        let mut dec = VideoDecoder::open_with(&clip, auto).unwrap();
        dec.simulate_gpu_failure_after(3);
        let f = dec.frame_at(40.0 / 30.0 + 0.004).unwrap().unwrap();
        assert_eq!(frame_id(&reference, &f), 40);
        for n in 41..60 {
            assert_eq!(frame_id(&reference, &dec.next_frame().unwrap().unwrap()), n);
        }
        assert_eq!(dec.hw_backend(), None);

        // On the very frame a seek returns: only that frame is downloaded (the
        // ones skipped on the way stay on the GPU), so it is the one that fails.
        let mut dec = VideoDecoder::open_with(&clip, auto).unwrap();
        dec.simulate_gpu_failure_after(0);
        for &n in &[37i64, 52, 8] {
            let f = dec.frame_at(n as f64 / 30.0 + 0.004).unwrap().unwrap();
            assert_eq!(frame_id(&reference, &f), n, "{api}: seek after failure");
        }
        assert_eq!(dec.hw_backend(), None);
    }
}

#[test]
fn rgb_vp9_stays_in_software() {
    require_ffmpeg!();
    // VP9 profile 1 holding RGB: some VA-API drivers accept it but return wrong pixels.
    let clip = gpu_id_clip(
        &scratch("gpu-rgb"),
        "gpu-vp9-rgb.webm",
        &[
            "-c:v",
            "libvpx-vp9",
            "-crf",
            "4",
            "-b:v",
            "0",
            "-pix_fmt",
            "gbrp",
        ],
    );
    let reference = software_frames(&clip);
    let mut dec = VideoDecoder::open_with(&clip, AUTO).unwrap();
    let f = dec.next_frame().unwrap().unwrap();
    assert_eq!(frame_id(&reference, &f), 0);
    assert_eq!(dec.hw_backend(), None);
    assert!(dec.hw_fallback_reason().unwrap().contains("4:2:0"));
    let required = VideoDecoder::open_with(&clip, on(HwApi::Vaapi));
    assert!(matches!(required, Err(Error::Unsupported(_))));
}

#[test]
fn stream_the_gpu_rejects_falls_back_or_errors() {
    let apis = require_gpu!();
    // Lossless H.264 is High 4:4:4 Predictive, which NVDEC and VA-API do not decode.
    let clip = id_clip(&scratch("gpu-444"));
    let mut dec = VideoDecoder::open_with(&clip, AUTO).unwrap();
    let mut n = 0;
    while let Some(f) = dec.next_frame().unwrap() {
        assert_eq!(frame_number(&f.data), n);
        n += 1;
    }
    assert_eq!(n, 60);
    if dec.hw_backend().is_none() {
        assert!(dec.hw_fallback_reason().is_some());
    }
    for api in apis {
        let mut dec = match VideoDecoder::open_with(&clip, on(api)) {
            Ok(d) => d,
            Err(Error::Unsupported(_)) => continue,
            Err(e) => panic!("{api}: unexpected error {e}"),
        };
        match dec.next_frame() {
            Err(Error::Unsupported(_)) => {}
            Ok(_) => assert_eq!(
                dec.hw_backend(),
                Some(api),
                "{api}: decoded 4:4:4 on the GPU"
            ),
            Err(e) => panic!("{api}: unexpected error {e}"),
        }
    }
}
