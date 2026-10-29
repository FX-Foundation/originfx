// SPDX-License-Identifier: GPL-3.0-or-later
//! Gain, pan, fades, track and master levels and the limiter, checked on exact numbers.
use std::sync::Arc;

use media_engine::VideoFrame;
use render_engine::{Limiter, MediaSource, RenderSettings, Renderer, Result};
use timeline_engine::{
    Asset, AssetId, AudioProps, ClipAudio, ClipId, ClipSpec, Editor, Fade, FadeCurve, FrameRate,
    Gain, LinkMode, NewAsset, Pan, PlaceMode, SequenceId, SequenceSettings, Time, TrackUpdate,
    VideoProps,
};

const RATE: i64 = 48_000;

/// Every sample of an asset is its fixed level, in both channels.
struct Flat(f32);

impl MediaSource for Flat {
    fn video_frame(&mut self, _: &Asset, _: u32, secs: f64) -> Result<Arc<VideoFrame>> {
        Ok(Arc::new(VideoFrame {
            width: 2,
            height: 2,
            pts_secs: secs,
            data: vec![0; 16],
        }))
    }

    fn audio(&mut self, _: &Asset, _: u32, _: f64, frames: usize, _: u32) -> Result<Vec<f32>> {
        Ok(vec![self.0; frames * 2])
    }
}

struct W {
    ed: Editor,
    seq: SequenceId,
    asset: AssetId,
}

fn secs(s: f64) -> Time {
    Time::from_secs_f64(s)
}

fn world() -> W {
    let mut ed = Editor::new("t");
    let seq = ed.add_sequence(SequenceSettings::default()).unwrap();
    let asset = ed
        .add_asset(NewAsset {
            name: "x".into(),
            path: "/x.wav".into(),
            duration: Some(secs(60.0)),
            video: vec![VideoProps {
                width: 2,
                height: 2,
                frame_rate: Some(FrameRate::FPS_24),
            }],
            audio: vec![AudioProps {
                sample_rate: 48_000,
                channels: 2,
            }],
        })
        .unwrap();
    W { ed, seq, asset }
}

impl W {
    fn track(&self, n: usize) -> timeline_engine::TrackId {
        self.ed.project().sequence(self.seq).unwrap().audio_tracks()[n].id
    }

    fn clip(&mut self, track: usize, start: f64, dur: f64) -> ClipId {
        let mut s = ClipSpec::new(self.asset, secs(start));
        s.duration = Some(secs(dur));
        let t = self.track(track);
        self.ed
            .place_clip(self.seq, t, s, PlaceMode::Overwrite)
            .unwrap()
    }

    fn set(&mut self, clip: ClipId, a: ClipAudio) {
        self.ed
            .set_clip_audio(self.seq, &[clip], a, LinkMode::Follow)
            .unwrap();
    }

