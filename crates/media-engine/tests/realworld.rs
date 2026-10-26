// SPDX-License-Identifier: GPL-3.0-or-later
//! Footage that looks more like the real world: surround sound, several audio
//! tracks, 10-bit and HDR video.

mod common;

use common::*;
use media_engine::{export, probe, Error, ExportPreset, ExportSettings, Quality};

fn five_one_clip(dir: &std::path::Path) -> std::path::PathBuf {
    let out = dir.join("surround51.mp4");
    if !out.exists() {
        run(&[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=2,pan=5.1|FL=c0|FR=c0|FC=c0|LFE=c0|BL=c0|BR=c0",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=25:duration=2",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-c:a",
            "ac3",
            out.to_str().unwrap(),
        ]);
    }
    out
}

/// Three audio tracks: stream 1 mono 48 kHz, stream 2 mono 44.1 kHz, stream 3 5.1.
fn multi_audio_clip(dir: &std::path::Path) -> std::path::PathBuf {
    let out = dir.join("multi.mkv");
    if !out.exists() {
        run(&[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=25:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=220:sample_rate=48000:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=44100:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=880:sample_rate=48000:duration=2,pan=5.1|FL=c0|FR=c0|FC=c0|LFE=c0|BL=c0|BR=c0",
            "-map",
            "0",
            "-map",
            "1",
            "-map",
            "2",
            "-map",
            "3",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-c:a",
            "aac",
            out.to_str().unwrap(),
        ]);
    }
    out
}

/// 10-bit HEVC with HDR10-style tags (BT.2020 matrix, PQ transfer).
fn hdr10_clip(dir: &std::path::Path) -> std::path::PathBuf {
    let out = dir.join("hdr10.mp4");
    if !out.exists() {
        run(&[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=25:duration=2",
            "-vf",
            "format=yuv420p10le",
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-x265-params",
            "log-level=none",
            "-colorspace",
            "bt2020nc",
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "smpte2084",
            "-color_range",
            "tv",
            out.to_str().unwrap(),
        ]);
    }
    out
}

fn channels_of(path: &std::path::Path) -> u16 {
    probe(path).unwrap().audio[0].channels
}

#[test]
fn surround_sound_is_kept_where_the_format_allows() {
    require_ffmpeg!();
    let dir = scratch("surround");
    let src = five_one_clip(&dir);
    assert_eq!(channels_of(&src), 6);

    for (preset, expected) in [
        (ExportPreset::Mp4H264, 6),
        (ExportPreset::MkvH265, 6),
        (ExportPreset::WebmVp9, 6),
        (ExportPreset::Flac, 6),
        (ExportPreset::Wav, 6),
        (ExportPreset::Mp3, 2), // MP3 cannot carry surround
    ] {
        if !preset.is_available() {
            eprintln!("skipping {}", preset.id());
            continue;
        }
        let out = dir.join(format!("out-{}.{}", preset.id(), preset.extension()));
        let _ = std::fs::remove_file(&out);
        export(
            &src,
            &out,
            &ExportSettings::new(preset).quality(Quality::Draft),
        )
        .unwrap_or_else(|e| panic!("{}: {e}", preset.id()));
        assert_eq!(channels_of(&out), expected, "{}", preset.id());
    }
}

#[test]
fn a_specific_audio_track_can_be_chosen() {
    require_ffmpeg!();
    let dir = scratch("tracks");
    let src = multi_audio_clip(&dir);
    let info = probe(&src).unwrap();
    assert_eq!(info.audio.len(), 3);
    let indices: Vec<usize> = info.audio.iter().map(|a| a.index).collect();

    let pick = |i: usize| {
        let out = dir.join(format!("track{i}.wav"));
        let _ = std::fs::remove_file(&out);
        export(
            &src,
            &out,
            &ExportSettings::new(ExportPreset::Wav).audio_stream(i),
        )
        .unwrap();
        probe(&out).unwrap().audio[0].clone()
    };
    let a = pick(indices[2]);
    assert_eq!(a.channels, 6, "5.1 track");
    let b = pick(indices[1]);
    assert_eq!(
        (b.channels, b.sample_rate),
        (1, 44_100),
        "44.1 kHz mono track"
    );
}

