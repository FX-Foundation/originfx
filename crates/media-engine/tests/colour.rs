mod common;

use common::*;
use media_engine::{export, probe, ExportPreset, ExportSettings, Quality, VideoDecoder};

fn decoded_error(path: &std::path::Path) -> i32 {
    let mut dec = VideoDecoder::open(path).unwrap();
    colour_error(&dec.next_frame().unwrap().unwrap())
}

#[test]
fn tagged_709_and_601_clips_decode_to_the_right_colour() {
    require_ffmpeg!();
    let dir = scratch("colour-decode");
    let hd = colour_clip(&dir, "hd709.mp4", "640x360", "bt709", TAGS_709);
    let sd = colour_clip(&dir, "sd601.mp4", "320x240", "bt601", TAGS_601);
    // 601 maths on 709 data is off by ~15+ levels, so a tight bound proves the matrix is honoured.
    assert!(decoded_error(&hd) <= 6, "709 clip: {}", decoded_error(&hd));
    assert!(decoded_error(&sd) <= 6, "601 clip: {}", decoded_error(&sd));
}

#[test]
fn untagged_hd_is_treated_as_709() {
    require_ffmpeg!();
    let dir = scratch("colour-untagged");
    let hd = colour_clip(&dir, "hd-untagged.mp4", "1280x720", "bt709", &[]);
    assert!(probe(&hd).unwrap().video[0].colour.space.is_none());
    assert!(decoded_error(&hd) <= 6, "{}", decoded_error(&hd));
}

#[test]
fn probe_reports_colour_tags() {
    require_ffmpeg!();
    let dir = scratch("colour-probe");
    let hd = colour_clip(&dir, "hd709.mp4", "640x360", "bt709", TAGS_709);
    let c = probe(&hd).unwrap().video[0].colour.clone();
    assert_eq!(c.space.as_deref(), Some("bt709"));
    assert_eq!(c.primaries.as_deref(), Some("bt709"));
    assert_eq!(c.range.as_deref(), Some("tv"));
    assert!(!c.is_hdr());
}

#[test]
fn export_keeps_colours_and_tags_the_output() {
    require_ffmpeg!();
    let dir = scratch("colour-export");
    let cases = [
        (
            colour_clip(&dir, "hd709.mp4", "640x360", "bt709", TAGS_709),
            "bt709",
        ),
        (
            colour_clip(&dir, "sd601.mp4", "320x240", "bt601", TAGS_601),
            "smpte170m",
        ),
        (
            colour_clip(&dir, "hd-untagged.mp4", "1280x720", "bt709", &[]),
            "bt709",
        ),
    ];
    for (src, expected_space) in cases {
        for preset in [ExportPreset::Mp4H264, ExportPreset::MkvH264] {
            let name = src.file_stem().unwrap().to_string_lossy().to_string();
            let out = dir.join(format!("out-{name}.{}", preset.extension()));
            let _ = std::fs::remove_file(&out);
            export(
                &src,
                &out,
                &ExportSettings::new(preset).quality(Quality::High),
            )
            .unwrap();

            let c = probe(&out).unwrap().video[0].colour.clone();
            assert_eq!(c.space.as_deref(), Some(expected_space), "{name} space");
            assert_eq!(c.range.as_deref(), Some("tv"), "{name} range");
            assert!(c.primaries.is_some() && c.transfer.is_some(), "{name}");
            let err = decoded_error(&out);
            assert!(err <= 8, "{name} {preset:?}: colour drifted by {err}");
        }
    }
}
