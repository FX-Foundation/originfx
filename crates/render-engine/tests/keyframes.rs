// SPDX-License-Identifier: GPL-3.0-or-later
//! Keyframed gain, pan and speed, checked on exact numbers with a source whose sound says where in
//! the media it came from.
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use media_engine::VideoFrame;
use render_engine::{Limiter, MediaSource, RenderSettings, Renderer, Result};
use timeline_engine::{
    Asset, AssetId, AudioProps, ClipId, ClipSpec, Editor, FrameRate, Interp, LinkMode, NewAsset,
    Param, PlaceMode, SequenceId, SequenceSettings, Target, Time, TrackId, VideoProps,
};

const RATE: u32 = 48_000;

/// Sound: every sample is its own media sample number divided by a million, or a fixed level.
/// Pictures: flat grey. Every picture request is noted.
struct Fake {
    level: Option<f32>,
    asked: Rc<RefCell<Vec<f64>>>,
}

impl MediaSource for Fake {
    fn video_frame(&mut self, _: &Asset, _: u32, secs: f64) -> Result<Arc<VideoFrame>> {
        self.asked.borrow_mut().push(secs);
        Ok(Arc::new(VideoFrame {
            width: 16,
            height: 9,
            pts_secs: secs,
            data: vec![128; 16 * 9 * 4],
        }))
    }

