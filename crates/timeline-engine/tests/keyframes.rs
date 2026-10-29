// SPDX-License-Identifier: GPL-3.0-or-later
//! Keyframes on clips, tracks and the master: gain and pan automation, speed ramps, cutting,
//! saving and the rules that keep them valid.
use timeline_engine::*;

fn secs(s: f64) -> Time {
    Time::from_secs_f64(s)
}

struct Fx {
    ed: Editor,
    seq: SequenceId,
    a1: TrackId,
    v1: TrackId,
    av: AssetId,
}

fn fx() -> Fx {
    let mut ed = Editor::new("t");
    let seq = ed.add_sequence(SequenceSettings::default()).unwrap();
    let s = ed.project().sequence(seq).unwrap();
    let (v1, a1) = (s.video_tracks()[0].id, s.audio_tracks()[0].id);
    let av = ed
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
    Fx {
        ed,
        seq,
        a1,
        v1,
        av,
    }
}

impl Fx {
    fn audio_clip(&mut self, start: f64, dur: f64) -> ClipId {
        let mut spec = ClipSpec::new(self.av, secs(start));
        spec.duration = Some(secs(dur));
        self.ed
            .place_clip(self.seq, self.a1, spec, PlaceMode::Overwrite)
            .unwrap()
    }

    fn video_clip(&mut self, start: f64, dur: f64) -> ClipId {
        let mut spec = ClipSpec::new(self.av, secs(start));
        spec.duration = Some(secs(dur));
        self.ed
            .place_clip(self.seq, self.v1, spec, PlaceMode::Overwrite)
            .unwrap()
    }

    fn pair(&mut self, dur: f64) -> (ClipId, ClipId) {
        let p = self
            .ed
            .place_asset(
                self.seq,
                self.av,
                Time::ZERO,
                Time::ZERO,
                Some(secs(dur)),
                Some(self.v1),
                Some(self.a1),
                PlaceMode::Overwrite,
            )
            .unwrap();
        (p.video.unwrap(), p.audio.unwrap())
    }

    fn get(&self, id: ClipId) -> Clip {
        self.ed
            .project()
            .sequence(self.seq)
            .unwrap()
            .find_clip(id)
            .unwrap()
            .1
            .clone()
    }

    fn key(&mut self, t: Target, p: Param, at: f64, v: f64, i: Interp) -> Result<()> {
        self.ed
            .set_keyframe(self.seq, t, p, secs(at), v, i, LinkMode::Follow)
    }

    fn seq(&self) -> &Sequence {
        self.ed.project().sequence(self.seq).unwrap()
    }
}

#[test]
fn gain_keys_start_from_the_fixed_value_and_follow_the_curve() {
    let mut f = fx();
    let c = f.audio_clip(10.0, 10.0);
    f.ed.set_clip_audio(
        f.seq,
        &[c],
        ClipAudio {
            gain: Gain::from_db(-6.0),
            ..Default::default()
        },
        LinkMode::Follow,
    )
    .unwrap();
    // A single key 4 seconds in: the old -6 dB holds until it, then ramps to 0 dB.
    f.key(Target::Clip(c), Param::Gain, 14.0, 0.0, Interp::Linear)
        .unwrap();
    let clip = f.get(c);
    assert_eq!(clip.anim.get(Param::Gain).unwrap().len(), 2);
    assert_eq!(clip.gain_db_at(secs(10.0)), -6.0);
    assert_eq!(clip.gain_db_at(secs(14.0)), 0.0);
    assert!((clip.gain_db_at(secs(12.0)) + 3.0).abs() < 1e-9);
    assert_eq!(clip.gain_db_at(secs(19.0)), 0.0, "holds the last value");
    // Unanimated values read the fixed fields.
    assert_eq!(clip.pan_at(secs(15.0)), 0.0);
    // Undo takes the keys away again.
    f.ed.undo().unwrap();
    assert!(f.get(c).anim.is_empty());
}

