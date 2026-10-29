// SPDX-License-Identifier: GPL-3.0-or-later
//! Effect stacks on clips: editing, cutting, saving and the rules that keep them valid.
use timeline_engine::*;

fn secs(s: f64) -> Time {
    Time::from_secs_f64(s)
}

struct Fx {
    ed: Editor,
    seq: SequenceId,
    v: ClipId,
    a: ClipId,
}

fn fx() -> Fx {
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
    Fx {
        ed,
        seq,
        v: p.video.unwrap(),
        a: p.audio.unwrap(),
    }
}

impl Fx {
    fn clip(&self, id: ClipId) -> Clip {
        self.ed
            .project()
            .sequence(self.seq)
            .unwrap()
            .find_clip(id)
            .unwrap()
            .1
            .clone()
    }
}

#[test]
fn effects_are_added_in_order_with_default_settings_and_unique_ids() {
    let mut f = fx();
    let b = f.ed.add_effect(f.seq, f.v, EffectKind::Blur, None).unwrap();
    let s =
        f.ed.add_effect(f.seq, f.v, EffectKind::Sepia, None)
            .unwrap();
    let c =
        f.ed.add_effect(f.seq, f.v, EffectKind::Contrast, Some(0))
            .unwrap();
    assert!(b != s && s != c && b != c);
    let kinds: Vec<EffectKind> = f.clip(f.v).effects.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [EffectKind::Contrast, EffectKind::Blur, EffectKind::Sepia]
    );
    assert_eq!(
        f.clip(f.v).effects[1].value(0),
        EffectKind::Blur.defaults()[0]
    );
    f.ed.move_effect(f.seq, f.v, c, 99).unwrap();
    assert_eq!(f.clip(f.v).effects.last().unwrap().id, c);
    f.ed.remove_effect(f.seq, f.v, s).unwrap();
    assert_eq!(f.clip(f.v).effects.len(), 2);
    // Ids are not reused while the stack is not empty.
    let n = f.ed.add_effect(f.seq, f.v, EffectKind::Hue, None).unwrap();
    assert!(f.clip(f.v).effects.iter().filter(|e| e.id == n).count() == 1);
    // Undo walks back one step at a time.
    f.ed.undo().unwrap();
    assert_eq!(f.clip(f.v).effects.len(), 2);
}

#[test]
fn sound_clips_take_no_effects_and_nothing_changes_when_refused() {
    let mut f = fx();
    let before = f.ed.project().clone();
    let e =
        f.ed.add_effect(f.seq, f.a, EffectKind::Blur, None)
            .unwrap_err();
    assert!(matches!(e, Error::NotPicture(_)), "{e}");
    assert!(f.ed.project() == &before);
    for _ in 0..MAX_EFFECTS {
        f.ed.add_effect(f.seq, f.v, EffectKind::Gamma, None)
            .unwrap();
    }
    assert!(f
        .ed
        .add_effect(f.seq, f.v, EffectKind::Gamma, None)
        .is_err());
}

#[test]
fn settings_are_checked_and_animation_locks_the_fixed_value() {
    let mut f = fx();
    let e =
        f.ed.add_effect(f.seq, f.v, EffectKind::Sharpen, None)
            .unwrap();
    f.ed.set_effect_value(f.seq, f.v, e, "radius", 1.5).unwrap();
    assert_eq!(f.clip(f.v).effects[0].value(1), 1.5);
    assert!(f
        .ed
        .set_effect_value(f.seq, f.v, e, "radius", 50.0)
        .is_err());
    assert!(f.ed.set_effect_value(f.seq, f.v, e, "nope", 1.0).is_err());
    f.ed.key_effect(f.seq, f.v, e, "amount", secs(4.0), 300.0, Interp::Smooth)
        .unwrap();
    let eff = f.clip(f.v).effects[0].clone();
    assert!(eff.is_animated(0) && !eff.is_animated(1));
    // Starts from the fixed value (100) and reaches 300 at 4 s.
    assert_eq!(eff.values_at(0)[0], 100.0);
    assert_eq!(eff.values_at(secs(4.0).ticks())[0], 300.0);
    assert!(matches!(
        f.ed.set_effect_value(f.seq, f.v, e, "amount", 10.0)
            .unwrap_err(),
        Error::Animated { .. }
    ));
    f.ed.unkey_effect(f.seq, f.v, e, "amount", secs(4.0))
        .unwrap();
    f.ed.unkey_effect(f.seq, f.v, e, "amount", secs(0.0))
        .unwrap();
    assert!(!f.clip(f.v).effects[0].is_animated(0));
    assert!(f
        .ed
        .unkey_effect(f.seq, f.v, e, "amount", secs(0.0))
        .is_err());
    // A key outside the clip is refused.
    assert!(f
        .ed
        .key_effect(f.seq, f.v, e, "amount", secs(11.0), 1.0, Interp::Linear)
        .is_err());
}

