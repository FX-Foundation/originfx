// SPDX-License-Identifier: GPL-3.0-or-later
//! Compositing and mixing, checked exactly with a source that makes up its own pictures and sound.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use media_engine::VideoFrame;
use render_engine::{
    Error, Fit, MediaSource, MissingMedia, RenderSettings, Renderer, Result, OFFLINE_COLOUR,
};
use timeline_engine::{
    Asset, AssetId, AudioProps, ClipId, ClipSpec, Editor, FrameRate, LinkMode, NewAsset, PlaceMode,
    SequenceId, SequenceSettings, Time, TimeRange, TrackId, TrackUpdate, VideoProps,
};

#[derive(Default)]
struct Spec {
    size: (u32, u32),
    colour: [u8; 4],
    level: f32,
}

#[derive(Default)]
struct Fake {
    specs: HashMap<AssetId, Spec>,
    asked: Rc<RefCell<Vec<(AssetId, f64)>>>,
}

impl MediaSource for Fake {
    fn video_frame(&mut self, asset: &Asset, _s: u32, secs: f64) -> Result<Arc<VideoFrame>> {
        self.asked.borrow_mut().push((asset.id, secs));
        let sp = &self.specs[&asset.id];
        let (w, h) = sp.size;
        let data = sp
            .colour
            .iter()
            .copied()
            .cycle()
            .take((w * h * 4) as usize)
            .collect();
        Ok(Arc::new(VideoFrame {
            width: w,
            height: h,
            pts_secs: secs,
            data,
        }))
    }
    fn audio(
        &mut self,
        asset: &Asset,
        _s: u32,
        _secs: f64,
        frames: usize,
        _r: u32,
    ) -> Result<Vec<f32>> {
        Ok(vec![self.specs[&asset.id].level; frames * 2])
    }
}

struct World {
    ed: Editor,
    seq: SequenceId,
    fake: Fake,
}

fn world() -> World {
    let mut ed = Editor::new("t");
    let seq = ed
        .add_sequence(SequenceSettings {
            width: 32,
            height: 18,
            ..Default::default()
        })
        .unwrap();
    World {
        ed,
        seq,
        fake: Fake::default(),
    }
}

impl World {
    fn asset(&mut self, size: (u32, u32), colour: [u8; 4], level: f32) -> AssetId {
        let id = self
            .ed
            .add_asset(NewAsset {
                name: "x".into(),
                path: "/nope.mp4".into(),
                duration: Some(Time::from_secs_f64(60.0)),
                video: vec![VideoProps {
                    width: size.0,
                    height: size.1,
                    frame_rate: Some(FrameRate::FPS_24),
                }],
                audio: vec![AudioProps {
                    sample_rate: 48000,
                    channels: 2,
                }],
            })
            .unwrap();
        self.fake.specs.insert(
            id,
            Spec {
                size,
                colour,
                level,
            },
        );
        id
    }
    fn video(&self, n: usize) -> TrackId {
        self.ed.project().sequence(self.seq).unwrap().video_tracks()[n].id
    }
    fn audio_track(&self, n: usize) -> TrackId {
        self.ed.project().sequence(self.seq).unwrap().audio_tracks()[n].id
    }
    fn put(
        &mut self,
        track: TrackId,
        asset: AssetId,
        start: f64,
        source_in: f64,
        dur: f64,
    ) -> ClipId {
        let mut s = ClipSpec::new(asset, Time::from_secs_f64(start));
        s.source_in = Time::from_secs_f64(source_in);
        s.duration = Some(Time::from_secs_f64(dur));
        self.ed
            .place_clip(self.seq, track, s, PlaceMode::Overwrite)
            .unwrap()
    }
    fn renderer(self, settings: RenderSettings) -> (Renderer<Fake>, Editor, SequenceId) {
        let r = Renderer::new(self.ed.project(), self.seq, self.fake, settings).unwrap();
        (r, self.ed, self.seq)
    }
}

