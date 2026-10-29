// SPDX-License-Identifier: GPL-3.0-or-later
//! Speed, reverse and transitions, checked exactly with a source whose pictures and sound say
//! where in the media they came from.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use media_engine::VideoFrame;
use render_engine::{MediaSource, RenderSettings, Renderer, Result};
use timeline_engine::{
    Alignment, Asset, AssetId, AudioProps, ClipId, ClipSpec, Direction, Editor, FrameRate,
    LinkMode, NewAsset, PlaceMode, Ripple, SequenceId, SequenceSettings, Speed, SpeedMode, Time,
    TimeRange, TrackId, Transition, TransitionKind, VideoProps,
};

const RATE: u32 = 48_000;

/// Every picture is one flat colour; every sound sample is its own media sample number divided
/// by a million, unless the asset has a fixed level.
#[derive(Default)]
struct Fake {
    colour: HashMap<AssetId, [u8; 4]>,
    level: HashMap<AssetId, f32>,
    asked: Rc<RefCell<Vec<f64>>>,
}

impl MediaSource for Fake {
    fn video_frame(&mut self, asset: &Asset, _s: u32, secs: f64) -> Result<Arc<VideoFrame>> {
        self.asked.borrow_mut().push(secs);
        let c = self.colour[&asset.id];
        Ok(Arc::new(VideoFrame {
            width: 16,
            height: 9,
            pts_secs: secs,
            data: c.iter().copied().cycle().take(16 * 9 * 4).collect(),
        }))
    }

    fn audio(
        &mut self,
        asset: &Asset,
        _s: u32,
        secs: f64,
        frames: usize,
        _r: u32,
    ) -> Result<Vec<f32>> {
        if let Some(l) = self.level.get(&asset.id) {
            return Ok(vec![*l; frames * 2]);
        }
        let first = (secs * f64::from(RATE)).round() as i64;
        let mut v = Vec::with_capacity(frames * 2);
        for i in 0..frames as i64 {
            let s = (first + i) as f32 / 1_000_000.0;
            v.extend([s, s]);
        }
        Ok(v)
    }
}

struct World {
    ed: Editor,
    seq: SequenceId,
    fake: Fake,
}

fn secs(s: f64) -> Time {
    Time::from_secs_f64(s)
}

