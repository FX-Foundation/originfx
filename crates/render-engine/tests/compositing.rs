// SPDX-License-Identifier: GPL-3.0-or-later
//! Opacity, blend modes, transform, crop and masks, checked on exact pixels with a source that gives one flat colour.
use std::sync::Arc;

use media_engine::VideoFrame;
use render_engine::{MediaSource, RenderSettings, Renderer, Result};
use timeline_engine::{
    Asset, AssetId, BlendMode, ClipId, ClipSpec, Editor, FrameRate, Interp, LinkMode, MaskShape,
    NewAsset, Param, PlaceMode, SequenceId, SequenceSettings, Target, Time, VideoProps,
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

fn at(data: &[u8], x: usize, y: usize) -> [u8; 4] {
    let i = (y * 32 + x) * 4;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}

#[test]
fn a_plain_look_changes_nothing_and_opacity_mixes_with_black() {
    let (mut w, c) = world();
    let plain = w.whole(200, 3);
    // Setting every value to its default is still bit-exact.
    w.ed.set_look_value(w.seq, c, Param::Opacity, 100.0)
        .unwrap();
    w.ed.set_blend_mode(w.seq, c, BlendMode::Normal).unwrap();
    assert_eq!(w.whole(200, 3), plain);
    w.ed.set_look_value(w.seq, c, Param::Opacity, 50.0).unwrap();
    let p = w.px(200, 3);
    assert!((i32::from(p[0]) - 100).abs() <= 1, "{p:?}");
    assert_eq!(p[3], 255);
    w.ed.set_look_value(w.seq, c, Param::Opacity, 0.0).unwrap();
    assert_eq!(w.px(200, 3), [0, 0, 0, 255]);
}

#[test]
fn opacity_can_fade_with_keyframes() {
    let (mut w, c) = world();
    let t = Target::Clip(c);
    w.ed.set_keyframe(
        w.seq,
        t,
        Param::Opacity,
        secs(0.0),
        0.0,
        Interp::Linear,
        LinkMode::Follow,
    )
    .unwrap();
    w.ed.set_keyframe(
        w.seq,
        t,
        Param::Opacity,
        secs(2.0),
        100.0,
        Interp::Linear,
        LinkMode::Follow,
    )
    .unwrap();
    assert_eq!(w.px(200, 0), [0, 0, 0, 255]);
    let mid = w.px(200, 24)[0];
    assert!((i32::from(mid) - 100).abs() <= 2, "{mid}");
    assert_eq!(w.px(200, 60), [200, 200, 200, 255]);
}

#[test]
fn blend_modes_use_the_layer_beneath() {
    // Two layers on two tracks, both the same grey (the test source gives every clip one colour).
    let (mut w, _c) = world();
    w.ed.add_track(w.seq, timeline_engine::TrackKind::Video, None)
        .unwrap();
    let v2 = w.ed.project().sequence(w.seq).unwrap().video_tracks()[1].id;
    let mut s = ClipSpec::new(w.asset, secs(0.0));
    s.duration = Some(secs(4.0));
    let top = w.ed.place_clip(w.seq, v2, s, PlaceMode::Overwrite).unwrap();
    w.ed.set_blend_mode(w.seq, top, BlendMode::Difference)
        .unwrap();
    // Both layers are the same grey, so the difference is black.
    assert_eq!(w.px(150, 0), [0, 0, 0, 255]);
    w.ed.set_blend_mode(w.seq, top, BlendMode::Add).unwrap();
    assert_eq!(w.px(150, 0), [255, 255, 255, 255]);
    w.ed.set_blend_mode(w.seq, top, BlendMode::Multiply)
        .unwrap();
    let m = w.px(150, 0)[0];
    // 150/255 squared of 255 is about 88.
    assert!((i32::from(m) - 88).abs() <= 1, "{m}");
    w.ed.set_blend_mode(w.seq, top, BlendMode::Normal).unwrap();
    assert_eq!(w.px(150, 0), [150, 150, 150, 255]);
}

#[test]
fn moving_leaves_the_rest_of_the_frame_clear() {
    let (mut w, c) = world();
    // Right by half the frame: the left half is now empty (black), the right half is picture.
    w.ed.set_look_value(w.seq, c, Param::PositionX, 50.0)
        .unwrap();
    let f = w.whole(200, 0);
    assert_eq!(at(&f, 4, 9), [0, 0, 0, 255]);
    assert_eq!(at(&f, 28, 9), [200, 200, 200, 255]);
}

#[test]
fn scale_and_rotation_shrink_and_turn_the_picture() {
    let (mut w, c) = world();
    w.ed.set_look_value(w.seq, c, Param::ScaleX, 50.0).unwrap();
    w.ed.set_look_value(w.seq, c, Param::ScaleY, 50.0).unwrap();
    let f = w.whole(200, 0);
    assert_eq!(at(&f, 16, 9), [200, 200, 200, 255]);
    assert_eq!(at(&f, 1, 1), [0, 0, 0, 255]);
    assert_eq!(at(&f, 30, 16), [0, 0, 0, 255]);
    // 90 degrees: a wide frame turned on its side no longer reaches the left and right edges.
    w.ed.set_look_value(w.seq, c, Param::ScaleX, 100.0).unwrap();
    w.ed.set_look_value(w.seq, c, Param::ScaleY, 100.0).unwrap();
    w.ed.set_look_value(w.seq, c, Param::Rotation, 90.0)
        .unwrap();
    let f = w.whole(200, 0);
    assert_eq!(at(&f, 16, 9), [200, 200, 200, 255]);
    assert_eq!(at(&f, 1, 9), [0, 0, 0, 255]);
    assert_eq!(at(&f, 16, 0)[0], 200);
}

#[test]
fn crop_and_masks_cut_the_picture() {
    let (mut w, c) = world();
    w.ed.set_look_value(w.seq, c, Param::CropLeft, 50.0)
        .unwrap();
    let f = w.whole(200, 0);
    assert_eq!(at(&f, 4, 9), [0, 0, 0, 255]);
    assert_eq!(at(&f, 28, 9), [200, 200, 200, 255]);
    w.ed.set_look_value(w.seq, c, Param::CropLeft, 0.0).unwrap();
    w.ed.set_mask(w.seq, c, MaskShape::Ellipse, false).unwrap();
    let f = w.whole(200, 0);
    assert_eq!(at(&f, 16, 9), [200, 200, 200, 255]);
    assert_eq!(at(&f, 0, 0), [0, 0, 0, 255]);
    // Inverted: the other way round.
    w.ed.set_mask(w.seq, c, MaskShape::Ellipse, true).unwrap();
    let f = w.whole(200, 0);
    assert_eq!(at(&f, 16, 9), [0, 0, 0, 255]);
    assert_eq!(at(&f, 0, 0), [200, 200, 200, 255]);
    // A mask that grows over time.
    w.ed.set_mask(w.seq, c, MaskShape::Rect, false).unwrap();
    let t = Target::Clip(c);
    w.ed.set_keyframe(
        w.seq,
        t,
        Param::MaskWidth,
        secs(0.0),
        10.0,
        Interp::Linear,
        LinkMode::Follow,
    )
    .unwrap();
    w.ed.set_keyframe(
        w.seq,
        t,
        Param::MaskWidth,
        secs(2.0),
        100.0,
        Interp::Linear,
        LinkMode::Follow,
    )
    .unwrap();
    w.ed.set_look_value(w.seq, c, Param::MaskHeight, 100.0)
        .unwrap();
    let early = w.whole(200, 0);
    let late = w.whole(200, 48);
    assert_eq!(at(&early, 2, 9), [0, 0, 0, 255]);
    assert_eq!(at(&late, 2, 9), [200, 200, 200, 255]);
}

#[test]
fn look_and_effects_work_together_and_rendering_is_repeatable() {
    let (mut w, c) = world();
    let fx =
        w.ed.add_effect(w.seq, c, timeline_engine::EffectKind::Invert, None)
            .unwrap();
    let _ = fx;
    w.ed.set_look_value(w.seq, c, Param::Opacity, 50.0).unwrap();
    // Invert 200 -> 55, then half over black -> about 27.
    let p = w.px(200, 0);
    assert!((i32::from(p[0]) - 27).abs() <= 1, "{p:?}");
    assert_eq!(w.whole(200, 5), w.whole(200, 5));
}