fn px(f: &VideoFrame, x: usize, y: usize) -> [u8; 4] {
    let i = (y * f.width as usize + x) * 4;
    [f.data[i], f.data[i + 1], f.data[i + 2], f.data[i + 3]]
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

#[test]
fn upper_layers_cover_lower_ones_and_gaps_show_background() {
    let mut w = world();
    let (red, blue) = (w.asset((32, 18), RED, 0.0), w.asset((32, 18), BLUE, 0.0));
    let (v1, v2) = (w.video(0), w.video(1));
    w.put(v1, red, 0.0, 0.0, 4.0);
    w.put(v2, blue, 1.0, 0.0, 1.0);
    let (mut r, _, _) = w.renderer(RenderSettings::default());
    assert_eq!(px(&r.frame(0).unwrap(), 5, 5), RED);
    assert_eq!(px(&r.frame(30).unwrap(), 5, 5), BLUE); // 1.25 s
    assert_eq!(px(&r.frame(48).unwrap(), 5, 5), RED); // 2.0 s, blue ended
    assert_eq!(px(&r.frame(100).unwrap(), 5, 5), [0, 0, 0, 255]); // past the end
}

#[test]
fn disabled_clips_and_tracks_are_skipped() {
    let mut w = world();
    let (red, blue) = (w.asset((32, 18), RED, 0.0), w.asset((32, 18), BLUE, 0.0));
    let (v1, v2) = (w.video(0), w.video(1));
    w.put(v1, red, 0.0, 0.0, 2.0);
    let top = w.put(v2, blue, 0.0, 0.0, 2.0);
    w.ed.set_enabled(w.seq, &[top], false, LinkMode::Ignore)
        .unwrap();
    let seq = w.seq;
    let (mut r, mut ed, _) = w.renderer(RenderSettings::default());
    assert_eq!(px(&r.frame(0).unwrap(), 1, 1), RED);
    ed.set_enabled(seq, &[top], true, LinkMode::Ignore).unwrap();
    let v2 = ed.project().sequence(seq).unwrap().video_tracks()[1].id;
    ed.update_track(
        seq,
        v2,
        TrackUpdate {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    let mut r2 = Renderer::new(
        ed.project(),
        seq,
        Fake {
            specs: r.into_source().specs,
            asked: Default::default(),
        },
        RenderSettings::default(),
    )
    .unwrap();
    assert_eq!(px(&r2.frame(0).unwrap(), 1, 1), RED);
}

#[test]
fn source_time_follows_the_clip_mapping() {
    let mut w = world();
    let a = w.asset((32, 18), RED, 0.0);
    let v1 = w.video(0);
    w.put(v1, a, 2.0, 5.0, 3.0);
    let asked = Rc::clone(&w.fake.asked);
    let (mut r, _, _) = w.renderer(RenderSettings::default());
    r.frame(72).unwrap(); // 3.0 s on the timeline = 6.0 s in the media
    r.frame(48).unwrap(); // 2.0 s = 5.0 s
    let got: Vec<f64> = asked.borrow().iter().map(|(_, s)| *s).collect();
    assert!(
        (got[0] - 6.0).abs() < 1e-9 && (got[1] - 5.0).abs() < 1e-9,
        "{got:?}"
    );
}

#[test]
fn different_shapes_get_bars_or_crop_or_stretch() {
    let mut w = world();
    // 4:3 picture on a 16:9 frame (32x18).
    let a = w.asset((40, 30), RED, 0.0);
    let v1 = w.video(0);
    w.put(v1, a, 0.0, 0.0, 2.0);
    let ed = w.ed.clone();
    let (seq, specs) = (w.seq, w.fake.specs);
    let make = |fit| {
        let fake = Fake {
            specs: specs
                .iter()
                .map(|(k, v)| {
                    (
                        *k,
                        Spec {
                            size: v.size,
                            colour: v.colour,
                            level: v.level,
                        },
                    )
                })
                .collect(),
            asked: Default::default(),
        };
        Renderer::new(
            ed.project(),
            seq,
            fake,
            RenderSettings {
                fit,
                ..Default::default()
            },
        )
        .unwrap()
    };
    // Contain: 24x18 picture centred, 4 px bars each side.
    let f = make(Fit::Contain).frame(0).unwrap();
    assert_eq!(px(&f, 0, 9), [0, 0, 0, 255]);
    assert_eq!(px(&f, 3, 9), [0, 0, 0, 255]);
    assert_eq!(px(&f, 4, 9), RED);
    assert_eq!(px(&f, 27, 9), RED);
    assert_eq!(px(&f, 28, 9), [0, 0, 0, 255]);
    // Cover and stretch fill every pixel.
    for fit in [Fit::Cover, Fit::Stretch] {
        let f = make(fit).frame(0).unwrap();
        assert!(
            (0..32).all(|x| (0..18).all(|y| px(&f, x, y) == RED)),
            "{fit:?}"
        );
    }
}

#[test]
fn translucent_layers_blend_over_what_is_below() {
    let mut w = world();
    let red = w.asset((32, 18), RED, 0.0);
    let glass = w.asset((32, 18), [0, 0, 255, 128], 0.0);
    let (v1, v2) = (w.video(0), w.video(1));
    w.put(v1, red, 0.0, 0.0, 2.0);
    w.put(v2, glass, 0.0, 0.0, 2.0);
    let (mut r, _, _) = w.renderer(RenderSettings::default());
    let p = px(&r.frame(0).unwrap(), 2, 2);
    assert!(
        (i32::from(p[0]) - 127).abs() <= 1 && (i32::from(p[2]) - 128).abs() <= 1 && p[3] == 255,
        "{p:?}"
    );
}

#[test]
fn audio_is_summed_muted_soloed_and_clipped() {
    let mut w = world();
    let (quiet, loud) = (w.asset((32, 18), RED, 0.25), w.asset((32, 18), RED, 0.5));
    let (a1, a2) = (w.audio_track(0), w.audio_track(1));
    w.put(a1, quiet, 0.0, 0.0, 2.0);
    w.put(a2, loud, 1.0, 0.0, 2.0);
    let (seq, ed, fake) = (w.seq, w.ed.clone(), w.fake);
    let specs = fake.specs;
    let fresh = |ed: &Editor| {
        let f = Fake {
            specs: specs
                .iter()
                .map(|(k, v)| {
                    (
                        *k,
                        Spec {
                            size: v.size,
                            colour: v.colour,
                            level: v.level,
                        },
                    )
                })
                .collect(),
            asked: Default::default(),
        };
        Renderer::new(ed.project(), seq, f, RenderSettings::default()).unwrap()
    };
    let mut r = fresh(&ed);
    // Mixed length matches the request exactly.
    let b = r
        .audio(TimeRange::new(Time::ZERO, Time::from_secs_f64(3.0)).unwrap())
        .unwrap();
    assert_eq!(
        (b.channels, b.sample_rate, b.samples.len()),
        (2, 48000, 3 * 48000 * 2)
    );
    let at = |sec: f64| b.samples[(sec * 48000.0) as usize * 2];
    assert_eq!(at(0.5), 0.25);
    assert_eq!(at(1.5), 0.75);
    assert_eq!(at(2.5), 0.5);
    // Sound joins without gaps: two calls equal one.
    let one = r.audio_samples(40_000, 20_000).unwrap();
    let mut two = r.audio_samples(40_000, 7_777).unwrap();
    two.extend(r.audio_samples(47_777, 20_000 - 7_777).unwrap());
    assert_eq!(one, two);
    // Clip edges are exact: sample 47999 quiet only, 48000 both.
    let edge = r.audio_samples(47_999, 2).unwrap();
    assert_eq!((edge[0], edge[2]), (0.25, 0.75));
    // Mute track 1.
    let mut ed2 = ed.clone();
    ed2.update_track(
        seq,
        a1,
        TrackUpdate {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(fresh(&ed2).audio_samples(60_000, 1).unwrap()[0], 0.5);
    // Solo track 1 only.
    let mut ed3 = ed.clone();
    ed3.update_track(
        seq,
        a1,
        TrackUpdate {
            solo: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(fresh(&ed3).audio_samples(60_000, 1).unwrap()[0], 0.25);
    assert_eq!(fresh(&ed3).audio_samples(110_000, 1).unwrap()[0], 0.0);
}

#[test]
fn loud_mixes_are_clipped_not_wrapped() {
    let mut w = world();
    let a = w.asset((32, 18), RED, 0.8);
    let (t1, t2) = (w.audio_track(0), w.audio_track(1));
    w.put(t1, a, 0.0, 0.0, 1.0);
    w.put(t2, a, 0.0, 0.0, 1.0);
    let (mut r, _, _) = w.renderer(RenderSettings::default());
    assert_eq!(r.audio_samples(100, 4).unwrap(), vec![1.0; 8]);
}

#[test]
fn still_images_look_identical_on_every_frame() {
    let mut ed = Editor::new("s");
    let seq = ed
        .add_sequence(SequenceSettings {
            width: 32,
            height: 18,
            ..Default::default()
        })
        .unwrap();
    let still = ed
        .add_asset(NewAsset {
            name: "still".into(),
            path: "/x.png".into(),
            duration: None,
            video: vec![VideoProps {
                width: 64,
                height: 36,
                frame_rate: None,
            }],
            audio: vec![],
        })
        .unwrap();
    let v1 = ed.project().sequence(seq).unwrap().video_tracks()[0].id;
    let mut spec = ClipSpec::new(still, Time::ZERO);
    spec.duration = Some(Time::from_secs_f64(2.0));
    ed.place_clip(seq, v1, spec, PlaceMode::Overwrite).unwrap();
    let mut fake = Fake::default();
    fake.specs.insert(
        still,
        Spec {
            size: (64, 36),
            colour: BLUE,
            level: 0.0,
        },
    );
    let mut r = Renderer::new(ed.project(), seq, fake, RenderSettings::default()).unwrap();
    let first = r.frame(0).unwrap();
    for i in 0..5 {
        let f = r.frame(i).unwrap();
        assert_eq!(px(&f, 10, 10), BLUE);
        assert_eq!(
            f.data, first.data,
            "a still must look the same on every frame"
        );
    }
}

#[test]
fn offline_media_is_magenta_or_an_error() {
    struct Gone;
    impl MediaSource for Gone {
        fn video_frame(&mut self, a: &Asset, _: u32, _: f64) -> Result<Arc<VideoFrame>> {
            Err(Error::MissingMedia {
                asset: a.id,
                name: a.name.clone(),
                path: a.path.clone(),
            })
        }
        fn audio(&mut self, a: &Asset, _: u32, _: f64, _: usize, _: u32) -> Result<Vec<f32>> {
            Err(Error::MissingMedia {
                asset: a.id,
                name: a.name.clone(),
                path: a.path.clone(),
            })
        }
    }
    let mut w = world();
    let a = w.asset((32, 18), RED, 0.5);
    let (v1, a1) = (w.video(0), w.audio_track(0));
    w.put(v1, a, 0.0, 0.0, 1.0);
    w.put(a1, a, 0.0, 0.0, 1.0);
    let (ed, seq) = (w.ed, w.seq);
    let mut strict = Renderer::new(ed.project(), seq, Gone, RenderSettings::default()).unwrap();
    assert!(matches!(strict.frame(0), Err(Error::MissingMedia { .. })));
    assert!(matches!(
        strict.audio_samples(0, 10),
        Err(Error::MissingMedia { .. })
    ));
    let lenient = RenderSettings {
        missing_media: MissingMedia::Offline,
        ..Default::default()
    };
    let mut r = Renderer::new(ed.project(), seq, Gone, lenient).unwrap();
    assert_eq!(px(&r.frame(0).unwrap(), 3, 3), OFFLINE_COLOUR);
    assert_eq!(r.audio_samples(0, 10).unwrap(), vec![0.0; 20]);
}