    fn render(&self, level: f32, limiter: Limiter) -> Renderer<Flat> {
        Renderer::new(
            self.ed.project(),
            self.seq,
            Flat(level),
            RenderSettings {
                limiter,
                ..Default::default()
            },
        )
        .unwrap()
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn clip_gain_scales_the_level() {
    let mut w = world();
    let c = w.clip(0, 0.0, 2.0);
    w.set(
        c,
        ClipAudio {
            gain: Gain::from_db(-6.0),
            ..Default::default()
        },
    );
    let out = w
        .render(0.5, Limiter::HardClip)
        .audio_samples(100, 10)
        .unwrap();
    let want = 0.5 * Gain::from_db(-6.0).unwrap().linear();
    assert!(out.iter().all(|s| close(*s, want)), "{out:?} vs {want}");
    // And a boost is not cut down below full scale.
    w.set(
        c,
        ClipAudio {
            gain: Gain::from_db(6.0),
            ..Default::default()
        },
    );
    let out = w
        .render(0.25, Limiter::HardClip)
        .audio_samples(100, 4)
        .unwrap();
    assert!(close(out[0], 0.25 * Gain::from_db(6.0).unwrap().linear()));
}

#[test]
fn pan_silences_the_far_channel() {
    let mut w = world();
    let c = w.clip(0, 0.0, 2.0);
    w.set(
        c,
        ClipAudio {
            pan: Pan::new(-100),
            ..Default::default()
        },
    );
    let out = w
        .render(0.5, Limiter::HardClip)
        .audio_samples(0, 8)
        .unwrap();
    for f in out.chunks(2) {
        assert!(close(f[0], 0.5) && f[1].abs() < 1e-6, "{f:?}");
    }
    w.set(
        c,
        ClipAudio {
            pan: Pan::new(50),
            ..Default::default()
        },
    );
    let out = w
        .render(0.5, Limiter::HardClip)
        .audio_samples(0, 2)
        .unwrap();
    assert!(close(out[1], 0.5) && out[0] > 0.0 && out[0] < 0.5);
}

#[test]
fn fades_shape_the_edges_and_leave_the_middle_alone() {
    let mut w = world();
    let c = w.clip(0, 1.0, 4.0);
    let fade = |d, curve| {
        Some(Some(Fade {
            duration: secs(d),
            curve,
        }))
    };
    w.set(
        c,
        ClipAudio {
            fade_in: fade(1.0, FadeCurve::Linear),
            fade_out: fade(2.0, FadeCurve::Smooth),
            ..Default::default()
        },
    );
    let mut r = w.render(0.8, Limiter::HardClip);
    let at = |r: &mut Renderer<Flat>, s: i64| r.audio_samples(s, 1).unwrap()[0];
    // Clip spans samples 48000..240000. Linear fade in over the first second.
    assert!(at(&mut r, RATE - 1).abs() < 1e-9, "silence before the clip");
    assert!(at(&mut r, RATE).abs() < 0.001, "starts from nothing");
    assert!(
        close(at(&mut r, RATE + RATE / 2), 0.4),
        "half way up is half level"
    );
    assert!(
        close(at(&mut r, RATE * 2 + 5), 0.8),
        "full level after the fade in"
    );
    assert!(close(at(&mut r, RATE * 3), 0.8), "untouched in the middle");
    // Smooth fade out over the last two seconds: half way down at 4 s.
    assert!(close(at(&mut r, RATE * 4), 0.4));
    assert!(at(&mut r, RATE * 5 - 1) < 0.001, "ends at nothing");
    // Reading in pieces equals reading at once.
    let whole = r.audio_samples(RATE, (RATE * 4) as usize).unwrap();
    let mut r2 = w.render(0.8, Limiter::HardClip);
    let mut parts = Vec::new();
    let mut s = RATE;
    for n in [1000, 47_000, 100_001, 43_999] {
        parts.extend(r2.audio_samples(s, n as usize).unwrap());
        s += n;
    }
    assert_eq!(s, RATE * 4 + RATE);
    assert_eq!(parts.len(), whole.len());
    assert_eq!(parts, whole);
}

#[test]
fn track_and_master_levels_multiply() {
    let mut w = world();
    w.clip(0, 0.0, 2.0);
    let t = w.track(0);
    w.ed.update_track(
        w.seq,
        t,
        TrackUpdate {
            gain: Gain::from_db(-6.0),
            ..Default::default()
        },
    )
    .unwrap();
    w.ed.set_master_gain(w.seq, Gain::from_db(-6.0).unwrap())
        .unwrap();
    let out = w
        .render(0.8, Limiter::HardClip)
        .audio_samples(0, 2)
        .unwrap();
    let g = Gain::from_db(-6.0).unwrap().linear();
    assert!(close(out[0], 0.8 * g * g), "{}", out[0]);
    // A silent track is silent.
    w.ed.update_track(
        w.seq,
        t,
        TrackUpdate {
            gain: Some(Gain::SILENT),
            ..Default::default()
        },
    )
    .unwrap();
    let out = w
        .render(0.8, Limiter::HardClip)
        .audio_samples(0, 2)
        .unwrap();
    assert_eq!(out, vec![0.0; 4]);
}

#[test]
fn track_pan_applies_to_the_whole_track() {
    let mut w = world();
    w.clip(0, 0.0, 2.0);
    let t = w.track(0);
    w.ed.update_track(
        w.seq,
        t,
        TrackUpdate {
            pan: Pan::new(100),
            ..Default::default()
        },
    )
    .unwrap();
    let out = w
        .render(0.5, Limiter::HardClip)
        .audio_samples(0, 2)
        .unwrap();
    assert!(out[0].abs() < 1e-6 && close(out[1], 0.5));
}

#[test]
fn limiters_keep_a_hot_mix_inside_full_scale() {
    let mut w = world();
    w.clip(0, 0.0, 2.0);
    w.clip(1, 0.0, 2.0);
    let hard = w
        .render(0.8, Limiter::HardClip)
        .audio_samples(0, 4)
        .unwrap();
    assert_eq!(hard, vec![1.0; 8], "1.6 is clipped flat");
    let soft = w
        .render(0.8, Limiter::SoftClip)
        .audio_samples(0, 4)
        .unwrap();
    assert!(soft.iter().all(|s| *s > 0.95 && *s < 1.0), "{soft:?}");
    // Below the knee the soft limiter changes nothing.
    let quiet = w
        .render(0.3, Limiter::SoftClip)
        .audio_samples(0, 2)
        .unwrap();
    assert_eq!(quiet[0], 0.6);
}

#[test]
fn neutral_settings_change_no_bits() {
    let mut w = world();
    w.clip(0, 0.0, 2.0);
    let a = w
        .render(0.123_456_79, Limiter::HardClip)
        .audio_samples(0, 50)
        .unwrap();
    assert!(a.iter().all(|s| *s == 0.123_456_79));
}