#[test]
fn cutting_and_trimming_keep_effects_and_their_curves_exact() {
    let mut f = fx();
    let e =
        f.ed.add_effect(f.seq, f.v, EffectKind::Brightness, None)
            .unwrap();
    f.ed.key_effect(f.seq, f.v, e, "amount", secs(0.0), -50.0, Interp::Smooth)
        .unwrap();
    f.ed.key_effect(f.seq, f.v, e, "amount", secs(10.0), 50.0, Interp::Smooth)
        .unwrap();
    let whole = f.clip(f.v);
    let (l, r) =
        f.ed.split_clip(f.seq, f.v, secs(4.0), LinkMode::Follow)
            .unwrap();
    let (l, r) = (f.clip(l), f.clip(r));
    assert_eq!(l.effects.len(), 1);
    assert_eq!(r.effects.len(), 1);
    // Both halves keep the same effect id (ids are per clip).
    assert_eq!(l.effects[0].id, r.effects[0].id);
    for k in 0..240 {
        let t = secs(f64::from(k) / 24.0);
        let want = whole.effect_values_at(&whole.effects[0], t)[0];
        let half = if t < secs(4.0) { &l } else { &r };
        let got = half.effect_values_at(&half.effects[0], t)[0];
        assert!(
            (want - got).abs() < 1e-6,
            "t={}: {want} vs {got}",
            t.as_secs_f64()
        );
    }
    // Editing one half leaves the other alone.
    f.ed.set_effect_enabled(f.seq, r.id, e, false).unwrap();
    assert!(f.clip(l.id).effects[0].enabled && !f.clip(r.id).effects[0].enabled);
    f.ed.trim(
        f.seq,
        r.id,
        Edge::Head,
        secs(1.0),
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    let t = f.clip(r.id);
    for k in 0..48 {
        let at = t.start + secs(f64::from(k) / 24.0);
        let want = whole.effect_values_at(&whole.effects[0], at)[0];
        assert!((want - t.effect_values_at(&t.effects[0], at)[0]).abs() < 1e-6);
    }
}

#[test]
fn live_effects_skip_disabled_and_neutral_ones() {
    let mut f = fx();
    let a =
        f.ed.add_effect(f.seq, f.v, EffectKind::Brightness, None)
            .unwrap();
    let b =
        f.ed.add_effect(f.seq, f.v, EffectKind::Invert, None)
            .unwrap();
    let clip = f.clip(f.v);
    // Brightness at its default does nothing; invert at its default does.
    let live = clip.live_effects_at(secs(1.0));
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].0, EffectKind::Invert);
    f.ed.set_effect_enabled(f.seq, f.v, b, false).unwrap();
    assert!(f.clip(f.v).live_effects_at(secs(1.0)).is_empty());
    f.ed.set_effect_value(f.seq, f.v, a, "amount", 10.0)
        .unwrap();
    assert_eq!(f.clip(f.v).live_effects_at(secs(1.0)).len(), 1);
}

#[test]
fn effects_survive_saving_and_loading() {
    let mut f = fx();
    let a = f.ed.add_effect(f.seq, f.v, EffectKind::Blur, None).unwrap();
    let b =
        f.ed.add_effect(f.seq, f.v, EffectKind::Vignette, None)
            .unwrap();
    f.ed.set_effect_value(f.seq, f.v, a, "radius", 2.75)
        .unwrap();
    f.ed.key_effect(f.seq, f.v, b, "amount", secs(3.0), 80.0, Interp::Smooth)
        .unwrap();
    f.ed.key_effect(f.seq, f.v, b, "softness", secs(5.0), 10.0, Interp::Hold)
        .unwrap();
    f.ed.set_effect_enabled(f.seq, f.v, a, false).unwrap();
    f.ed.split_clip(f.seq, f.v, secs(4.0), LinkMode::Follow)
        .unwrap();
    let text = f.ed.project().to_json().unwrap();
    let back = Project::from_json(&text).unwrap();
    assert!(&back == f.ed.project());
    assert!(text.contains("\"vignette\""));
    // No effects, nothing written.
    let plain = Editor::new("p").project().to_json().unwrap();
    assert!(!plain.contains("effects"));
}

#[test]
fn damaged_effects_are_refused_on_load() {
    let mut f = fx();
    let e = f.ed.add_effect(f.seq, f.v, EffectKind::Blur, None).unwrap();
    f.ed.set_effect_value(f.seq, f.v, e, "radius", 3.0).unwrap();
    let text = f.ed.project().to_json().unwrap();
    assert!(Project::from_json(&text).is_ok());
    assert!(Project::from_json(&text.replace("\"blur\"", "\"glitter\"")).is_err());
    assert!(Project::from_json(&text.replace("\"radius\"", "\"size\"")).is_err());
    assert!(Project::from_json(&text.replace("\"radius\": 3.0", "\"radius\": 300.0")).is_err());
    // An effect on a sound clip is refused even if hand-written into the file.
    let mut g = fx();
    let e =
        g.ed.add_effect(g.seq, g.v, EffectKind::Invert, None)
            .unwrap();
    let _ = e;
    let t = g.ed.project().to_json().unwrap();
    let a = g.a.0;
    let moved = t.replacen(&format!("\"id\": {a},"), &format!("\"id\": {a}, \"effects\": [{{\"id\": 1, \"kind\": \"invert\", \"params\": {{\"amount\": 100.0}}}}],"), 1);
    assert!(Project::from_json(&moved).is_err());
}

#[test]
fn files_from_before_effects_still_load() {
    let f = fx();
    let text = f.ed.project().to_json().unwrap().replace(
        &format!("\"version\": {}", file::FORMAT_VERSION),
        "\"version\": 4",
    );
    let p = Project::from_json(&text).unwrap();
    assert!(p
        .sequence(f.seq)
        .unwrap()
        .clips()
        .all(|(_, c)| c.effects.is_empty()));
}