#[test]
fn a_fixed_value_cannot_be_set_while_a_curve_exists() {
    let mut f = fx();
    let c = f.audio_clip(0.0, 10.0);
    f.key(Target::Clip(c), Param::Gain, 5.0, -12.0, Interp::Smooth)
        .unwrap();
    let err =
        f.ed.set_clip_audio(
            f.seq,
            &[c],
            ClipAudio {
                gain: Gain::from_db(-3.0),
                ..Default::default()
            },
            LinkMode::Follow,
        )
        .unwrap_err();
    assert!(matches!(err, Error::Animated { .. }), "{err}");
    // Pan is not animated, so it can still change.
    f.ed.set_clip_audio(
        f.seq,
        &[c],
        ClipAudio {
            pan: Pan::new(20),
            ..Default::default()
        },
        LinkMode::Follow,
    )
    .unwrap();
    f.ed.clear_animation(f.seq, Target::Clip(c), Param::Gain, LinkMode::Follow)
        .unwrap();
    assert!(f.get(c).anim.is_empty());
    assert_eq!(f.get(c).gain, Gain::UNITY);
    assert!(f
        .ed
        .clear_animation(f.seq, Target::Clip(c), Param::Gain, LinkMode::Follow)
        .is_err());
}

#[test]
fn bad_keyframes_are_refused_and_change_nothing() {
    let mut f = fx();
    let a = f.audio_clip(5.0, 5.0);
    let v = f.video_clip(5.0, 5.0);
    let before = f.ed.project().clone();
    for (target, param, at, value) in [
        (Target::Clip(a), Param::Gain, 6.0, 30.0),
        (Target::Clip(a), Param::Gain, 6.0, f64::NAN),
        (Target::Clip(a), Param::Pan, 6.0, 101.0),
        (Target::Clip(a), Param::Speed, 6.0, 0.0),
        (Target::Clip(a), Param::Speed, 6.0, 200.0),
        (Target::Clip(a), Param::Gain, 4.0, 0.0),
        (Target::Clip(a), Param::Gain, 11.0, 0.0),
        (Target::Clip(v), Param::Gain, 6.0, 0.0),
        (Target::Clip(v), Param::Pan, 6.0, 0.0),
        (Target::Master, Param::Pan, 1.0, 0.0),
        (Target::Master, Param::Speed, 1.0, 1.0),
        (Target::Master, Param::Gain, -1.0, 0.0),
    ] {
        let r = f.key(target, param, at, value, Interp::Linear);
        assert!(r.is_err(), "{target:?} {param} at {at} = {value}");
    }
    let vt = f.v1;
    assert!(f
        .key(Target::Track(vt), Param::Gain, 1.0, 0.0, Interp::Linear)
        .is_err());
    assert!(f.ed.project() == &before);
    // A key exactly at the clip's end is fine.
    f.key(Target::Clip(a), Param::Gain, 10.0, -20.0, Interp::Linear)
        .unwrap();
}

#[test]
fn removing_keys_and_the_last_key_ends_the_animation() {
    let mut f = fx();
    let c = f.audio_clip(0.0, 10.0);
    f.key(Target::Clip(c), Param::Pan, 0.0, -100.0, Interp::Linear)
        .unwrap();
    f.key(Target::Clip(c), Param::Pan, 8.0, 100.0, Interp::Linear)
        .unwrap();
    assert!((f.get(c).pan_at(secs(4.0))).abs() < 1e-9);
    f.ed.remove_keyframe(
        f.seq,
        Target::Clip(c),
        Param::Pan,
        secs(8.0),
        LinkMode::Follow,
    )
    .unwrap();
    assert_eq!(f.get(c).pan_at(secs(9.0)), -100.0);
    assert!(f
        .ed
        .remove_keyframe(
            f.seq,
            Target::Clip(c),
            Param::Pan,
            secs(3.0),
            LinkMode::Follow
        )
        .is_err());
    f.ed.remove_keyframe(
        f.seq,
        Target::Clip(c),
        Param::Pan,
        secs(0.0),
        LinkMode::Follow,
    )
    .unwrap();
    assert!(!f.get(c).anim.is_animated(Param::Pan));
    assert_eq!(f.get(c).pan_at(secs(9.0)), 0.0);
}

