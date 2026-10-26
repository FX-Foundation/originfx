//! Text subtitles and chapters: reading them and carrying them through export.

mod common;

use common::*;
use media_engine::{
    export, probe, read_subtitles, ExportPreset, ExportSettings, Quality, SubtitleCue,
};
use std::path::PathBuf;

const SRT: &str = "1\n00:00:01,000 --> 00:00:02,500\nHello there\n\n\
2\n00:00:04,000 --> 00:00:06,000\n<i>Second</i> line,\nwith two rows\n\n\
3\n00:00:08,000 --> 00:00:09,500\nLast one\n";

const CHAPTERS: &str = ";FFMETADATA1\n\
[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=4000\ntitle=Intro\n\
[CHAPTER]\nTIMEBASE=1/1000\nSTART=4000\nEND=7000\ntitle=Middle\n\
[CHAPTER]\nTIMEBASE=1/1000\nSTART=7000\nEND=10000\ntitle=Outro\n";

/// A 10 s MKV with one English SRT track and three chapters.
fn source(dir: &std::path::Path) -> PathBuf {
    let out = dir.join("subs-source.mkv");
    if !out.exists() {
        let srt = dir.join("subs.srt");
        let meta = dir.join("chapters.txt");
        std::fs::write(&srt, SRT).unwrap();
        std::fs::write(&meta, CHAPTERS).unwrap();
        run(&[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=128x72:rate=10:duration=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=44100:duration=10",
            "-i",
            srt.to_str().unwrap(),
            "-i",
            meta.to_str().unwrap(),
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "2:s",
            "-map_chapters",
            "3",
            "-metadata:s:s:0",
            "language=eng",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-c:s",
            "srt",
            out.to_str().unwrap(),
        ]);
    }
    out
}

fn settings(preset: ExportPreset) -> ExportSettings {
    ExportSettings::new(preset)
        .quality(Quality::Draft)
        .height(72)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.06
}

fn assert_cues(got: &[SubtitleCue], want: &[(f64, f64, &str)]) {
    assert_eq!(got.len(), want.len(), "cue count: {got:?}");
    for (g, (s, e, t)) in got.iter().zip(want) {
        assert_eq!(g.text, *t, "text");
        assert!(close(g.start_secs, *s), "start {} vs {s}", g.start_secs);
        assert!(close(g.end_secs, *e), "end {} vs {e}", g.end_secs);
    }
}

#[test]
fn probe_reports_subtitle_details_and_chapters() {
    require_ffmpeg!();
    let src = source(&scratch("subs-probe"));
    let info = probe(&src).unwrap();
    assert_eq!(info.subtitles.len(), 1);
    let s = &info.subtitles[0];
    assert!(s.text_based);
    assert_eq!(s.language.as_deref(), Some("eng"));
    assert_eq!(info.chapters.len(), 3);
    assert_eq!(info.chapters[1].title.as_deref(), Some("Middle"));
    assert!(close(info.chapters[1].start_secs, 4.0));
    assert!(close(info.chapters[2].end_secs, 10.0));
}

#[test]
fn read_subtitles_gives_plain_timed_text() {
    require_ffmpeg!();
    let src = source(&scratch("subs-read"));
    let idx = probe(&src).unwrap().subtitles[0].index;
    let cues = read_subtitles(&src, idx).unwrap();
    assert_cues(
        &cues,
        &[
            (1.0, 2.5, "Hello there"),
            (4.0, 6.0, "Second line,\nwith two rows"),
            (8.0, 9.5, "Last one"),
        ],
    );
    // A video stream is not a subtitle stream.
    let v = probe(&src).unwrap().video[0].index;
    assert!(read_subtitles(&src, v).is_err());
}

#[test]
fn ass_subtitles_lose_their_styling() {
    require_ffmpeg!();
    let dir = scratch("subs-ass");
    let ass = dir.join("s.ass");
    std::fs::write(
        &ass,
        "[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\n\
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
Style: Default,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,1,0,2,10,10,10,1\n\n\
[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,{\\i1}Styled{\\i0}, with comma\\Nnext row\n",
    )
    .unwrap();
    let clip = dir.join("ass.mkv");
    run(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=64x64:rate=10:duration=4",
        "-i",
        ass.to_str().unwrap(),
        "-c:v",
        "libx264",
        "-c:s",
        "ass",
        clip.to_str().unwrap(),
    ]);
    let idx = probe(&clip).unwrap().subtitles[0].index;
    let cues = read_subtitles(&clip, idx).unwrap();
    assert_cues(&cues, &[(1.0, 3.0, "Styled, with comma\nnext row")]);
}