    fn audio(&mut self, _: &Asset, _: u32, secs: f64, frames: usize, _: u32) -> Result<Vec<f32>> {
        if let Some(l) = self.level {
            return Ok(vec![l; frames * 2]);
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

fn secs(s: f64) -> Time {
    Time::from_secs_f64(s)
}

struct W {
    ed: Editor,
    seq: SequenceId,
    asset: AssetId,
    asked: Rc<RefCell<Vec<f64>>>,
}

fn world() -> W {
    let mut ed = Editor::new("t");
    let seq = ed
        .add_sequence(SequenceSettings {
            width: 16,
            height: 9,
            ..Default::default()
        })
        .unwrap();
    let asset = ed
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
    W {
        ed,
        seq,
        asset,
        asked: Rc::default(),
    }
}

impl W {
    fn a1(&self) -> TrackId {
        self.ed.project().sequence(self.seq).unwrap().audio_tracks()[0].id
    }

    fn v1(&self) -> TrackId {
        self.ed.project().sequence(self.seq).unwrap().video_tracks()[0].id
    }

    fn audio(&mut self, start: f64, dur: f64, source_in: f64) -> ClipId {
        let mut s = ClipSpec::new(self.asset, secs(start));
        s.duration = Some(secs(dur));
        s.source_in = secs(source_in);
        let t = self.a1();
        self.ed
            .place_clip(self.seq, t, s, PlaceMode::Overwrite)
            .unwrap()
    }

    fn video(&mut self, start: f64, dur: f64) -> ClipId {
        let mut s = ClipSpec::new(self.asset, secs(start));
        s.duration = Some(secs(dur));
        let t = self.v1();
        self.ed
            .place_clip(self.seq, t, s, PlaceMode::Overwrite)
            .unwrap()
    }

    fn key(&mut self, t: Target, p: Param, at: f64, v: f64, i: Interp) {
        self.ed
            .set_keyframe(self.seq, t, p, secs(at), v, i, LinkMode::Follow)
            .unwrap();
    }

    fn renderer(&self, level: Option<f32>) -> Renderer<Fake> {
        Renderer::new(
            self.ed.project(),
            self.seq,
            Fake {
                level,
                asked: self.asked.clone(),
            },
            RenderSettings {
                limiter: Limiter::Off,
                ..Default::default()
            },
        )
        .unwrap()
    }
}

fn sample_at(t: f64) -> i64 {
    (t * f64::from(RATE)).round() as i64
}

fn db(x: f64) -> f32 {
    10f64.powf(x / 20.0) as f32
}

#[test]
fn gain_automation_follows_the_curve_sample_by_sample() {
    let mut w = world();
    let c = w.audio(1.0, 4.0, 0.0);
    // 0 dB for the first second of the clip, then down to -20 dB at its end.
    w.key(Target::Clip(c), Param::Gain, 2.0, 0.0, Interp::Linear);
    w.key(Target::Clip(c), Param::Gain, 5.0, -20.0, Interp::Linear);
    let mut r = w.renderer(Some(1.0));
    for t in [1.0, 1.9, 2.0, 2.5, 3.5, 4.99] {
        let out = r.audio_samples(sample_at(t), 4).unwrap();
        let want_db = if t < 2.0 {
            0.0
        } else {
            -20.0 * (t - 2.0) / 3.0
        };
        assert!(
            (out[0] - db(want_db)).abs() < 2e-3,
            "t={t}: {} vs {}",
            out[0],
            db(want_db)
        );
        assert_eq!(out[0], out[1]);
    }
    // Before the clip and after it there is nothing.
    assert!(r
        .audio_samples(sample_at(0.5), 8)
        .unwrap()
        .iter()
        .all(|s| *s == 0.0));
    // The level really moves within a single call, not just between calls.
    let out = r.audio_samples(sample_at(2.0), 48_000).unwrap();
    assert!(out[0] > out[2 * 24_000] && out[2 * 24_000] > out[2 * 47_999]);
}

#[test]
fn pan_automation_sweeps_across_the_picture() {
    let mut w = world();
    let c = w.audio(0.0, 2.0, 0.0);
    w.key(Target::Clip(c), Param::Pan, 0.0, -100.0, Interp::Linear);
    w.key(Target::Clip(c), Param::Pan, 2.0, 100.0, Interp::Linear);
    let mut r = w.renderer(Some(1.0));
    let start = r.audio_samples(0, 2).unwrap();
    assert!(
        (start[0] - 1.0).abs() < 1e-3 && start[1].abs() < 1e-3,
        "{start:?}"
    );
    let mid = r.audio_samples(sample_at(1.0), 2).unwrap();
    assert!(
        (mid[0] - 1.0).abs() < 1e-3 && (mid[1] - 1.0).abs() < 1e-3,
        "{mid:?}"
    );
    let end = r.audio_samples(sample_at(2.0) - 2, 2).unwrap();
    assert!(
        end[0].abs() < 1e-3 && (end[1] - 1.0).abs() < 1e-3,
        "{end:?}"
    );
}

#[test]
fn track_and_master_automation_are_in_sequence_time() {
    let mut w = world();
    w.audio(2.0, 6.0, 0.0);
    let a1 = w.a1();
    // The track fades from 0 to -12 dB between seconds 2 and 6; the master drops 6 dB at 7 s.
    w.key(Target::Track(a1), Param::Gain, 2.0, 0.0, Interp::Linear);
    w.key(Target::Track(a1), Param::Gain, 6.0, -12.0, Interp::Linear);
    w.key(Target::Master, Param::Gain, 7.0, 0.0, Interp::Hold);
    w.key(Target::Master, Param::Gain, 7.5, -6.0, Interp::Hold);
    let mut r = w.renderer(Some(1.0));
    let at = |r: &mut Renderer<Fake>, t: f64| r.audio_samples(sample_at(t), 1).unwrap()[0];
    assert!((at(&mut r, 2.0) - 1.0).abs() < 1e-3);
    assert!((at(&mut r, 4.0) - db(-6.0)).abs() < 2e-3);
    assert!((at(&mut r, 6.5) - db(-12.0)).abs() < 2e-3);
    assert!((at(&mut r, 7.2) - db(-12.0)).abs() < 2e-3);
    assert!((at(&mut r, 7.8) - db(-18.0)).abs() < 2e-3);
}

#[test]
fn a_speed_ramp_plays_the_right_media_at_every_moment() {
    let mut w = world();
    // 4 s of timeline, starting 10 s into the media, ramping from 1x to 3x. The speed at time t
    // is 1 + t/2, so the media played is t + t^2/4.
    let c = w.audio(0.0, 4.0, 10.0);
    w.key(Target::Clip(c), Param::Speed, 0.0, 1.0, Interp::Linear);
    w.key(Target::Clip(c), Param::Speed, 4.0, 3.0, Interp::Linear);
    let mut r = w.renderer(None);
    for t in [0.0, 0.5, 1.0, 2.0, 3.0, 3.9] {
        let out = r.audio_samples(sample_at(t), 3).unwrap();
        for (j, pair) in out.chunks_exact(2).enumerate() {
            let tt = sample_at(t) as f64 / f64::from(RATE) + j as f64 / f64::from(RATE);
            let media = 10.0 + tt + tt * tt / 4.0;
            let want = (media * f64::from(RATE)) / 1_000_000.0;
            assert!(
                (f64::from(pair[0]) - want).abs() < 5e-5,
                "t={tt}: {} vs {want}",
                pair[0]
            );
        }
    }
}

#[test]
fn a_reversed_speed_ramp_runs_backwards_to_the_start_of_the_media() {
    let mut w = world();
    let c = w.audio(0.0, 4.0, 10.0);
    w.key(Target::Clip(c), Param::Speed, 0.0, 1.0, Interp::Linear);
    w.key(Target::Clip(c), Param::Speed, 4.0, 3.0, Interp::Linear);
    w.ed.set_reverse(w.seq, c, true, LinkMode::Follow).unwrap();
    let mut r = w.renderer(None);
    let out = r.audio_samples(0, sample_at(4.0) as usize).unwrap();
    let left: Vec<f32> = out.chunks_exact(2).map(|p| p[0]).collect();
    assert!(left.windows(2).all(|p| p[1] <= p[0] + 1e-9), "not falling");
    // 4 s ramping 1x to 3x plays 8 s of media, starting 10 s in: it shows 10 s to 18 s.
    let top = (18.0 * f64::from(RATE)) / 1_000_000.0;
    let bottom = (10.0 * f64::from(RATE)) / 1_000_000.0;
    assert!(
        (f64::from(left[0]) - top).abs() < 1e-3,
        "{} vs {top}",
        left[0]
    );
    assert!((f64::from(*left.last().unwrap()) - bottom).abs() < 1e-3);
}

#[test]
fn cutting_a_ramped_clip_does_not_change_what_is_heard() {
    let mut w = world();
    let c = w.audio(0.0, 6.0, 5.0);
    w.key(Target::Clip(c), Param::Speed, 0.0, 0.5, Interp::Smooth);
    w.key(Target::Clip(c), Param::Speed, 3.0, 2.5, Interp::Smooth);
    w.key(Target::Clip(c), Param::Speed, 6.0, 1.0, Interp::Linear);
    w.key(Target::Clip(c), Param::Gain, 0.0, -9.0, Interp::Smooth);
    w.key(Target::Clip(c), Param::Gain, 6.0, 0.0, Interp::Smooth);
    let n = sample_at(6.0) as usize;
    let whole = w.renderer(None).audio_samples(0, n).unwrap();
    w.ed.split_clip(w.seq, c, secs(2.5), LinkMode::Follow)
        .unwrap();
    let cut = w.renderer(None).audio_samples(0, n).unwrap();
    let worst = whole
        .iter()
        .zip(&cut)
        .map(|(a, b)| (a - b).abs())
        .fold(0f32, f32::max);
    assert!(worst < 5e-5, "{worst}");
}

#[test]
fn picture_follows_a_speed_ramp() {
    let mut w = world();
    let c = w.video(0.0, 4.0);
    w.key(Target::Clip(c), Param::Speed, 0.0, 1.0, Interp::Linear);
    w.key(Target::Clip(c), Param::Speed, 4.0, 3.0, Interp::Linear);
    let clip =
        w.ed.project()
            .sequence(w.seq)
            .unwrap()
            .find_clip(c)
            .unwrap()
            .1
            .clone();
    let mut r = w.renderer(None);
    for k in [0i64, 12, 48, 70, 95] {
        w.asked.borrow_mut().clear();
        r.frame(k).unwrap();
        let t = Time::from_frames(k, FrameRate::FPS_24);
        let want = clip.frame_source_time(t, Time::from_frames(1, FrameRate::FPS_24));
        let asked = w.asked.borrow().clone();
        assert!(
            asked
                .iter()
                .any(|a| (a - want.as_secs_f64()).abs() < 0.5 / 24.0),
            "frame {k}: asked {asked:?}, wanted {}",
            want.as_secs_f64()
        );
    }
    // Later frames reach further into the media than steady 1x would.
    let late = clip.source_time_at(secs(3.9)).as_secs_f64();
    assert!(late > 3.9 * 1.9);
}

#[test]
fn without_keyframes_nothing_changes() {
    let mut w = world();
    w.audio(0.0, 2.0, 3.0);
    let out = w.renderer(None).audio_samples(100, 4).unwrap();
    for (j, pair) in out.chunks_exact(2).enumerate() {
        let want = (3 * RATE as i64 + 100 + j as i64) as f32 / 1_000_000.0;
        assert_eq!(pair[0], want);
    }
}
