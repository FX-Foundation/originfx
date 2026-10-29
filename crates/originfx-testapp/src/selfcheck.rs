// SPDX-License-Identifier: GPL-3.0-or-later
//! The self-check: a battery of small tests of every engine, run on this machine, ending in a
//! plain-text report that can be copied and pasted into a bug report.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use media_engine::{
    hwaccel, probe, DecodeOptions, ExportPreset, ExportSettings, HwPolicy, Quality, VideoFrame,
};
use plugin_engine::{Host, Plugin};
use render_engine::{
    export_sequence, ExportOptions, FileSource, MediaSource, RenderSettings, Renderer,
};
use timeline_engine::{
    Asset, BlendMode, ClipId, ClipSpec, Colour, Editor, EffectKind, Fill, FrameRate, Generator,
    Interp, LinkMode, MaskShape, NewAsset, Param, PlaceMode, PluginSpec, Project, SequenceId,
    SequenceSettings, Setting, Shape, ShapeKind, Text, Time, VideoProps,
};

/// One flat grey for every picture, so results can be checked to the exact pixel.
struct Grey(u8);

impl MediaSource for Grey {
    fn video_frame(
        &mut self,
        _: &Asset,
        _: u32,
        secs: f64,
    ) -> render_engine::Result<Arc<VideoFrame>> {
        Ok(Arc::new(VideoFrame {
            width: 64,
            height: 36,
            pts_secs: secs,
            data: [self.0, self.0, self.0, 255]
                .iter()
                .copied()
                .cycle()
                .take(64 * 36 * 4)
                .collect(),
        }))
    }

    fn audio(
        &mut self,
        _: &Asset,
        _: u32,
        _: f64,
        frames: usize,
        _: u32,
    ) -> render_engine::Result<Vec<f32>> {
        Ok(vec![0.0; frames * 2])
    }
}

fn secs(s: f64) -> Time {
    Time::from_secs_f64(s)
}

struct W {
    ed: Editor,
    seq: SequenceId,
    clip: ClipId,
}

fn world() -> Result<W, String> {
    let mut ed = Editor::new("selfcheck");
    let seq = ed
        .add_sequence(SequenceSettings {
            width: 64,
            height: 36,
            ..Default::default()
        })
        .map_err(|e| e.to_string())?;
    let asset = ed
        .add_asset(NewAsset {
            name: "grey".into(),
            path: "/grey.mp4".into(),
            duration: Some(secs(30.0)),
            video: vec![VideoProps {
                width: 64,
                height: 36,
                frame_rate: Some(FrameRate::FPS_24),
            }],
            audio: vec![],
        })
        .map_err(|e| e.to_string())?;
    let v1 = ed
        .project()
        .sequence(seq)
        .ok_or("no sequence")?
        .video_tracks()[0]
        .id;
    let mut s = ClipSpec::new(asset, secs(0.0));
    s.duration = Some(secs(4.0));
    let clip = ed
        .place_clip(seq, v1, s, PlaceMode::Overwrite)
        .map_err(|e| e.to_string())?;
    Ok(W { ed, seq, clip })
}

/// The middle pixel of a frame of the project, drawn from a flat grey source.
fn mid(w: &W, grey: u8, frame: i64, plugins: Option<Arc<Host>>) -> Result<[u8; 4], String> {
    let mut r = Renderer::new(w.ed.project(), w.seq, Grey(grey), RenderSettings::default())
        .map_err(|e| e.to_string())?;
    if let Some(h) = plugins {
        r = r.with_plugins(h);
    }
    let f = r.frame(frame).map_err(|e| e.to_string())?;
    let i = (18 * 64 + 32) * 4;
    Ok([f.data[i], f.data[i + 1], f.data[i + 2], f.data[i + 3]])
}

fn expect(what: &str, got: [u8; 4], want: u8, slack: i32) -> Result<(), String> {
    if (i32::from(got[0]) - i32::from(want)).abs() <= slack {
        Ok(())
    } else {
        Err(format!("{what}: expected about {want}, got {}", got[0]))
    }
}

