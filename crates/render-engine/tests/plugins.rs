// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugin effects in rendered frames, using the example plugin (mixes with the negative).
use std::sync::Arc;

use media_engine::VideoFrame;
use plugin_engine::{Host, Plugin};
use render_engine::{MediaSource, MissingMedia, RenderSettings, Renderer, Result};
use timeline_engine::{
    Asset, ClipId, ClipSpec, Editor, EffectKind, FrameRate, Interp, NewAsset, PlaceMode,
    PluginSpec, SequenceId, SequenceSettings, Setting, Time, VideoProps,
};

struct Grey(u8);

impl MediaSource for Grey {
    fn video_frame(&mut self, _: &Asset, _: u32, secs: f64) -> Result<Arc<VideoFrame>> {
        Ok(Arc::new(VideoFrame {
            width: 32,
            height: 18,
            pts_secs: secs,
            data: [self.0, self.0, self.0, 255]
                .iter()
                .copied()
                .cycle()
                .take(32 * 18 * 4)
                .collect(),
        }))
    }

    fn audio(&mut self, _: &Asset, _: u32, _: f64, frames: usize, _: u32) -> Result<Vec<f32>> {
        Ok(vec![0.0; frames * 2])
    }
}

fn secs(s: f64) -> Time {
    Time::from_secs_f64(s)
}

fn world() -> (Editor, SequenceId, ClipId) {
    let mut ed = Editor::new("t");
    let seq = ed
        .add_sequence(SequenceSettings {
            width: 32,
            height: 18,
            ..Default::default()
        })
        .unwrap();
    let asset = ed
        .add_asset(NewAsset {
            name: "x".into(),
            path: "/x.mp4".into(),
            duration: Some(secs(30.0)),
            video: vec![VideoProps {
                width: 32,
                height: 18,
                frame_rate: Some(FrameRate::FPS_24),
            }],
            audio: vec![],
        })
        .unwrap();
    let v1 = ed.project().sequence(seq).unwrap().video_tracks()[0].id;
    let mut s = ClipSpec::new(asset, secs(0.0));
    s.duration = Some(secs(4.0));
    let c = ed.place_clip(seq, v1, s, PlaceMode::Overwrite).unwrap();
    (ed, seq, c)
}

fn host() -> Arc<Host> {
    let mut h = Host::new();
    h.add(
        "invert-mix.wasm",
        Plugin::load(&plugin_engine::example_invert_mix_wasm()).unwrap(),
    );
    Arc::new(h)
}

fn spec() -> PluginSpec {
    let m = host()
        .get(plugin_engine::EXAMPLE_ID)
        .unwrap()
        .manifest()
        .clone();
    PluginSpec {
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
    }
}

fn mid(
    ed: &Editor,
    seq: SequenceId,
    settings: RenderSettings,
    h: Option<Arc<Host>>,
    frame: i64,
) -> Result<[u8; 4]> {
    let mut r = Renderer::new(ed.project(), seq, Grey(40), settings)?;
    if let Some(h) = h {
        r = r.with_plugins(h);
    }
    let f = r.frame(frame)?;
    let i = (9 * 32 + 16) * 4;
    Ok([f.data[i], f.data[i + 1], f.data[i + 2], f.data[i + 3]])
}

#[test]
fn a_plugin_effect_changes_the_frame_and_follows_its_settings_and_keys() {
    let (mut ed, seq, c) = world();
    let d = RenderSettings::default();
    assert_eq!(mid(&ed, seq, d, None, 0).unwrap(), [40, 40, 40, 255]);
    let fx = ed.add_plugin_effect(seq, c, spec(), None).unwrap();
    // Full negative of 40 is 215.
    assert_eq!(
        mid(&ed, seq, d, Some(host()), 0).unwrap(),
        [215, 215, 215, 255]
    );
    ed.set_effect_value(seq, c, fx, "amount", 0.0).unwrap();
    assert_eq!(
        mid(&ed, seq, d, Some(host()), 0).unwrap(),
        [40, 40, 40, 255]
    );
}

#[test]
fn a_keyed_plugin_setting_moves_over_time() {
    let (mut ed, seq, c) = world();
    let fx = ed.add_plugin_effect(seq, c, spec(), None).unwrap();
    ed.key_effect(seq, c, fx, "amount", secs(0.0), 0.0, Interp::Linear)
        .unwrap();
    ed.key_effect(seq, c, fx, "amount", secs(2.0), 100.0, Interp::Linear)
        .unwrap();
    let d = RenderSettings::default();
    let a = mid(&ed, seq, d, Some(host()), 0).unwrap()[0];
    let b = mid(&ed, seq, d, Some(host()), 24).unwrap()[0];
    let e = mid(&ed, seq, d, Some(host()), 48).unwrap()[0];
    assert_eq!(a, 40);
    assert!((i32::from(b) - 128).abs() <= 1, "{b}");
    assert_eq!(e, 215);
}

#[test]
fn plugin_and_built_in_effects_run_in_stack_order() {
    let (mut ed, seq, c) = world();
    let p = ed.add_plugin_effect(seq, c, spec(), None).unwrap();
    let b = ed.add_effect(seq, c, EffectKind::Brightness, None).unwrap();
    ed.set_effect_value(seq, c, b, "amount", 20.0).unwrap();
    let d = RenderSettings::default();
    // Invert 40 -> 215, then +20 % brightness (+51) -> 255 (clipped).
    assert_eq!(mid(&ed, seq, d, Some(host()), 0).unwrap()[0], 255);
    ed.move_effect(seq, c, p, 1).unwrap();
    // Brighten 40 -> 91, then invert -> 164.
    assert_eq!(mid(&ed, seq, d, Some(host()), 0).unwrap()[0], 164);
}

#[test]
fn a_missing_plugin_stops_the_render_clearly_or_is_skipped_offline() {
    let (mut ed, seq, c) = world();
    ed.add_plugin_effect(seq, c, spec(), None).unwrap();
    let d = RenderSettings::default();
    for h in [None, Some(Arc::new(Host::new()))] {
        let e = mid(&ed, seq, d, h, 0).unwrap_err();
        assert!(e.to_string().contains("not installed"), "{e}");
    }
    let offline = RenderSettings {
        missing_media: MissingMedia::Offline,
        ..d
    };
    assert_eq!(mid(&ed, seq, offline, None, 0).unwrap(), [40, 40, 40, 255]);
}

#[test]
fn a_plugin_whose_settings_changed_is_reported() {
    let (mut ed, seq, c) = world();
    let mut s = spec();
    s.settings.push(Setting {
        name: "extra".into(),
        unit: String::new(),
        default: 0.0,
        min: 0.0,
        max: 1.0,
    });
    ed.add_plugin_effect(seq, c, s, None).unwrap();
    let e = mid(&ed, seq, RenderSettings::default(), Some(host()), 0).unwrap_err();
    assert!(e.to_string().contains("settings"), "{e}");
}
