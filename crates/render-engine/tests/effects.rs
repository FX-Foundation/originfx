// SPDX-License-Identifier: GPL-3.0-or-later
//! Effects on clips, checked on exact pixels with a source that gives one flat colour.
use std::sync::Arc;

use media_engine::VideoFrame;
use render_engine::{MediaSource, RenderSettings, Renderer, Result};
use timeline_engine::{
    Asset, AssetId, ClipId, ClipSpec, Editor, EffectKind, FrameRate, Interp, LinkMode, NewAsset,
    PlaceMode, SequenceId, SequenceSettings, Time, VideoProps,
};

/// Every picture is one flat grey.
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

struct W {
    ed: Editor,
    seq: SequenceId,
    asset: AssetId,
}

fn world() -> (W, ClipId) {
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
    let mut w = W { ed, seq, asset };
    let c = w.clip(0.0, 4.0);
    (w, c)
}

impl W {
    fn clip(&mut self, start: f64, dur: f64) -> ClipId {
        let mut s = ClipSpec::new(self.asset, secs(start));
        s.duration = Some(secs(dur));
        let v1 = self.ed.project().sequence(self.seq).unwrap().video_tracks()[0].id;
        self.ed
            .place_clip(self.seq, v1, s, PlaceMode::Overwrite)
            .unwrap()
    }

    fn px(&self, grey: u8, frame: i64) -> [u8; 4] {
        let mut r = Renderer::new(
            self.ed.project(),
            self.seq,
            Grey(grey),
            RenderSettings::default(),
        )
        .unwrap();
        let f = r.frame(frame).unwrap();
        // The middle pixel (effects like a vignette leave the centre alone).
        let i = (9 * 32 + 16) * 4;
        [f.data[i], f.data[i + 1], f.data[i + 2], f.data[i + 3]]
    }

    fn whole(&self, grey: u8, frame: i64) -> Vec<u8> {
        Renderer::new(
            self.ed.project(),
            self.seq,
            Grey(grey),
            RenderSettings::default(),
        )
        .unwrap()
        .frame(frame)
        .unwrap()
        .data
    }
}

#[test]
fn no_effects_no_change_and_neutral_effects_no_change() {
    let (mut w, c) = world();
    let plain = w.whole(100, 3);
    let fx =
        w.ed.add_effect(w.seq, c, EffectKind::Brightness, None)
            .unwrap();
    // Default brightness is neutral.
    assert_eq!(w.whole(100, 3), plain);
    w.ed.set_effect_value(w.seq, c, fx, "amount", 20.0).unwrap();
    assert_eq!(w.px(100, 3), [151, 151, 151, 255]);
    // Disabled: back to plain. Enabled again: changed. Removed: plain.
    w.ed.set_effect_enabled(w.seq, c, fx, false).unwrap();
    assert_eq!(w.whole(100, 3), plain);
    w.ed.set_effect_enabled(w.seq, c, fx, true).unwrap();
    assert_eq!(w.px(100, 3), [151, 151, 151, 255]);
    w.ed.remove_effect(w.seq, c, fx).unwrap();
    assert_eq!(w.whole(100, 3), plain);
    // Undo brings it back.
    w.ed.undo().unwrap();
    assert_eq!(w.px(100, 3), [151, 151, 151, 255]);
}

#[test]
fn the_stack_is_applied_in_order() {
    let (mut w, c) = world();
    let b =
        w.ed.add_effect(w.seq, c, EffectKind::Brightness, None)
            .unwrap();
    w.ed.set_effect_value(w.seq, c, b, "amount", 50.0).unwrap();
    let i = w.ed.add_effect(w.seq, c, EffectKind::Invert, None).unwrap();
    // Brighten (100 -> 228), then invert (-> 27).
    let first = w.px(100, 0);
    assert_eq!(first, [27, 27, 27, 255]);
    // Invert first (155), then brighten (-> 255, clipped).
    w.ed.move_effect(w.seq, c, i, 0).unwrap();
    let second = w.px(100, 0);
    assert_eq!(second, [255, 255, 255, 255]);
    assert_ne!(first, second);
}

