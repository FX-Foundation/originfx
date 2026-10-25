mod common;

use common::*;
use media_engine::{
    export, export_with_progress, probe, Error, ExportPreset, ExportSettings, Quality, VideoDecoder,
};

fn expected_video_codec(p: ExportPreset) -> Option<&'static str> {
    use ExportPreset::*;
    Some(match p {
        Mp4H264 | MovH264 | MkvH264 => "h264",
        Mp4H265 | MkvH265 => "hevc",
        WebmVp9 => "vp9",
        WebmVp8 => "vp8",
        WebmAv1 => "av1",
        MovProRes422 | MovProRes422Hq | MovProRes4444 => "prores",
        MovDnxhrHq => "dnxhd",
        AviMjpeg => "mjpeg",
        Gif => "gif",
        Mp3 | AacM4a | Wav | Flac | OpusOgg => return None,
    })
}

fn expected_audio_codec(p: ExportPreset) -> Option<&'static str> {
    use ExportPreset::*;
    Some(match p {
        Mp4H264 | MovH264 | MkvH264 | Mp4H265 | MkvH265 | AacM4a => "aac",
        WebmVp9 | WebmVp8 | WebmAv1 | OpusOgg => "opus",
        MovProRes422 | MovProRes422Hq | MovProRes4444 | MovDnxhrHq | AviMjpeg | Wav => "pcm_s16le",
        Mp3 => "mp3",
        Flac => "flac",
        Gif => return None,
    })
}

#[test]
fn every_available_preset_produces_a_valid_file() {
    require_ffmpeg!();
    let dir = scratch("presets");
    let src = pattern_clip(&dir, "src.mp4", "320x180", 25, 2);
    let mut tested = 0;

    for preset in ExportPreset::ALL {
        if !preset.is_available() {
            eprintln!("skipping {}: encoder not in this FFmpeg build", preset.id());
            continue;
        }
        let out = dir.join(format!("out-{}.{}", preset.id(), preset.extension()));
        let _ = std::fs::remove_file(&out);
        let settings = ExportSettings::new(preset).quality(Quality::Draft);
        let report =
            export(&src, &out, &settings).unwrap_or_else(|e| panic!("{} failed: {e}", preset.id()));
        assert!(report.output_bytes > 0, "{}", preset.id());

        let info = probe(&out).unwrap_or_else(|e| panic!("probing {}: {e}", preset.id()));
        match expected_video_codec(preset) {
            Some(c) => {
                let v = info
                    .primary_video()
                    .unwrap_or_else(|| panic!("{}: no video", preset.id()));
                assert_eq!(v.codec, c, "{}", preset.id());
                assert_eq!((v.width, v.height), (320, 180), "{}", preset.id());
            }
            None => assert!(info.video.is_empty(), "{}: unexpected video", preset.id()),
        }
        match expected_audio_codec(preset) {
            Some(c) => {
                let a = info
                    .primary_audio()
                    .unwrap_or_else(|| panic!("{}: no audio", preset.id()));
                assert_eq!(a.codec, c, "{}", preset.id());
            }
            None => assert!(info.audio.is_empty(), "{}: unexpected audio", preset.id()),
        }
        // Every format, FLAC included, must report its length.
        let d = info
            .duration_secs
            .unwrap_or_else(|| panic!("{}: no duration", preset.id()));
        assert!((d - 2.0).abs() < 0.35, "{}: duration {d}", preset.id());
        tested += 1;
    }
    assert!(tested >= 10, "only {tested} presets could be tested");
}

#[test]
fn h264_roundtrip_keeps_every_frame_in_order() {
    require_ffmpeg!();
    let dir = scratch("roundtrip");
    let src = id_clip(&dir);
    let out = dir.join("rt.mp4");
    let report = export(
        &src,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264).quality(Quality::High),
    )
    .unwrap();
    assert_eq!(report.frames, 60);

    let mut dec = VideoDecoder::open(&out).unwrap();
    let mut greens = Vec::new();
    while let Some(f) = dec.next_frame().unwrap() {
        greens.push(i32::from(f.data[1]));
    }
    assert_eq!(greens.len(), 60);
    for w in greens.windows(2) {
        assert!(w[1] >= w[0] - 2, "frame order scrambled: {greens:?}");
    }
    assert!(
        greens[59] - greens[0] > 200,
        "content changed too much: {greens:?}"
    );
}