fn check_timeline() -> Result<String, String> {
    let mut w = world()?;
    let (l, r) =
        w.ed.split_clip(w.seq, w.clip, secs(2.0), LinkMode::Follow)
            .map_err(|e| e.to_string())?;
    w.ed.undo().map_err(|e| e.to_string())?;
    w.ed.redo().map_err(|e| e.to_string())?;
    let dir = std::env::temp_dir().join(format!("originfx-selfcheck-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join("check.originfx");
    w.ed.project().save(&file).map_err(|e| e.to_string())?;
    let back = Project::load(&file).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&dir);
    if &back != w.ed.project() {
        return Err("a saved and reloaded project is not identical".into());
    }
    Ok(format!(
        "split into {l} and {r}, undo/redo, save and reload identical"
    ))
}

fn check_effects() -> Result<String, String> {
    let mut w = world()?;
    let plain = mid(&w, 100, 0, None)?;
    expect("plain", plain, 100, 0)?;
    let fx =
        w.ed.add_effect(w.seq, w.clip, EffectKind::Brightness, None)
            .map_err(|e| e.to_string())?;
    w.ed.set_effect_value(w.seq, w.clip, fx, "amount", 20.0)
        .map_err(|e| e.to_string())?;
    expect("brightness +20%", mid(&w, 100, 0, None)?, 151, 1)?;
    w.ed.add_effect(w.seq, w.clip, EffectKind::Invert, None)
        .map_err(|e| e.to_string())?;
    expect("then invert", mid(&w, 100, 0, None)?, 104, 1)?;
    Ok("brightness and invert stack in order".into())
}

fn check_keyframes() -> Result<String, String> {
    let mut w = world()?;
    let t = timeline_engine::Target::Clip(w.clip);
    for (at, v) in [(0.0, 0.0), (2.0, 100.0)] {
        w.ed.set_keyframe(
            w.seq,
            t,
            Param::Opacity,
            secs(at),
            v,
            Interp::Linear,
            LinkMode::Follow,
        )
        .map_err(|e| e.to_string())?;
    }
    expect("start", mid(&w, 200, 0, None)?, 0, 0)?;
    expect("halfway", mid(&w, 200, 24, None)?, 100, 2)?;
    expect("end", mid(&w, 200, 48, None)?, 200, 0)?;
    Ok("an opacity fade runs 0, 100, 200 as expected".into())
}

fn check_compositing() -> Result<String, String> {
    let mut w = world()?;
    w.ed.set_look_value(w.seq, w.clip, Param::PositionX, 100.0)
        .map_err(|e| e.to_string())?;
    expect("moved off screen", mid(&w, 200, 0, None)?, 0, 0)?;
    w.ed.set_look_value(w.seq, w.clip, Param::PositionX, 0.0)
        .map_err(|e| e.to_string())?;
    w.ed.set_mask(w.seq, w.clip, MaskShape::Ellipse, true)
        .map_err(|e| e.to_string())?;
    expect(
        "inverted mask hides the middle",
        mid(&w, 200, 0, None)?,
        0,
        0,
    )?;
    w.ed.remove_mask(w.seq, w.clip).map_err(|e| e.to_string())?;
    // A second layer that multiplies with the first.
    w.ed.add_track(w.seq, timeline_engine::TrackKind::Video, None)
        .map_err(|e| e.to_string())?;
    let v2 =
        w.ed.project()
            .sequence(w.seq)
            .ok_or("no sequence")?
            .video_tracks()[1]
            .id;
    let asset =
        w.ed.project()
            .sequence(w.seq)
            .ok_or("no sequence")?
            .video_tracks()[0]
            .clips()
            .next()
            .map(|c| c.asset)
            .ok_or("no clip")?;
    let mut s = ClipSpec::new(asset, secs(0.0));
    s.duration = Some(secs(4.0));
    let top =
        w.ed.place_clip(w.seq, v2, s, PlaceMode::Overwrite)
            .map_err(|e| e.to_string())?;
    w.ed.set_blend_mode(w.seq, top, BlendMode::Difference)
        .map_err(|e| e.to_string())?;
    expect("difference of equal layers", mid(&w, 150, 0, None)?, 0, 0)?;
    w.ed.set_blend_mode(w.seq, top, BlendMode::Multiply)
        .map_err(|e| e.to_string())?;
    expect("multiply 150 x 150", mid(&w, 150, 0, None)?, 88, 1)?;
    Ok("move, inverted mask, difference and multiply blends are right".into())
}

fn check_graphics() -> Result<String, String> {
    let mut w = world()?;
    let a =
        w.ed.add_generated(
            "blue box",
            Generator::Shape(Shape {
                kind: ShapeKind::Rect { corner_pct: 0.0 },
                width_pct: 50.0,
                height_pct: 50.0,
                fill: Fill::Solid(Colour::rgb(0, 0, 255)),
                stroke: None,
            }),
        )
        .map_err(|e| e.to_string())?;
    w.ed.add_track(w.seq, timeline_engine::TrackKind::Video, None)
        .map_err(|e| e.to_string())?;
    let v2 =
        w.ed.project()
            .sequence(w.seq)
            .ok_or("no sequence")?
            .video_tracks()[1]
            .id;
    let mut s = ClipSpec::new(a, secs(0.0));
    s.duration = Some(secs(4.0));
    w.ed.place_clip(w.seq, v2, s, PlaceMode::Overwrite)
        .map_err(|e| e.to_string())?;
    let p = mid(&w, 100, 0, None)?;
    if p != [0, 0, 255, 255] {
        return Err(format!("the box should be blue in the middle, got {p:?}"));
    }
    let font = motion_graphics_engine_default_font();
    Ok(format!(
        "a shape draws; text font found: {}",
        font.unwrap_or_else(|| "NONE (text needs --font)".into())
    ))
}

fn motion_graphics_engine_default_font() -> Option<String> {
    // The same list the engine uses when no font is named.
    let spec = Generator::Text(Text {
        text: "A".into(),
        font: None,
        size_pct: 50.0,
        colour: Colour::WHITE,
        align: timeline_engine::Align::Centre,
        line_gap_pct: 0.0,
        tracking_pct: 0.0,
        outline: None,
        shadow: None,
    });
    let mut w = world().ok()?;
    let a = w.ed.add_generated("t", spec).ok()?;
    w.ed.add_track(w.seq, timeline_engine::TrackKind::Video, None)
        .ok()?;
    let v2 = w.ed.project().sequence(w.seq)?.video_tracks()[1].id;
    let mut s = ClipSpec::new(a, secs(0.0));
    s.duration = Some(secs(1.0));
    w.ed.place_clip(w.seq, v2, s, PlaceMode::Overwrite).ok()?;
    let mut r = Renderer::new(w.ed.project(), w.seq, Grey(0), RenderSettings::default()).ok()?;
    r.frame(0).ok().map(|_| "found and drew text".to_string())
}

fn example_spec(host: &Host) -> Result<PluginSpec, String> {
    let m = host
        .get(plugin_engine::EXAMPLE_ID)
        .map_err(|e| e.to_string())?
        .manifest()
        .clone();
    Ok(PluginSpec {
        id: m.id,
        name: m.name,
        version: m.version,
        settings: m
            .params
            .iter()
            .map(|d| Setting {
                name: d.name.clone(),
                unit: d.unit.clone(),
                default: d.default,
                min: d.min,
                max: d.max,
            })
            .collect(),
    })
}

fn check_plugins() -> Result<String, String> {
    let mut host = Host::new();
    host.add(
        "invert-mix.wasm",
        Plugin::load(&plugin_engine::example_invert_mix_wasm()).map_err(|e| e.to_string())?,
    );
    let host = Arc::new(host);
    let mut w = world()?;
    w.ed.add_plugin_effect(w.seq, w.clip, example_spec(&host)?, None)
        .map_err(|e| e.to_string())?;
    let t = Instant::now();
    expect(
        "plugin inverts 40",
        mid(&w, 40, 0, Some(Arc::clone(&host)))?,
        215,
        0,
    )?;
    // A plugin that is not installed must be reported, not drawn wrongly.
    match mid(&w, 40, 0, None) {
        Err(m) if m.contains("not installed") => {}
        other => {
            return Err(format!(
                "a missing plugin should be reported, got {other:?}"
            ))
        }
    }
    Ok(format!(
        "example plugin ran in the sandbox ({:.0} ms)",
        t.elapsed().as_secs_f64() * 1000.0
    ))
}

fn check_export() -> Result<String, String> {
    // A project made only of drawn graphics, so it needs no media files.
    let mut ed = Editor::new("export check");
    let seq = ed
        .add_sequence(SequenceSettings {
            width: 640,
            height: 360,
            ..Default::default()
        })
        .map_err(|e| e.to_string())?;
    let bg = ed
        .add_generated(
            "bg",
            Generator::Fill(Fill::Linear {
                from: Colour::rgb(20, 20, 60),
                to: Colour::rgb(190, 60, 40),
                angle_deg: 45.0,
            }),
        )
        .map_err(|e| e.to_string())?;
    let v1 = ed
        .project()
        .sequence(seq)
        .ok_or("no sequence")?
        .video_tracks()[0]
        .id;
    let mut s = ClipSpec::new(bg, secs(0.0));
    s.duration = Some(secs(2.0));
    ed.place_clip(seq, v1, s, PlaceMode::Overwrite)
        .map_err(|e| e.to_string())?;
    let dir = std::env::temp_dir().join(format!("originfx-selfcheck-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = dir.join("check.mp4");
    let settings = ExportSettings::new(ExportPreset::Mp4H264).quality(Quality::Draft);
    let t = Instant::now();
    let report = export_sequence(
        ed.project(),
        seq,
        &out,
        &settings,
        &ExportOptions::default(),
        |_| true,
    )
    .map_err(|e| e.to_string());
    let result = report.and_then(|r| {
        let info = probe(&out).map_err(|e| e.to_string())?;
        let v = info
            .primary_video()
            .ok_or("the exported file has no picture")?;
        if (v.width, v.height) != (640, 360) {
            return Err(format!("exported size {}x{}", v.width, v.height));
        }
        Ok(format!(
            "exported 2 s of 640x360 H.264 MP4 ({} frames, {:.1} s)",
            r.frames,
            t.elapsed().as_secs_f64()
        ))
    });
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn check_hardware() -> Result<String, String> {
    let mut lines = Vec::new();
    let backends = hwaccel::probe_backends();
    if backends.is_empty() {
        lines.push("no hardware backends in this FFmpeg".to_string());
    }
    for b in &backends {
        match &b.error {
            None => lines.push(format!("    [usable] {} ({})", b.name, b.vendor)),
            Some(e) => lines.push(format!("    [no]     {} ({}) -- {e}", b.name, b.vendor)),
        }
    }
    let codecs = hwaccel::hardware_codecs();
    lines.push(format!(
        "    hardware codecs in this FFmpeg: {}",
        if codecs.is_empty() {
            "none".to_string()
        } else {
            codecs.join(", ")
        }
    ));
    Ok(format!("information only\n{}", lines.join("\n")))
}

fn time_frames(ed: &Editor, seq: SequenceId, hw: HwPolicy) -> Result<(f64, f64, u32, u32), String> {
    let source = FileSource::new().with_decode(DecodeOptions { hw });
    let mut r = Renderer::new(ed.project(), seq, source, RenderSettings::default())
        .map_err(|e| e.to_string())?;
    let t = Instant::now();
    let first = r.frame(0).map_err(|e| e.to_string())?;
    let first_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let n = 24;
    for i in 1..=n {
        r.frame(i).map_err(|e| e.to_string())?;
    }
    let per = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
    Ok((first_ms, per, first.width, first.height))
}

fn check_clip(path: &Path) -> Result<String, String> {
    let info = probe(path).map_err(|e| e.to_string())?;
    let v = info.primary_video().ok_or("the file has no picture")?;
    let asset = timeline_engine::media::probe_asset(path).map_err(|e| e.to_string())?;
    let has_audio = !asset.audio.is_empty();
    let k = (1920.0 / f64::from(v.width))
        .min(1920.0 / f64::from(v.height))
        .min(1.0);
    let even = |n: u32| ((f64::from(n) * k / 2.0).round() as u32 * 2).max(2);
    let mut ed = Editor::new("clip check");
    let seq = ed
        .add_sequence(SequenceSettings {
            width: even(v.width),
            height: even(v.height),
            ..Default::default()
        })
        .map_err(|e| e.to_string())?;
    let id = ed.add_asset(asset).map_err(|e| e.to_string())?;
    let s = ed.project().sequence(seq).ok_or("no sequence")?;
    let v1 = s.video_tracks()[0].id;
    let a1 = s.audio_tracks().front().map(|t| t.id).filter(|_| has_audio);
    ed.place_asset(
        seq,
        id,
        secs(0.0),
        secs(0.0),
        Some(secs(3.0)),
        Some(v1),
        a1,
        PlaceMode::Overwrite,
    )
    .map_err(|e| e.to_string())?;
    let name = path
        .file_name()
        .map_or(String::new(), |n| n.to_string_lossy().into());
    let mut out = format!(
        "{name} {}x{} {} {}",
        v.width,
        v.height,
        v.codec,
        v.frame_rate
            .map_or(String::new(), |r| format!("{:.2} fps", r.as_f64()))
    );
    let (first, per, w, h) = time_frames(&ed, seq, HwPolicy::Off)?;
    out.push_str(&format!(
        "\n    CPU decode: first frame {first:.0} ms, then {per:.1} ms/frame ({:.0} fps) at {w}x{h}",
        1000.0 / per.max(0.001)
    ));
    match time_frames(&ed, seq, HwPolicy::Auto) {
        Ok((first, per, _, _)) => out.push_str(&format!(
            "\n    GPU decode: first frame {first:.0} ms, then {per:.1} ms/frame ({:.0} fps)",
            1000.0 / per.max(0.001)
        )),
        Err(e) => out.push_str(&format!("\n    GPU decode: failed: {e}")),
    }
    if has_audio {
        let mut r = Renderer::new(
            ed.project(),
            seq,
            FileSource::new(),
            RenderSettings::default(),
        )
        .map_err(|e| e.to_string())?;
        let t = Instant::now();
        let samples = r.audio_samples(0, 48_000).map_err(|e| e.to_string())?;
        let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
        out.push_str(&format!(
            "\n    Sound: 1 s of audio mixed in {:.0} ms, loudest sample {peak:.2}{}",
            t.elapsed().as_secs_f64() * 1000.0,
            if peak < 0.001 {
                "  (SILENT - no sound reached the mix)"
            } else {
                ""
            }
        ));
    } else {
        out.push_str("\n    Sound: this file has no audio track");
    }
    Ok(out)
}

type Check = (&'static str, fn() -> Result<String, String>);

/// Run every check, sending each result line to `say` as it finishes, and return the full report.
pub fn run(sample: Option<PathBuf>, say: impl Fn(&str)) -> String {
    let mut report = String::new();
    let mut put = |line: String| {
        say(&line);
        report.push_str(&line);
        report.push('\n');
    };
    put(format!(
        "OriginFX self-check  (version {}, {} / {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    let checks: [Check; 8] = [
        ("Timeline", check_timeline),
        ("Effects", check_effects),
        ("Keyframes", check_keyframes),
        ("Compositing", check_compositing),
        ("Graphics", check_graphics),
        ("Plugins", check_plugins),
        ("Export (FFmpeg)", check_export),
        ("Hardware", check_hardware),
    ];
    let (mut passed, mut failed) = (0, 0);
    for (name, f) in checks {
        let t = Instant::now();
        let result = std::panic::catch_unwind(f)
            .unwrap_or_else(|_| Err("the check crashed (panic)".to_string()));
        match result {
            Ok(m) => {
                passed += 1;
                put(format!(
                    "PASS  {name:<16} {m}  [{:.2}s]",
                    t.elapsed().as_secs_f64()
                ));
            }
            Err(m) => {
                failed += 1;
                put(format!(
                    "FAIL  {name:<16} {m}  [{:.2}s]",
                    t.elapsed().as_secs_f64()
                ));
            }
        }
    }
    if let Some(p) = sample {
        let t = Instant::now();
        match check_clip(&p) {
            Ok(m) => {
                passed += 1;
                put(format!(
                    "PASS  {:<16} {m}  [{:.2}s]",
                    "Your clip",
                    t.elapsed().as_secs_f64()
                ));
            }
            Err(m) => {
                failed += 1;
                put(format!("FAIL  {:<16} {m}", "Your clip"));
            }
        }
    }
    put(format!("{passed} passed, {failed} failed"));
    report
}