#[test]
fn animated_settings_change_from_frame_to_frame() {
    let (mut w, c) = world();
    let b =
        w.ed.add_effect(w.seq, c, EffectKind::Brightness, None)
            .unwrap();
    // Brightness 0 at the start of the clip, +40 at 2 s, held after.
    w.ed.key_effect(w.seq, c, b, "amount", secs(2.0), 40.0, Interp::Linear)
        .unwrap();
    assert_eq!(w.px(100, 0), [100, 100, 100, 255]);
    let mid = w.px(100, 24)[0]; // 1 s: +20 %
    assert_eq!(mid, 151);
    assert_eq!(w.px(100, 48)[0], 202); // +40 %: 100 + 102
    assert_eq!(w.px(100, 90)[0], 202);
    // The fixed value is locked while animated.
    assert!(w.ed.set_effect_value(w.seq, c, b, "amount", 5.0).is_err());
    w.ed.clear_effect_animation(w.seq, c, b, "amount").unwrap();
    assert_eq!(w.px(100, 48), [100, 100, 100, 255]);
}

#[test]
fn cutting_a_clip_does_not_change_its_animated_effects() {
    let (mut w, c) = world();
    let b =
        w.ed.add_effect(w.seq, c, EffectKind::Brightness, None)
            .unwrap();
    w.ed.key_effect(w.seq, c, b, "amount", secs(0.0), -30.0, Interp::Smooth)
        .unwrap();
    w.ed.key_effect(w.seq, c, b, "amount", secs(4.0), 60.0, Interp::Smooth)
        .unwrap();
    let before: Vec<Vec<u8>> = (0..96).step_by(5).map(|f| w.whole(120, f)).collect();
    w.ed.split_clip(w.seq, c, secs(1.5), LinkMode::Follow)
        .unwrap();
    let after: Vec<Vec<u8>> = (0..96).step_by(5).map(|f| w.whole(120, f)).collect();
    assert_eq!(before, after);
}

#[test]
fn colour_effects_work_through_the_renderer() {
    let (mut w, c) = world();
    let s =
        w.ed.add_effect(w.seq, c, EffectKind::Saturation, None)
            .unwrap();
    w.ed.set_effect_value(w.seq, c, s, "amount", 0.0).unwrap();
    let g = w.ed.add_effect(w.seq, c, EffectKind::Gamma, None).unwrap();
    w.ed.set_effect_value(w.seq, c, g, "gamma", 2.0).unwrap();
    let p = w.px(128, 0);
    assert!(
        (i32::from(p[0]) - 181).abs() <= 1 && p[0] == p[1] && p[1] == p[2],
        "{p:?}"
    );
}

#[test]
fn a_vignette_dims_the_corners_but_not_the_middle() {
    let (mut w, c) = world();
    let v =
        w.ed.add_effect(w.seq, c, EffectKind::Vignette, None)
            .unwrap();
    w.ed.set_effect_value(w.seq, c, v, "amount", 100.0).unwrap();
    let data = w.whole(200, 0);
    assert_eq!(&data[(9 * 32 + 16) * 4..][..4], &[200, 200, 200, 255]);
    assert!(data[0] < 30);
}

#[test]
fn bad_effect_edits_are_refused_and_change_nothing() {
    let (mut w, c) = world();
    let b =
        w.ed.add_effect(w.seq, c, EffectKind::Brightness, None)
            .unwrap();
    let before = w.ed.project().clone();
    assert!(w.ed.set_effect_value(w.seq, c, b, "amount", 500.0).is_err());
    assert!(w.ed.set_effect_value(w.seq, c, b, "nonsense", 1.0).is_err());
    assert!(w
        .ed
        .set_effect_value(w.seq, c, b, "amount", f64::NAN)
        .is_err());
    assert!(w
        .ed
        .key_effect(w.seq, c, b, "amount", secs(9.0), 1.0, Interp::Linear)
        .is_err());
    assert!(w
        .ed
        .remove_effect(w.seq, c, timeline_engine::EffectId(99))
        .is_err());
    assert!(w.ed.project() == &before);
}