#[test]
fn scaling_keeps_aspect_and_never_upscales() {
    require_ffmpeg!();
    let dir = scratch("scale");
    let src = pattern_clip(&dir, "src.mp4", "320x180", 25, 1);

    let out = dir.join("small.mp4");
    export(
        &src,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264)
            .quality(Quality::Draft)
            .height(90),
    )
    .unwrap();
    let v = probe(&out).unwrap().video[0].clone();
    assert_eq!((v.width, v.height), (160, 90));

    let out = dir.join("noup.mp4");
    export(
        &src,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264)
            .quality(Quality::Draft)
            .height(1080),
    )
    .unwrap();
    let v = probe(&out).unwrap().video[0].clone();
    assert_eq!((v.width, v.height), (320, 180));
}

#[test]
fn fps_conform_changes_frame_count_not_duration() {
    require_ffmpeg!();
    let dir = scratch("fps");
    let src = pattern_clip(&dir, "src.mp4", "160x90", 30, 2);
    let out = dir.join("fps15.mp4");
    let r = export(
        &src,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264)
            .quality(Quality::Draft)
            .fps(15.0),
    )
    .unwrap();
    assert!((29..=31).contains(&r.frames), "frames {}", r.frames);
    let v = &probe(&out).unwrap().video[0];
    let fr = v.frame_rate.map(|r| r.as_f64());
    assert!(
        matches!(fr, Some(f) if (f - 15.0).abs() < 0.1),
        "frame rate {fr:?}"
    );
}

#[test]
fn progress_is_monotonic_and_cancel_cleans_up() {
    require_ffmpeg!();
    let dir = scratch("progress");
    let src = pattern_clip(&dir, "src.mp4", "320x180", 25, 3);

    let out = dir.join("p.mp4");
    let mut seen = Vec::new();
    export_with_progress(
        &src,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264).quality(Quality::Draft),
        |p| {
            seen.push(p.position_secs);
            true
        },
    )
    .unwrap();
    assert!(seen.len() >= 3, "{seen:?}");
    assert!(seen.windows(2).all(|w| w[1] >= w[0]), "{seen:?}");
    assert!((seen.last().unwrap() - 3.0).abs() < 0.4);

    let out = dir.join("cancelled.mp4");
    let r = export_with_progress(
        &src,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264).quality(Quality::Draft),
        |_| false,
    );
    assert!(matches!(r, Err(Error::Cancelled)));
    assert!(!out.exists(), "partial file must be removed");
}

#[test]
fn audio_only_and_missing_stream_cases() {
    require_ffmpeg!();
    let dir = scratch("streams");
    let src = pattern_clip(&dir, "src.mp4", "160x90", 25, 2);

    // audio-only preset from a video file works
    let mp3 = dir.join("a.mp3");
    export(&src, &mp3, &ExportSettings::new(ExportPreset::Mp3)).unwrap();
    let i = probe(&mp3).unwrap();
    assert!(i.video.is_empty() && i.audio.len() == 1);

    // video-only source + video preset: exports without audio
    let silent = dir.join("silent.mp4");
    if !silent.exists() {
        assert!(std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-i",
                src.to_str().unwrap(),
                "-an",
                "-c:v",
                "copy",
                silent.to_str().unwrap()
            ])
            .status()
            .unwrap()
            .success());
    }
    let out = dir.join("silent-out.mp4");
    export(
        &silent,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264).quality(Quality::Draft),
    )
    .unwrap();
    assert!(probe(&out).unwrap().audio.is_empty());

    // video-only source + audio-only preset: clean error
    let r = export(
        &silent,
        dir.join("x.mp3"),
        &ExportSettings::new(ExportPreset::Mp3),
    );
    assert!(matches!(r, Err(Error::NoStream("audio"))), "{r:?}");
}

#[test]
fn missing_input_is_not_found() {
    let r = export(
        "/no/such/file.mp4",
        "/tmp/never.mp4",
        &ExportSettings::new(ExportPreset::Mp4H264),
    );
    assert!(matches!(r, Err(Error::NotFound(_))));
}