#[test]
fn wrong_stream_choices_are_clean_errors() {
    require_ffmpeg!();
    let dir = scratch("tracks-bad");
    let src = multi_audio_clip(&dir);
    let out = dir.join("bad.wav");
    // Stream 0 is the video stream, 99 does not exist.
    for i in [0, 99] {
        let r = export(
            &src,
            &out,
            &ExportSettings::new(ExportPreset::Wav).audio_stream(i),
        );
        assert!(
            matches!(r, Err(Error::InvalidArgument(_))),
            "stream {i}: {r:?}"
        );
    }
}

#[test]
fn ten_bit_sources_stay_ten_bit_on_hevc_and_keep_their_hdr_tags() {
    require_ffmpeg!();
    let dir = scratch("tenbit");
    let src = hdr10_clip(&dir);
    let v = probe(&src).unwrap().video[0].clone();
    assert_eq!(v.pixel_format, "yuv420p10le");
    assert!(v.colour.is_hdr());

    let out = dir.join("out.mkv");
    let _ = std::fs::remove_file(&out);
    export(
        &src,
        &out,
        &ExportSettings::new(ExportPreset::MkvH265).quality(Quality::Draft),
    )
    .unwrap();
    let o = probe(&out).unwrap().video[0].clone();
    assert_eq!(o.pixel_format, "yuv420p10le");
    assert_eq!(o.colour.space.as_deref(), Some("bt2020nc"));
    assert_eq!(o.colour.transfer.as_deref(), Some("smpte2084"));
    assert!(o.colour.is_hdr());
}

#[test]
fn bit_depth_can_be_forced_either_way() {
    require_ffmpeg!();
    let dir = scratch("tenbit-force");
    let hdr = hdr10_clip(&dir);
    let sdr = colour_clip(&dir, "plain.mp4", "320x180", "bt709", TAGS_709);

    // H.264 stays 8-bit by default, even from a 10-bit source...
    let out = dir.join("h264-default.mp4");
    let _ = std::fs::remove_file(&out);
    export(
        &hdr,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264).quality(Quality::Draft),
    )
    .unwrap();
    assert_eq!(probe(&out).unwrap().video[0].pixel_format, "yuv420p");

    // ...but can be asked for 10-bit...
    let out = dir.join("h264-10.mp4");
    let _ = std::fs::remove_file(&out);
    export(
        &sdr,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264)
            .quality(Quality::Draft)
            .bit_depth(10),
    )
    .unwrap();
    assert_eq!(probe(&out).unwrap().video[0].pixel_format, "yuv420p10le");

    // ...and HEVC from a 10-bit source can be forced down to 8-bit.
    let out = dir.join("hevc-8.mp4");
    let _ = std::fs::remove_file(&out);
    export(
        &hdr,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H265)
            .quality(Quality::Draft)
            .bit_depth(8),
    )
    .unwrap();
    assert_eq!(probe(&out).unwrap().video[0].pixel_format, "yuv420p");
}

#[test]
fn impossible_bit_depths_are_rejected() {
    require_ffmpeg!();
    let dir = scratch("tenbit-bad");
    let src = colour_clip(&dir, "plain.mp4", "320x180", "bt709", TAGS_709);
    let out = dir.join("bad.webm");
    for (preset, bits) in [(ExportPreset::WebmVp8, 10), (ExportPreset::Mp4H264, 12)] {
        if !preset.is_available() {
            continue;
        }
        let r = export(&src, &out, &ExportSettings::new(preset).bit_depth(bits));
        assert!(
            r.is_err(),
            "{preset:?} at {bits} bits should fail, got {r:?}"
        );
    }
}