#[test]
fn cutting_a_clip_keeps_its_automation_exactly() {
    let mut f = fx();
    let c = f.audio_clip(2.0, 12.0);
    for (at, v, i) in [
        (2.0, -30.0, Interp::Smooth),
        (6.0, 0.0, Interp::Smooth),
        (10.0, -6.0, Interp::Linear),
        (14.0, -40.0, Interp::Linear),
    ] {
        f.key(Target::Clip(c), Param::Gain, at, v, i).unwrap();
    }
    let whole = f.get(c);
    let (l, r) =
        f.ed.split_clip(f.seq, c, secs(7.5), LinkMode::Follow)
            .unwrap();
    let (l, r) = (f.get(l), f.get(r));
    for k in 0..=240 {
        let t = secs(2.0 + f64::from(k) * 0.05);
        let want = whole.gain_db_at(t);
        let got = if t < secs(7.5) {
            l.gain_db_at(t)
        } else {
            r.gain_db_at(t)
        };
        assert!(
            (want - got).abs() < 1e-6,
            "t={}: {want} vs {got}",
            t.as_secs_f64()
        );
    }
    // Trimming the head shortens the clip without moving the curve.
    f.ed.trim(
        f.seq,
        r.id,
        Edge::Head,
        secs(1.0),
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    let t = f.get(r.id);
    for k in 0..40 {
        let at = t.start + secs(f64::from(k) * 0.1);
        assert!((whole.gain_db_at(at) - t.gain_db_at(at)).abs() < 1e-6);
    }
    // Moving a clip takes its keys with it (they are relative to the clip).
    let before = t.gain_db_at(t.start + secs(1.0));
    f.ed.move_clips(f.seq, &[r.id], secs(5.0), 0, LinkMode::Follow)
        .unwrap();
    let moved = f.get(r.id);
    assert_eq!(moved.gain_db_at(moved.start + secs(1.0)), before);
}

#[test]
fn speed_ramp_changes_how_much_media_plays() {
    let mut f = fx();
    let c = f.video_clip(0.0, 10.0);
    // 1x for the first 4 s, then a linear ramp to 3x at 8 s, then 3x.
    f.key(Target::Clip(c), Param::Speed, 4.0, 1.0, Interp::Linear)
        .unwrap();
    f.key(Target::Clip(c), Param::Speed, 8.0, 3.0, Interp::Linear)
        .unwrap();
    let clip = f.get(c);
    assert!(clip.is_ramped());
    // 4 s at 1x + the ramp (average 2x for 4 s = 8 s) + 2 s at 3x = 18 s of media.
    assert_eq!(clip.source_len(), secs(18.0));
    assert_eq!(clip.source_time_at(secs(4.0)), secs(4.0));
    assert_eq!(clip.source_time_at(secs(8.0)), secs(12.0));
    assert_eq!(clip.source_time_at(secs(10.0)), secs(18.0));
    assert_eq!(clip.speed_at(secs(6.0)), 2.0);
    // Smooth ramps agree with their own integral at every frame and never run backwards.
    let mut last = Time::ZERO;
    for k in 0..240 {
        let m = clip.source_time_at(secs(f64::from(k) / 24.0));
        assert!(m >= last);
        last = m;
    }
}

#[test]
fn speed_ramps_cannot_run_past_the_media_or_be_mixed_with_a_fixed_speed() {
    let mut f = fx();
    let c = f.video_clip(0.0, 30.0);
    f.key(Target::Clip(c), Param::Speed, 0.0, 1.0, Interp::Linear)
        .unwrap();
    // A ramp from 1x to 3.2x over 30 s plays 63 s of media but the file has 60 s.
    let err = f
        .key(Target::Clip(c), Param::Speed, 30.0, 3.2, Interp::Linear)
        .unwrap_err();
    assert!(err.to_string().contains("media"), "{err}");
    f.key(Target::Clip(c), Param::Speed, 30.0, 2.0, Interp::Hold)
        .unwrap();
    let err =
        f.ed.set_speed(
            f.seq,
            c,
            Speed::new(2, 1).unwrap(),
            SpeedMode::KeepMedia,
            Ripple::Off,
            LinkMode::Follow,
        )
        .unwrap_err();
    assert!(matches!(err, Error::Animated { .. }), "{err}");
    f.ed.clear_animation(f.seq, Target::Clip(c), Param::Speed, LinkMode::Follow)
        .unwrap();
    f.ed.set_speed(
        f.seq,
        c,
        Speed::new(2, 1).unwrap(),
        SpeedMode::KeepMedia,
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
}

#[test]
fn cutting_a_ramped_clip_keeps_the_media_continuous() {
    let mut f = fx();
    let c = f.video_clip(0.0, 12.0);
    f.key(Target::Clip(c), Param::Speed, 0.0, 0.5, Interp::Smooth)
        .unwrap();
    f.key(Target::Clip(c), Param::Speed, 6.0, 4.0, Interp::Smooth)
        .unwrap();
    f.key(Target::Clip(c), Param::Speed, 12.0, 1.0, Interp::Linear)
        .unwrap();
    let whole = f.get(c);
    let (l, r) =
        f.ed.split_clip(f.seq, c, secs(3.5), LinkMode::Follow)
            .unwrap();
    let (l, r) = (f.get(l), f.get(r));
    // The two halves meet in the media (a few ticks of rounding at most)...
    let gap = (r.source_in - (l.source_in + l.source_len())).ticks().abs();
    assert!(gap <= 2, "{gap}");
    // ...and every frame still shows the same moment of media as before the cut.
    for k in 0..288 {
        let t = secs(f64::from(k) / 24.0);
        let want = whole.source_time_at(t);
        let got = if t < secs(3.5) {
            l.source_time_at(t)
        } else {
            r.source_time_at(t)
        };
        assert!((want - got).ticks().abs() <= 4, "t={}", t.as_secs_f64());
    }
}

#[test]
fn reversed_ramps_play_the_mirror_image() {
    let mut f = fx();
    let c = f.video_clip(10.0, 6.0);
    f.key(Target::Clip(c), Param::Speed, 10.0, 1.0, Interp::Linear)
        .unwrap();
    f.key(Target::Clip(c), Param::Speed, 16.0, 3.0, Interp::Linear)
        .unwrap();
    f.ed.set_reverse(f.seq, c, true, LinkMode::Follow).unwrap();
    let clip = f.get(c);
    assert_eq!(clip.source_len(), secs(12.0));
    // Starts at the end of the stretch and runs back to its beginning.
    assert_eq!(clip.source_time_at(clip.start), clip.source_in + secs(12.0));
    assert_eq!(clip.source_time_at(clip.end()), clip.source_in);
    let mut last = clip.source_time_at(clip.start);
    for k in 1..=144 {
        let m = clip.source_time_at(clip.start + secs(f64::from(k) / 24.0));
        assert!(m <= last);
        last = m;
    }
}

#[test]
fn a_speed_ramp_follows_links_so_picture_and_sound_stay_together() {
    let mut f = fx();
    let (v, a) = f.pair(10.0);
    f.key(Target::Clip(v), Param::Speed, 5.0, 2.0, Interp::Linear)
        .unwrap();
    assert!(f.get(a).is_ramped());
    assert_eq!(f.get(a).source_len(), f.get(v).source_len());
    // Unlinked: only the one clip changes.
    let (v2, a2) = {
        f.ed.unlink(f.seq, &[v, a]).unwrap();
        (v, a)
    };
    f.ed.clear_animation(f.seq, Target::Clip(v2), Param::Speed, LinkMode::Ignore)
        .unwrap();
    assert!(f.get(a2).is_ramped() && !f.get(v2).is_ramped());
}

#[test]
fn track_and_master_automation_use_sequence_time() {
    let mut f = fx();
    let a1 = f.a1;
    f.key(Target::Track(a1), Param::Gain, 10.0, -20.0, Interp::Linear)
        .unwrap();
    f.key(Target::Master, Param::Gain, 5.0, -10.0, Interp::Smooth)
        .unwrap();
    let s = f.seq();
    let t = s.track(a1).unwrap();
    assert_eq!(t.gain_db_at(secs(0.0)), 0.0);
    assert_eq!(t.gain_db_at(secs(10.0)), -20.0);
    assert!((t.gain_db_at(secs(5.0)) + 10.0).abs() < 1e-9);
    assert_eq!(s.master_gain_db_at(secs(100.0)), -10.0);
    // The fixed values are locked out while animated.
    let err =
        f.ed.update_track(
            f.seq,
            a1,
            TrackUpdate {
                gain: Gain::from_db(-3.0),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(err, Error::Animated { .. }));
    assert!(matches!(
        f.ed.set_master_gain(f.seq, Gain::UNITY).unwrap_err(),
        Error::Animated { .. }
    ));
    // Moving clips around does not touch the track's curve.
    let c = f.audio_clip(0.0, 4.0);
    f.ed.move_clips(f.seq, &[c], secs(3.0), 0, LinkMode::Follow)
        .unwrap();
    assert_eq!(f.seq().track(a1).unwrap().gain_db_at(secs(10.0)), -20.0);
}

#[test]
fn animation_survives_saving_and_loading() {
    let mut f = fx();
    let (v, a) = f.pair(10.0);
    f.key(Target::Clip(v), Param::Speed, 2.0, 1.5, Interp::Smooth)
        .unwrap();
    f.key(Target::Clip(v), Param::Speed, 9.0, 0.5, Interp::Hold)
        .unwrap();
    f.key(Target::Clip(a), Param::Gain, 3.0, -7.25, Interp::Smooth)
        .unwrap();
    f.key(Target::Clip(a), Param::Pan, 6.0, 40.0, Interp::Linear)
        .unwrap();
    let a1 = f.a1;
    f.key(Target::Track(a1), Param::Pan, 4.0, -50.0, Interp::Linear)
        .unwrap();
    f.key(Target::Master, Param::Gain, 8.0, -3.0, Interp::Linear)
        .unwrap();
    // Cut once so a frozen tangent is in the file too.
    f.ed.split_clip(f.seq, a, secs(5.0), LinkMode::Ignore)
        .unwrap();
    let text = f.ed.project().to_json().unwrap();
    let back = Project::from_json(&text).unwrap();
    assert!(&back == f.ed.project());
    assert!(text.contains("\"version\": 4"));
    // A project with no animation writes none of it.
    let plain = Editor::new("p");
    assert!(!plain.project().to_json().unwrap().contains("anim"));
}

#[test]
fn damaged_animation_is_refused_on_load() {
    let mut f = fx();
    let a = f.audio_clip(0.0, 10.0);
    f.key(Target::Clip(a), Param::Gain, 2.0, -3.0, Interp::Linear)
        .unwrap();
    f.key(Target::Clip(a), Param::Gain, 4.0, -9.0, Interp::Linear)
        .unwrap();
    let text = f.ed.project().to_json().unwrap();
    assert!(Project::from_json(&text).is_ok());
    let t0 = secs(2.0).ticks().to_string();
    let t1 = secs(4.0).ticks().to_string();
    // Keys out of order.
    let swapped = text
        .replacen(&format!("\"t\": {t0}"), "\"t\": TMP", 1)
        .replacen(&format!("\"t\": {t1}"), &format!("\"t\": {t0}"), 1)
        .replacen("\"t\": TMP", &format!("\"t\": {t1}"), 1);
    assert!(Project::from_json(&swapped).is_err());
    // Value out of range, unknown names, key beyond the clip.
    assert!(Project::from_json(&text.replace("-300.0", "300.0").replace("-3.0", "30.0")).is_err());
    assert!(Project::from_json(&text.replace("\"gain\"", "\"wobble\"")).is_err());
    assert!(Project::from_json(&text.replace("\"linear\"", "\"bouncy\"")).is_err());
    let far = secs(99.0).ticks().to_string();
    assert!(Project::from_json(&text.replacen(
        &format!("\"t\": {t1}"),
        &format!("\"t\": {far}"),
        1
    ))
    .is_err());
    // A speed curve on a picture clip is fine, but one on a clip that would then run past
    // its media is not.
    let mut g = fx();
    let v = g.video_clip(0.0, 30.0);
    g.key(Target::Clip(v), Param::Speed, 0.0, 1.0, Interp::Hold)
        .unwrap();
    let t = g.ed.project().to_json().unwrap();
    assert!(Project::from_json(&t.replace("\"v\": 1.0", "\"v\": 3.0")).is_err());
}

#[test]
fn files_from_before_keyframes_still_load() {
    let mut f = fx();
    f.audio_clip(0.0, 5.0);
    let text =
        f.ed.project()
            .to_json()
            .unwrap()
            .replace("\"version\": 4", "\"version\": 3");
    let p = Project::from_json(&text).unwrap();
    assert!(p
        .sequence(f.seq)
        .unwrap()
        .tracks()
        .all(|t| t.anim.is_empty()));
}