#[test]
fn trim_keeps_exactly_the_requested_frames() {
    require_ffmpeg!();
    let dir = scratch("trim");
    let src = id_clip(&dir); // 60 frames at 30 fps, frame n has green = n * 4
    let out = dir.join("trim.mp4");
    let _ = std::fs::remove_file(&out);
    let report = export(
        &src,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264)
            .quality(Quality::High)
            .trim(Some(0.5), Some(1.5)),
    )
    .unwrap();
    assert_eq!(report.frames, 30, "1 s at 30 fps");
    assert!(
        (report.duration_secs - 1.0).abs() < 0.1,
        "{}",
        report.duration_secs
    );

    let mut dec = VideoDecoder::open(&out).unwrap();
    let mut greens = Vec::new();
    while let Some(f) = dec.next_frame().unwrap() {
        greens.push(i32::from(f.data[1]));
    }
    assert_eq!(greens.len(), 30);
    // Source frame 15 is green 60, frame 44 is green 176 (lossy re-encode: allow slack).
    assert!(
        (greens[0] - 60).abs() <= 12,
        "first frame green {}",
        greens[0]
    );
    assert!(
        (greens[29] - 176).abs() <= 12,
        "last frame green {}",
        greens[29]
    );

    let info = probe(&out).unwrap();
    assert!((info.duration_secs.unwrap_or(0.0) - 1.0).abs() < 0.15);
    if let Some(a) = info.audio.first() {
        assert!((a.duration_secs.unwrap_or(1.0) - 1.0).abs() < 0.2);
    }
}

#[test]
fn trim_with_only_a_start_keeps_the_rest() {
    require_ffmpeg!();
    let dir = scratch("trim-start");
    let src = id_clip(&dir);
    let out = dir.join("tail.mp4");
    let _ = std::fs::remove_file(&out);
    let report = export(
        &src,
        &out,
        &ExportSettings::new(ExportPreset::Mp4H264)
            .quality(Quality::Draft)
            .trim(Some(1.0), None),
    )
    .unwrap();
    assert_eq!(report.frames, 30);
}

#[test]
fn trim_works_for_audio_only_exports() {
    require_ffmpeg!();
    let dir = scratch("trim-audio");
    let src = id_clip(&dir);
    let out = dir.join("cut.wav");
    let _ = std::fs::remove_file(&out);
    export(
        &src,
        &out,
        &ExportSettings::new(ExportPreset::Wav).trim(Some(0.5), Some(1.5)),
    )
    .unwrap();
    let a = probe(&out).unwrap().audio[0].clone();
    let d = a.duration_secs.unwrap();
    assert!((d - 1.0).abs() < 0.1, "wav is {d} s");
}

#[test]
fn trim_rejects_nonsense_ranges() {
    require_ffmpeg!();
    let dir = scratch("trim-bad");
    let src = id_clip(&dir);
    let out = dir.join("bad.mp4");
    for (s, e) in [
        (Some(1.0), Some(0.5)),
        (Some(-1.0), None),
        (None, Some(0.0)),
    ] {
        let r = export(
            &src,
            &out,
            &ExportSettings::new(ExportPreset::Mp4H264).trim(s, e),
        );
        assert!(
            matches!(r, Err(Error::InvalidArgument(_))),
            "{s:?}..{e:?} gave {r:?}"
        );
    }
}

#[test]
fn long_gifs_export_with_a_streaming_palette() {
    require_ffmpeg!();
    if !ExportPreset::Gif.is_available() {
        return;
    }
    let dir = scratch("gif-long");
    let src = pattern_clip(&dir, "long.mp4", "160x90", 10, 25); // over the 20 s threshold
    let out = dir.join("long.gif");
    let _ = std::fs::remove_file(&out);
    let report = export(
        &src,
        &out,
        &ExportSettings::new(ExportPreset::Gif).quality(Quality::Draft),
    )
    .unwrap();
    assert_eq!(report.frames, 25 * 10);
    let v = probe(&out).unwrap().video[0].clone();
    assert_eq!(v.codec, "gif");
}
