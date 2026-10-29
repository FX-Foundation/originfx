// SPDX-License-Identifier: GPL-3.0-or-later
//! Effects that come from plugins: editing, keyframing, cutting, saving and the rules.
use timeline_engine::*;

fn secs(s: f64) -> Time {
    Time::from_secs_f64(s)
}

fn spec() -> PluginSpec {
    PluginSpec {
        id: "com.example.glow".into(),
        name: "Glow".into(),
        version: "1.2.0".into(),
        settings: vec![
            Setting {
                name: "radius".into(),
                unit: "px".into(),
                default: 10.0,
                min: 0.0,
                max: 50.0,
            },
            Setting {
                name: "mix".into(),
                unit: "%".into(),
                default: 50.0,
                min: 0.0,
                max: 100.0,
            },
        ],
    }
}

fn setup() -> (Editor, SequenceId, ClipId, ClipId) {
    let mut ed = Editor::new("t");
    let seq = ed.add_sequence(SequenceSettings::default()).unwrap();
    let s = ed.project().sequence(seq).unwrap();
    let (v1, a1) = (s.video_tracks()[0].id, s.audio_tracks()[0].id);
    let asset = ed
        .add_asset(NewAsset {
            name: "av".into(),
            path: "/av.mp4".into(),
            duration: Some(secs(60.0)),
            video: vec![VideoProps {
                width: 1920,
                height: 1080,
                frame_rate: Some(FrameRate::FPS_24),
            }],
            audio: vec![AudioProps {
                sample_rate: 48000,
                channels: 2,
            }],
        })
        .unwrap();
    let p = ed
        .place_asset(
            seq,
            asset,
            Time::ZERO,
            Time::ZERO,
            Some(secs(10.0)),
            Some(v1),
            Some(a1),
            PlaceMode::Overwrite,
        )
        .unwrap();
    (ed, seq, p.video.unwrap(), p.audio.unwrap())
}

fn clip(ed: &Editor, seq: SequenceId, id: ClipId) -> Clip {
    ed.project()
        .sequence(seq)
        .unwrap()
        .find_clip(id)
        .unwrap()
        .1
        .clone()
}

#[test]
fn a_plugin_effect_has_the_settings_its_plugin_declared() {
    let (mut ed, seq, v, a) = setup();
    let fx = ed.add_plugin_effect(seq, v, spec(), None).unwrap();
    let e = clip(&ed, seq, v).effects[0].clone();
    assert_eq!(e.label(), "plugin:com.example.glow");
    assert_eq!(e.settings().len(), 2);
    assert_eq!((e.value(0), e.value(1)), (10.0, 50.0));
    // A plugin effect is always live (the plugin decides what it does).
    assert!(matches!(
        clip(&ed, seq, v).live_effects_at(secs(1.0))[0],
        LiveEffect::Plugin(..)
    ));
    // Built-in and plugin effects share one stack, one set of edits.
    let b = ed.add_effect(seq, v, EffectKind::Blur, Some(0)).unwrap();
    ed.set_effect_value(seq, v, fx, "radius", 25.0).unwrap();
    assert!(ed.set_effect_value(seq, v, fx, "radius", 51.0).is_err());
    assert!(ed.set_effect_value(seq, v, fx, "nope", 1.0).is_err());
    assert!(ed.set_effect_value(seq, v, fx, "amount", 1.0).is_err());
    ed.move_effect(seq, v, fx, 0).unwrap();
    assert_eq!(clip(&ed, seq, v).effects[0].id, fx);
    ed.set_effect_enabled(seq, v, fx, false).unwrap();
    assert_eq!(clip(&ed, seq, v).live_effects_at(secs(1.0)).len(), 1);
    ed.remove_effect(seq, v, b).unwrap();
    // Sound clips cannot take picture effects, plugin or not.
    assert!(ed.add_plugin_effect(seq, a, spec(), None).is_err());
}

