// SPDX-License-Identifier: GPL-3.0-or-later
//! Drawn graphics (colours, gradients, shapes, text) rendered into frames, checked on exact pixels with a source that gives one flat colour.
use std::sync::Arc;

use media_engine::VideoFrame;
use render_engine::{MediaSource, RenderSettings, Renderer, Result};
use timeline_engine::{
    Asset, AssetId, ClipId, ClipSpec, Colour, Editor, Fill, FrameRate, Generator, NewAsset, Param,
    PlaceMode, SequenceId, SequenceSettings, Shape, ShapeKind, Text, Time, VideoProps,
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

fn on_top(w: &mut W, g: Generator) -> ClipId {
    let a = w.ed.add_generated("g", g).unwrap();
    w.ed.add_track(w.seq, timeline_engine::TrackKind::Video, None)
        .unwrap();
    let tracks = w.ed.project().sequence(w.seq).unwrap().video_tracks().len();
    let top = w.ed.project().sequence(w.seq).unwrap().video_tracks()[tracks - 1].id;
    let mut s = ClipSpec::new(a, secs(0.0));
    s.duration = Some(secs(4.0));
    w.ed.place_clip(w.seq, top, s, PlaceMode::Overwrite)
        .unwrap()
}

fn rect(wp: f64, hp: f64, c: Colour) -> Generator {
    Generator::Shape(Shape {
        kind: ShapeKind::Rect { corner_pct: 0.0 },
        width_pct: wp,
        height_pct: hp,
        fill: Fill::Solid(c),
        stroke: None,
    })
}

#[test]
fn a_solid_covers_the_picture_beneath_and_opacity_lets_it_through() {
    let (mut w, _) = world();
    let c = on_top(&mut w, Generator::Fill(Fill::Solid(Colour::rgb(255, 0, 0))));
    let f = w.whole(100, 0);
    assert!(f.chunks(4).all(|p| p == [255, 0, 0, 255]));
    w.ed.set_look_value(w.seq, c, Param::Opacity, 50.0).unwrap();
    let p = w.px(100, 0);
    // Half red over grey 100.
    assert!(
        (i32::from(p[0]) - 178).abs() <= 1 && (i32::from(p[1]) - 50).abs() <= 1,
        "{p:?}"
    );
    // A half-transparent colour in the graphic itself does the same.
    w.ed.set_look_value(w.seq, c, Param::Opacity, 100.0)
        .unwrap();
    let a =
        w.ed.project()
            .sequence(w.seq)
            .unwrap()
            .find_clip(c)
            .unwrap()
            .1
            .asset;
    w.ed.set_generated(
        a,
        Generator::Fill(Fill::Solid(Colour::parse("#ff000080").unwrap())),
    )
    .unwrap();
    let q = w.px(100, 0);
    assert!(
        (i32::from(q[0]) - i32::from(p[0])).abs() <= 1,
        "{q:?} vs {p:?}"
    );
}

#[test]
fn a_shape_sits_in_the_middle_and_leaves_the_rest_alone() {
    let (mut w, _) = world();
    let c = on_top(&mut w, rect(50.0, 50.0, Colour::rgb(0, 0, 255)));
    let f = w.whole(100, 0);
    assert_eq!(at(&f, 16, 9), [0, 0, 255, 255]);
    assert_eq!(at(&f, 2, 2), [100, 100, 100, 255]);
    assert_eq!(at(&f, 30, 16), [100, 100, 100, 255]);
    // It moves and scales like any picture.
    w.ed.set_look_value(w.seq, c, Param::PositionX, 50.0)
        .unwrap();
    let f = w.whole(100, 0);
    assert_eq!(at(&f, 16, 9), [100, 100, 100, 255]);
    assert_eq!(at(&f, 28, 9), [0, 0, 255, 255]);
}

#[test]
fn text_draws_and_a_missing_font_is_a_clean_error() {
    let (mut w, _) = world();
    let mut t = Text {
        text: "I".into(),
        font: Some("/no/such/font.ttf".into()),
        size_pct: 80.0,
        colour: Colour::rgb(255, 255, 255),
        align: timeline_engine::Align::Centre,
        line_gap_pct: 0.0,
        tracking_pct: 0.0,
        outline: None,
        shadow: None,
    };
    let c = on_top(&mut w, Generator::Text(t.clone()));
    let mut r = Renderer::new(w.ed.project(), w.seq, Grey(0), RenderSettings::default()).unwrap();
    let e = r.frame(0).unwrap_err();
    assert!(e.to_string().contains("font"), "{e}");
    // With a real font it draws white ink on the black picture, and the same every time.
    if motion_graphics_engine::Fonts::default_path().is_some() {
        t.font = None;
        let a =
            w.ed.project()
                .sequence(w.seq)
                .unwrap()
                .find_clip(c)
                .unwrap()
                .1
                .asset;
        w.ed.set_generated(a, Generator::Text(t)).unwrap();
        let f = w.whole(0, 0);
        assert!(f.chunks(4).any(|p| p[0] > 200));
        assert_eq!(f, w.whole(0, 0));
    }
}

#[test]
fn sound_is_unaffected_by_graphics() {
    let (mut w, _) = world();
    let before = Renderer::new(w.ed.project(), w.seq, Grey(0), RenderSettings::default())
        .unwrap()
        .audio_samples(0, 4800)
        .unwrap();
    on_top(&mut w, rect(10.0, 10.0, Colour::WHITE));
    let after = Renderer::new(w.ed.project(), w.seq, Grey(0), RenderSettings::default())
        .unwrap()
        .audio_samples(0, 4800)
        .unwrap();
    assert_eq!(before, after);
}