#[test]
fn mkv_export_carries_subtitles_and_chapters() {
    require_ffmpeg!();
    let dir = scratch("subs-mkv");
    let src = source(&dir);
    let out = dir.join("out.mkv");
    let report = export(
        &src,
        &out,
        &settings(ExportPreset::MkvH264)
            .subtitles(true)
            .chapters(true),
    )
    .unwrap();
    assert_eq!(report.subtitle_tracks, 1);
    assert_eq!(report.chapters, 3);
    assert!(report.notes.is_empty(), "{:?}", report.notes);

    let info = probe(&out).unwrap();
    assert_eq!(info.subtitles.len(), 1);
    assert_eq!(info.subtitles[0].language.as_deref(), Some("eng"));
    assert_eq!(info.chapters.len(), 3);
    assert_eq!(info.chapters[0].title.as_deref(), Some("Intro"));
    let cues = read_subtitles(&out, info.subtitles[0].index).unwrap();
    assert_cues(
        &cues,
        &[
            (1.0, 2.5, "Hello there"),
            (4.0, 6.0, "Second line,\nwith two rows"),
            (8.0, 9.5, "Last one"),
        ],
    );
}

#[test]
fn mp4_export_writes_mov_text_with_gaps_and_chapters() {
    require_ffmpeg!();
    let dir = scratch("subs-mp4");
    let src = source(&dir);
    let out = dir.join("out.mp4");
    let report = export(
        &src,
        &out,
        &settings(ExportPreset::Mp4H264)
            .subtitles(true)
            .chapters(true),
    )
    .unwrap();
    assert_eq!(report.subtitle_tracks, 1);
    assert_eq!(report.chapters, 3);

    let info = probe(&out).unwrap();
    assert_eq!(info.subtitles.len(), 1);
    assert_eq!(info.subtitles[0].codec, "mov_text");
    assert_eq!(info.subtitles[0].language.as_deref(), Some("eng"));
    assert_eq!(info.chapters.len(), 3);
    assert_eq!(info.chapters[2].title.as_deref(), Some("Outro"));
    let cues = read_subtitles(&out, info.subtitles[0].index).unwrap();
    assert_cues(
        &cues,
        &[
            (1.0, 2.5, "Hello there"),
            (4.0, 6.0, "Second line,\nwith two rows"),
            (8.0, 9.5, "Last one"),
        ],
    );
}

#[test]
fn trimmed_export_shifts_and_clips_subtitles_and_chapters() {
    require_ffmpeg!();
    let dir = scratch("subs-trim");
    let src = source(&dir);
    let out = dir.join("trim.mkv");
    let report = export(
        &src,
        &out,
        &settings(ExportPreset::MkvH264)
            .trim(Some(3.0), Some(8.5))
            .subtitles(true)
            .chapters(true),
    )
    .unwrap();
    // Chapters: Intro 0-4 -> 0-1, Middle 4-7 -> 1-4, Outro 7-10 -> 4-5.5.
    assert_eq!(report.chapters, 3);
    let info = probe(&out).unwrap();
    assert!(close(info.chapters[0].end_secs, 1.0), "{:?}", info.chapters);
    assert!(close(info.chapters[2].end_secs, 5.5), "{:?}", info.chapters);
    // Subtitles: only "Second line" (4-6 -> 1-3) and "Last one" (8-9.5 -> 5-5.5).
    let cues = read_subtitles(&out, info.subtitles[0].index).unwrap();
    assert_cues(
        &cues,
        &[
            (1.0, 3.0, "Second line,\nwith two rows"),
            (5.0, 5.5, "Last one"),
        ],
    );
}

#[test]
fn nothing_is_carried_over_unless_asked() {
    require_ffmpeg!();
    let dir = scratch("subs-default");
    let src = source(&dir);
    let out = dir.join("plain.mkv");
    let report = export(&src, &out, &settings(ExportPreset::MkvH264)).unwrap();
    assert_eq!(report.subtitle_tracks, 0);
    assert_eq!(report.chapters, 0);
    let info = probe(&out).unwrap();
    assert!(info.subtitles.is_empty());
    assert!(info.chapters.is_empty());
}

#[test]
fn formats_that_cannot_hold_subtitles_say_so() {
    require_ffmpeg!();
    let dir = scratch("subs-webm");
    let src = source(&dir);
    let out = dir.join("out.webm");
    let report = export(
        &src,
        &out,
        &settings(ExportPreset::WebmVp8)
            .subtitles(true)
            .chapters(true),
    )
    .unwrap();
    assert_eq!(report.subtitle_tracks, 0);
    assert_eq!(report.chapters, 0);
    assert!(
        report.notes.iter().any(|n| n.contains("webm")),
        "{:?}",
        report.notes
    );
    assert!(probe(&out).unwrap().subtitles.is_empty());
}