#[test]
fn plugin_settings_can_be_keyframed_and_cut_exactly() {
    let (mut ed, seq, v, _a) = setup();
    let fx = ed.add_plugin_effect(seq, v, spec(), None).unwrap();
    ed.key_effect(seq, v, fx, "mix", secs(0.0), 0.0, Interp::Linear)
        .unwrap();
    ed.key_effect(seq, v, fx, "mix", secs(10.0), 100.0, Interp::Linear)
        .unwrap();
    assert!(ed.set_effect_value(seq, v, fx, "mix", 5.0).is_err());
    assert!(ed
        .key_effect(seq, v, fx, "mix", secs(5.0), 101.0, Interp::Linear)
        .is_err());
    let (l, r) = ed.split_clip(seq, v, secs(4.0), LinkMode::Follow).unwrap();
    let at = |id, t: f64| {
        let c = clip(&ed, seq, id);
        match &c.live_effects_at(secs(t))[0] {
            LiveEffect::Plugin(_, v) => v[1],
            _ => panic!(),
        }
    };
    assert!((at(l, 2.0) - 20.0).abs() < 1e-9);
    assert!((at(r, 4.0) - 40.0).abs() < 1e-9);
    assert!((at(r, 10.0) - 100.0).abs() < 1e-9);
    ed.unkey_effect(seq, r, fx, "mix", secs(4.0)).unwrap();
    ed.clear_effect_animation(seq, l, fx, "mix").unwrap();
    ed.project().validate().unwrap();
}

#[test]
fn bad_plugin_descriptions_are_refused() {
    let (mut ed, seq, v, _a) = setup();
    let mut s = spec();
    s.id = "has space".into();
    assert!(ed.add_plugin_effect(seq, v, s, None).is_err());
    let mut s = spec();
    s.settings[0].default = 99.0;
    assert!(ed.add_plugin_effect(seq, v, s, None).is_err());
    let mut s = spec();
    s.settings[1].name = "radius".into();
    assert!(ed.add_plugin_effect(seq, v, s, None).is_err());
    let mut s = spec();
    s.settings[0].max = f64::NAN;
    assert!(ed.add_plugin_effect(seq, v, s, None).is_err());
    let mut s = spec();
    s.settings = (0..17)
        .map(|i| Setting {
            name: format!("s{i}"),
            unit: String::new(),
            default: 0.0,
            min: 0.0,
            max: 1.0,
        })
        .collect();
    assert!(ed.add_plugin_effect(seq, v, s, None).is_err());
    assert!(clip(&ed, seq, v).effects.is_empty());
}

#[test]
fn plugin_effects_save_exactly_and_old_files_open() {
    let (mut ed, seq, v, _a) = setup();
    let fx = ed.add_plugin_effect(seq, v, spec(), None).unwrap();
    ed.add_effect(seq, v, EffectKind::Sepia, None).unwrap();
    ed.set_effect_value(seq, v, fx, "radius", 12.5).unwrap();
    ed.key_effect(seq, v, fx, "mix", secs(2.0), 70.0, Interp::Smooth)
        .unwrap();
    let json = ed.project().to_json().unwrap();
    let back = Project::from_json(&json).unwrap();
    assert_eq!(&back, ed.project());
    assert_eq!(back.to_json().unwrap(), json);
    // A project with only built-in effects writes no plugin fields, and a version 7 file opens.
    let (mut plain, pseq, pv, _) = setup();
    plain.add_effect(pseq, pv, EffectKind::Blur, None).unwrap();
    let pj = plain.project().to_json().unwrap();
    assert!(!pj.contains("\"plugin\""));
    let v7 = pj.replace(
        &format!("\"version\": {}", file::FORMAT_VERSION),
        "\"version\": 7",
    );
    assert_eq!(&Project::from_json(&v7).unwrap(), plain.project());
    // A damaged plugin description in a file is refused.
    let bad = json.replace("\"max\": 50.0", "\"max\": -1.0");
    assert!(Project::from_json(&bad).is_err());
    let bad = json.replace("\"radius\": 12.5", "\"radius\": 500.0");
    assert!(Project::from_json(&bad).is_err());
}