fn world() -> World {
    let mut ed = Editor::new("t");
    let seq = ed
        .add_sequence(SequenceSettings {
            width: 16,
            height: 9,
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
    fn asset(&mut self, colour: [u8; 4], level: Option<f32>) -> AssetId {
        let id = self
            .ed
            .add_asset(NewAsset {
                name: "x".into(),
                path: "/nope.mp4".into(),
                duration: Some(secs(60.0)),
                video: vec![VideoProps {
                    width: 16,
                    height: 9,
                    frame_rate: Some(FrameRate::FPS_24),
                }],
                audio: vec![AudioProps {
                    sample_rate: RATE,
                    channels: 2,
                }],
            })
            .unwrap();
        self.fake.colour.insert(id, colour);
        if let Some(l) = level {
            self.fake.level.insert(id, l);
        }
        id
    }

    fn v(&self) -> TrackId {
        self.ed.project().sequence(self.seq).unwrap().video_tracks()[0].id
    }

    fn a(&self) -> TrackId {
        self.ed.project().sequence(self.seq).unwrap().audio_tracks()[0].id
    }

    fn put(
        &mut self,
        track: TrackId,
        asset: AssetId,
        start: f64,
        source_in: f64,
        dur: f64,
    ) -> ClipId {
        let mut s = ClipSpec::new(asset, secs(start));
        s.source_in = secs(source_in);
        s.duration = Some(secs(dur));
        self.ed
            .place_clip(self.seq, track, s, PlaceMode::Overwrite)
            .unwrap()
    }

    fn renderer(&self) -> (Renderer<Fake>, Rc<RefCell<Vec<f64>>>) {
        let asked: Rc<RefCell<Vec<f64>>> = Default::default();
        let fake = Fake {
            colour: self.fake.colour.clone(),
            level: self.fake.level.clone(),
            asked: Rc::clone(&asked),
        };
        (
            Renderer::new(self.ed.project(), self.seq, fake, RenderSettings::default()).unwrap(),
            asked,
        )
    }
}

fn red(f: &VideoFrame) -> u8 {
    f.data[0]
}

fn blue(f: &VideoFrame) -> u8 {
    f.data[2]
}

#[test]
fn video_frames_follow_speed() {
    let mut w = world();
    let a = w.asset([255, 0, 0, 255], None);
    let c = w.put(w.v(), a, 0.0, 10.0, 4.0);
    w.ed.set_speed(
        w.seq,
        c,
        Speed::new(2, 1).unwrap(),
        SpeedMode::KeepLength,
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    let (mut r, asked) = w.renderer();
    r.frame(0).unwrap();
    r.frame(24).unwrap(); // 1 s in, so 2 s of media
    r.frame(48).unwrap();
    let asked = asked.borrow();
    assert!((asked[0] - 10.0).abs() < 1e-9);
    assert!((asked[1] - 12.0).abs() < 1e-9);
    assert!((asked[2] - 14.0).abs() < 1e-9);
}

#[test]
fn video_frames_play_backwards() {
    let mut w = world();
    let a = w.asset([255, 0, 0, 255], None);
    let c = w.put(w.v(), a, 0.0, 10.0, 2.0);
    w.ed.set_reverse(w.seq, c, true, LinkMode::Follow).unwrap();
    let (mut r, asked) = w.renderer();
    r.frame(0).unwrap();
    r.frame(1).unwrap();
    r.frame(47).unwrap();
    let asked = asked.borrow();
    let f = 1.0 / 24.0;
    assert!((asked[0] - (12.0 - f)).abs() < 1e-6, "{}", asked[0]);
    assert!((asked[1] - (12.0 - 2.0 * f)).abs() < 1e-6);
    assert!(
        (asked[2] - 10.0).abs() < 1e-6,
        "the last frame is the first of the stretch"
    );
}

#[test]
fn reversed_sound_is_the_samples_backwards() {
    let mut w = world();
    let a = w.asset([0; 4], None);
    let c = w.put(w.a(), a, 0.0, 2.0, 1.0);
    w.ed.set_reverse(w.seq, c, true, LinkMode::Follow).unwrap();
    let (mut r, _) = w.renderer();
    let out = r.audio_samples(0, 48_000).unwrap();
    let origin = 2 * i64::from(RATE);
    for k in [0i64, 1, 100, 47_999] {
        let want = (origin + 47_999 - k) as f32 / 1_000_000.0;
        assert_eq!(out[k as usize * 2], want, "sample {k}");
    }
    // And it joins up: reading in two halves gives the same thing.
    let mut r2 = w.renderer().0;
    let mut halves = r2.audio_samples(0, 20_000).unwrap();
    halves.extend(r2.audio_samples(20_000, 28_000).unwrap());
    assert_eq!(halves, out);
}

#[test]
fn fast_sound_reads_every_other_sample() {
    let mut w = world();
    let a = w.asset([0; 4], None);
    let c = w.put(w.a(), a, 0.0, 2.0, 2.0);
    w.ed.set_speed(
        w.seq,
        c,
        Speed::new(2, 1).unwrap(),
        SpeedMode::KeepLength,
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    let (mut r, _) = w.renderer();
    let out = r.audio_samples(1000, 500).unwrap();
    let origin = 2 * i64::from(RATE);
    for j in [0usize, 1, 250, 499] {
        let want = (origin + 2 * (1000 + j as i64)) as f32 / 1_000_000.0;
        assert!(
            (out[j * 2] - want).abs() < 1e-6,
            "{j}: {} vs {want}",
            out[j * 2]
        );
    }
}

#[test]
fn slow_sound_is_interpolated() {
    let mut w = world();
    let a = w.asset([0; 4], None);
    let c = w.put(w.a(), a, 0.0, 2.0, 1.0);
    w.ed.set_speed(
        w.seq,
        c,
        Speed::new(1, 2).unwrap(),
        SpeedMode::KeepLength,
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    let (mut r, _) = w.renderer();
    let out = r.audio_samples(0, 1000).unwrap();
    let origin = 2.0 * f64::from(RATE);
    for j in [0usize, 1, 2, 3, 501, 999] {
        // A ramp stays a ramp: the value half-way between two samples is half-way between them.
        let want = ((origin + j as f64 * 0.5) / 1_000_000.0) as f32;
        assert!(
            (out[j * 2] - want).abs() < 1e-6,
            "{j}: {} vs {want}",
            out[j * 2]
        );
    }
}

fn two_clips(w: &mut World, tr: Transition) -> (ClipId, ClipId) {
    let a = w.asset([255, 0, 0, 255], Some(0.5));
    let b = w.asset([0, 0, 255, 255], Some(0.5));
    let c1 = w.put(w.v(), a, 0.0, 5.0, 4.0);
    let c2 = w.put(w.v(), b, 4.0, 5.0, 4.0);
    w.ed.set_transition(w.seq, c2, Some(tr), LinkMode::Follow)
        .unwrap();
    (c1, c2)
}

fn tr(kind: TransitionKind, d: f64, alignment: Alignment) -> Transition {
    Transition {
        kind,
        duration: secs(d),
        alignment,
    }
}

#[test]
fn dissolve_blends_and_ends_exactly() {
    let mut w = world();
    two_clips(&mut w, tr(TransitionKind::Dissolve, 1.0, Alignment::Center));
    let (mut r, _) = w.renderer();
    // Window is 3.5 s to 4.5 s: frames 84..108.
    let before = r.frame(83).unwrap();
    assert_eq!((red(&before), blue(&before)), (255, 0));
    let after = r.frame(108).unwrap();
    assert_eq!((red(&after), blue(&after)), (0, 255));
    let early = r.frame(84).unwrap();
    assert!(
        red(&early) > 240 && blue(&early) < 15,
        "first frame is nearly all outgoing"
    );
    let mid = r.frame(96).unwrap();
    // Frame 96 is the cut; progress is 12.5/24.
    let want = (255.0f64 * (1.0 - 12.5 / 24.0)).round() as i32;
    assert!(
        (i32::from(red(&mid)) - want).abs() <= 1,
        "{} vs {want}",
        red(&mid)
    );
    assert_eq!(i32::from(red(&mid)) + i32::from(blue(&mid)), 255);
    let late = r.frame(107).unwrap();
    assert!(blue(&late) > 240);
}

#[test]
fn start_and_end_alignments_put_the_window_on_the_right_side_of_the_cut() {
    let mut w = world();
    two_clips(
        &mut w,
        tr(TransitionKind::Dissolve, 1.0, Alignment::StartAtCut),
    );
    let (mut r, _) = w.renderer();
    // Starts at the cut (frame 96): everything before is untouched.
    let f = r.frame(95).unwrap();
    assert_eq!((red(&f), blue(&f)), (255, 0));
    let f = r.frame(96).unwrap();
    assert!(red(&f) > 240 && red(&f) < 255);
    let f = r.frame(120).unwrap();
    assert_eq!((red(&f), blue(&f)), (0, 255));

    let mut w = world();
    two_clips(
        &mut w,
        tr(TransitionKind::Dissolve, 1.0, Alignment::EndAtCut),
    );
    let (mut r, _) = w.renderer();
    let f = r.frame(72).unwrap(); // 3 s, the window starts
    assert!(red(&f) > 240 && blue(&f) > 0, "{} {}", red(&f), blue(&f));
    let f = r.frame(96).unwrap();
    assert_eq!((red(&f), blue(&f)), (0, 255), "done by the cut");
}

#[test]
fn both_clips_read_beyond_their_edges() {
    let mut w = world();
    two_clips(&mut w, tr(TransitionKind::Dissolve, 2.0, Alignment::Center));
    let (mut r, asked) = w.renderer();
    r.frame(72).unwrap(); // 3.0 s: window start
    let asked = asked.borrow();
    assert_eq!(asked.len(), 2);
    // Outgoing clip is 3 s in, its media is 5 s + 3 s. Incoming is 1 s before its start.
    assert!((asked[0] - 8.0).abs() < 1e-6);
    assert!((asked[1] - 4.0).abs() < 1e-6);
}

#[test]
fn dip_to_black_goes_dark_in_the_middle() {
    let mut w = world();
    two_clips(
        &mut w,
        tr(TransitionKind::DipToBlack, 1.0, Alignment::Center),
    );
    let (mut r, _) = w.renderer();
    // Frame 95 has progress 11.5/24, nearly the darkest point.
    let f = r.frame(95).unwrap();
    assert!(red(&f) < 15 && blue(&f) < 15, "{} {}", red(&f), blue(&f));
    assert_eq!(f.data[3], 255);
}

#[test]
fn wipe_and_push_show_both_pictures_at_once() {
    for kind in [
        TransitionKind::Wipe(Direction::Left),
        TransitionKind::Push(Direction::Left),
    ] {
        let mut w = world();
        two_clips(&mut w, tr(kind, 1.0, Alignment::Center));
        let (mut r, _) = w.renderer();
        let f = r.frame(96).unwrap();
        let px = |x: usize| (f.data[x * 4], f.data[x * 4 + 2]);
        assert_eq!(px(1), (0, 255), "{kind:?}: new picture on the left");
        assert_eq!(px(14), (255, 0), "{kind:?}: old picture on the right");
    }
}

#[test]
fn transition_on_a_lower_track_is_covered_by_the_upper_one() {
    let mut w = world();
    two_clips(&mut w, tr(TransitionKind::Dissolve, 1.0, Alignment::Center));
    let top = w.asset([0, 255, 0, 255], None);
    let v2 = w.ed.project().sequence(w.seq).unwrap().video_tracks()[1].id;
    w.put(v2, top, 3.0, 0.0, 2.0);
    let (mut r, _) = w.renderer();
    let f = r.frame(96).unwrap();
    assert_eq!(&f.data[..4], &[0, 255, 0, 255]);
}

#[test]
fn crossfade_is_equal_power_and_joins_the_clips() {
    let mut w = world();
    let a = w.asset([255, 0, 0, 255], Some(0.5));
    let b = w.asset([0, 0, 255, 255], Some(0.5));
    let c1 = w.put(w.a(), a, 0.0, 5.0, 4.0);
    let _ = c1;
    let c2 = w.put(w.a(), b, 4.0, 5.0, 4.0);
    w.ed.set_transition(
        w.seq,
        c2,
        Some(tr(TransitionKind::Dissolve, 1.0, Alignment::Center)),
        LinkMode::Follow,
    )
    .unwrap();
    let (mut r, _) = w.renderer();
    let at = |r: &mut Renderer<Fake>, secs: f64| {
        let s = (secs * f64::from(RATE)) as i64;
        r.audio_samples(s, 1).unwrap()[0]
    };
    assert!(
        (at(&mut r, 3.0) - 0.5).abs() < 1e-6,
        "before: just the outgoing clip"
    );
    assert!(
        (at(&mut r, 5.0) - 0.5).abs() < 1e-6,
        "after: just the incoming clip"
    );
    let mid = at(&mut r, 4.0);
    assert!((mid - 0.5 * std::f32::consts::SQRT_2).abs() < 0.01, "{mid}");
    // Right at the start the outgoing clip is nearly full level, the incoming nearly silent.
    let start = at(&mut r, 3.5);
    assert!((start - 0.5).abs() < 0.01, "{start}");
    // Whole-window reads match reading in pieces.
    let whole = r
        .audio(TimeRange::new(secs(3.0), secs(5.0)).unwrap())
        .unwrap()
        .samples;
    let mut r2 = w.renderer().0;
    let mut parts = Vec::new();
    for i in 0..4 {
        parts.extend(r2.audio_samples(144_000 + i * 24_000, 24_000).unwrap());
    }
    assert_eq!(whole, parts);
}

#[test]
fn offline_media_inside_a_transition_does_not_crash_the_render() {
    use render_engine::MissingMedia;
    struct Gone;
    impl MediaSource for Gone {
        fn video_frame(&mut self, a: &Asset, _: u32, _: f64) -> Result<Arc<VideoFrame>> {
            Err(render_engine::Error::MissingMedia {
                asset: a.id,
                name: a.name.clone(),
                path: a.path.clone(),
            })
        }
        fn audio(&mut self, a: &Asset, _: u32, _: f64, _: usize, _: u32) -> Result<Vec<f32>> {
            Err(render_engine::Error::MissingMedia {
                asset: a.id,
                name: a.name.clone(),
                path: a.path.clone(),
            })
        }
    }
    let mut w = world();
    two_clips(&mut w, tr(TransitionKind::Dissolve, 1.0, Alignment::Center));
    let mut r = Renderer::new(
        w.ed.project(),
        w.seq,
        Gone,
        RenderSettings {
            missing_media: MissingMedia::Offline,
            ..Default::default()
        },
    )
    .unwrap();
    let f = r.frame(96).unwrap();
    assert_eq!(&f.data[..4], &[255, 0, 255, 255]);
    r.audio_samples(0, 48_000 * 5).unwrap();
}
