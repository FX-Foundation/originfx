// SPDX-License-Identifier: GPL-3.0-or-later
use timeline_engine::*;

fn secs(s: f64) -> Time {
    Time::from_secs_f64(s)
}

fn fr(n: i64) -> Time {
    Time::from_frames(n, FrameRate::FPS_24)
}

struct Fx {
    ed: Editor,
    seq: SequenceId,
    v1: TrackId,
    a1: TrackId,
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
            duration: Some(secs(20.0)),
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
        v1,
        a1,
        av,
    }
}

impl Fx {
    fn clip(&mut self, start: f64, source_in: f64, dur: f64) -> ClipId {
        let mut spec = ClipSpec::new(self.av, secs(start));
        spec.source_in = secs(source_in);
        spec.duration = Some(secs(dur));
        self.ed
            .place_clip(self.seq, self.v1, spec, PlaceMode::Overwrite)
            .unwrap()
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

    fn v1_clips(&self) -> Vec<Clip> {
        self.ed.project().sequence(self.seq).unwrap().video_tracks()[0]
            .clips()
            .cloned()
            .collect()
    }
}

fn dissolve(d: f64, alignment: Alignment) -> Transition {
    Transition {
        kind: TransitionKind::Dissolve,
        duration: secs(d),
        alignment,
    }
}

#[test]
fn speed_keeps_media_and_changes_length() {
    let mut f = fx();
    let c = f.clip(0.0, 2.0, 10.0);
    f.ed.set_speed(
        f.seq,
        c,
        Speed::new(2, 1).unwrap(),
        SpeedMode::KeepMedia,
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    let c = f.get(c);
    assert_eq!(c.duration, secs(5.0));
    assert_eq!(c.source_in, secs(2.0));
    assert_eq!(c.source_len(), secs(10.0));
    // Half way through the clip is 5 s into the media stretch.
    assert_eq!(c.source_time_at(secs(2.5)), secs(7.0));
}

#[test]
fn speed_can_keep_length_instead() {
    let mut f = fx();
    let c = f.clip(0.0, 0.0, 10.0);
    f.ed.set_speed(
        f.seq,
        c,
        Speed::new(1, 2).unwrap(),
        SpeedMode::KeepLength,
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    let c = f.get(c);
    assert_eq!(c.duration, secs(10.0));
    assert_eq!(c.source_len(), secs(5.0));
    // Too fast to fit in the file: 10 s at 4x needs 40 s of a 20 s file.
    let err =
        f.ed.set_speed(
            f.seq,
            c.id,
            Speed::new(4, 1).unwrap(),
            SpeedMode::KeepLength,
            Ripple::Off,
            LinkMode::Follow,
        )
        .unwrap_err();
    assert!(matches!(err, Error::OutsideSource { .. }));
}

#[test]
fn slowing_down_needs_room_or_ripple() {
    let mut f = fx();
    let a = f.clip(0.0, 0.0, 4.0);
    let b = f.clip(4.0, 5.0, 4.0);
    let half = Speed::new(1, 2).unwrap();
    let err =
        f.ed.set_speed(
            f.seq,
            a,
            half,
            SpeedMode::KeepMedia,
            Ripple::Off,
            LinkMode::Follow,
        )
        .unwrap_err();
    assert!(matches!(err, Error::Collision { .. }));
    assert_eq!(f.get(a).duration, secs(4.0), "failed edits change nothing");
    f.ed.set_speed(
        f.seq,
        a,
        half,
        SpeedMode::KeepMedia,
        Ripple::Track,
        LinkMode::Follow,
    )
    .unwrap();
    assert_eq!(f.get(a).duration, secs(8.0));
    assert_eq!(f.get(b).start, secs(8.0));
    f.ed.undo().unwrap();
    assert_eq!(f.get(b).start, secs(4.0));
}

#[test]
fn linked_audio_follows_speed() {
    let mut f = fx();
    let p =
        f.ed.place_asset(
            f.seq,
            f.av,
            Time::ZERO,
            Time::ZERO,
            Some(secs(6.0)),
            Some(f.v1),
            Some(f.a1),
            PlaceMode::Overwrite,
        )
        .unwrap();
    f.ed.set_speed(
        f.seq,
        p.video.unwrap(),
        Speed::new(3, 2).unwrap(),
        SpeedMode::KeepMedia,
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    let (v, a) = (f.get(p.video.unwrap()), f.get(p.audio.unwrap()));
    assert_eq!(v.duration, secs(4.0));
    assert_eq!(a.duration, v.duration);
    assert_eq!(a.speed, v.speed);
}

#[test]
fn reverse_mirrors_time() {
    let mut f = fx();
    let c = f.clip(1.0, 3.0, 4.0);
    f.ed.set_reverse(f.seq, c, true, LinkMode::Follow).unwrap();
    let c = f.get(c);
    assert!(c.reverse);
    assert_eq!(c.source_range().start, secs(3.0));
    assert_eq!(c.source_range().end, secs(7.0));
    // The first frame shown is the last frame of the stretch.
    let frame = fr(1);
    assert_eq!(c.frame_source_time(secs(1.0), frame), secs(7.0) - frame);
    // The last frame shown is the first of the stretch.
    assert_eq!(c.frame_source_time(secs(5.0) - frame, frame), secs(3.0));
}

#[test]
fn split_reversed_clip_keeps_the_right_media_on_each_side() {
    let mut f = fx();
    let c = f.clip(0.0, 10.0, 8.0);
    f.ed.set_reverse(f.seq, c, true, LinkMode::Follow).unwrap();
    let (l, r) =
        f.ed.split_clip(f.seq, c, secs(3.0), LinkMode::Follow)
            .unwrap();
    let (l, r) = (f.get(l), f.get(r));
    // Playing back from 18 s down to 10 s: the first 3 s cover 18..15, the rest 15..10.
    assert_eq!(l.source_range().start, secs(15.0));
    assert_eq!(l.source_range().end, secs(18.0));
    assert_eq!(r.source_range().start, secs(10.0));
    assert_eq!(r.source_range().end, secs(15.0));
    assert!(l.reverse && r.reverse);
}

#[test]
fn split_sped_up_clip_scales_the_cut() {
    let mut f = fx();
    let c = f.clip(0.0, 1.0, 5.0);
    f.ed.set_speed(
        f.seq,
        c,
        Speed::new(2, 1).unwrap(),
        SpeedMode::KeepLength,
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    let (_, r) =
        f.ed.split_clip(f.seq, c, secs(2.0), LinkMode::Follow)
            .unwrap();
    let r = f.get(r);
    assert_eq!(
        r.source_in,
        secs(5.0),
        "2 s on the timeline is 4 s of media"
    );
    assert_eq!(r.duration, secs(3.0));
}

#[test]
fn trimming_a_reversed_clip_cuts_the_right_end_of_the_media() {
    let mut f = fx();
    let c = f.clip(0.0, 10.0, 8.0);
    f.ed.set_reverse(f.seq, c, true, LinkMode::Follow).unwrap();
    // Cutting the head of a reversed clip removes media from the *end* of its stretch.
    f.ed.trim(
        f.seq,
        c,
        Edge::Head,
        secs(2.0),
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    let t = f.get(c);
    assert_eq!(t.source_range().start, secs(10.0));
    assert_eq!(t.source_range().end, secs(16.0));
    // Cutting the tail removes media from the start of the stretch.
    f.ed.trim(
        f.seq,
        c,
        Edge::Tail,
        secs(-2.0),
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    let t = f.get(c);
    assert_eq!(t.source_range().start, secs(12.0));
    assert_eq!(t.source_range().end, secs(16.0));
}

#[test]
fn overwrite_through_a_retimed_clip_keeps_both_halves_right() {
    let mut f = fx();
    let c = f.clip(0.0, 0.0, 10.0);
    f.ed.set_speed(
        f.seq,
        c,
        Speed::new(2, 1).unwrap(),
        SpeedMode::KeepLength,
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    // Punch a hole from 4 to 6 s.
    let mut spec = ClipSpec::new(f.av, secs(4.0));
    spec.duration = Some(secs(2.0));
    f.ed.place_clip(f.seq, f.v1, spec, PlaceMode::Overwrite)
        .unwrap();
    let clips = f.v1_clips();
    assert_eq!(clips.len(), 3);
    assert_eq!(clips[0].source_range().end, secs(8.0));
    assert_eq!(clips[2].source_in, secs(12.0));
    assert_eq!(clips[2].speed, Speed::new(2, 1).unwrap());
}

#[test]
fn placing_with_speed_uses_the_rest_of_the_file() {
    let mut f = fx();
    let mut spec = ClipSpec::new(f.av, Time::ZERO);
    spec.source_in = secs(4.0);
    spec.speed = Speed::new(2, 1).unwrap();
    let id =
        f.ed.place_clip(f.seq, f.v1, spec, PlaceMode::Overwrite)
            .unwrap();
    // 16 s of media left at 2x is 8 s of timeline.
    assert_eq!(f.get(id).duration, secs(8.0));
}

#[test]
fn transitions_need_touching_clips_and_spare_media() {
    let mut f = fx();
    let a = f.clip(0.0, 5.0, 4.0);
    let b = f.clip(4.0, 5.0, 4.0);
    f.ed.set_transition(
        f.seq,
        b,
        Some(dissolve(1.0, Alignment::Center)),
        LinkMode::Follow,
    )
    .unwrap();
    assert!(f.get(b).transition_in.is_some());
    // Not touching: refused.
    let gap = f.clip(10.0, 5.0, 2.0);
    let err =
        f.ed.set_transition(
            f.seq,
            gap,
            Some(dissolve(1.0, Alignment::Center)),
            LinkMode::Follow,
        )
        .unwrap_err();
    assert!(matches!(err, Error::BadTransition { .. }));
    // No spare media: a clip taken from the very start of the file has nothing before it.
    let mut f = fx();
    let _a = f.clip(0.0, 5.0, 4.0);
    let b = f.clip(4.0, 0.0, 4.0);
    let err =
        f.ed.set_transition(
            f.seq,
            b,
            Some(dissolve(1.0, Alignment::Center)),
            LinkMode::Follow,
        )
        .unwrap_err();
    assert!(matches!(err, Error::BadTransition { .. }));
    // But a transition that only needs the outgoing clip's tail is fine.
    f.ed.set_transition(
        f.seq,
        b,
        Some(dissolve(1.0, Alignment::StartAtCut)),
        LinkMode::Follow,
    )
    .unwrap();
    let _ = a;
}

#[test]
fn edits_that_break_a_transition_drop_it() {
    let mut f = fx();
    let _a = f.clip(0.0, 5.0, 4.0);
    let b = f.clip(4.0, 5.0, 4.0);
    f.ed.set_transition(
        f.seq,
        b,
        Some(dissolve(1.0, Alignment::Center)),
        LinkMode::Follow,
    )
    .unwrap();
    // Moving the clip away leaves a gap, so the transition goes.
    f.ed.move_clips(f.seq, &[b], secs(1.0), 0, LinkMode::Follow)
        .unwrap();
    assert!(f.get(b).transition_in.is_none());
    // Undo brings it back with the clip.
    f.ed.undo().unwrap();
    assert!(f.get(b).transition_in.is_some());
}

#[test]
fn trimming_into_the_handle_drops_the_transition() {
    let mut f = fx();
    let _a = f.clip(0.0, 5.0, 4.0);
    let b = f.clip(4.0, 5.0, 4.0);
    f.ed.set_transition(
        f.seq,
        b,
        Some(dissolve(2.0, Alignment::EndAtCut)),
        LinkMode::Follow,
    )
    .unwrap();
    // Shortening the head of b by 1 s moves its start; the clips no longer touch.
    f.ed.trim(
        f.seq,
        b,
        Edge::Head,
        secs(1.0),
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    assert!(f.get(b).transition_in.is_none());
}

#[test]
fn transition_layers_report_progress() {
    let mut f = fx();
    let a = f.clip(0.0, 5.0, 4.0);
    let b = f.clip(4.0, 5.0, 4.0);
    f.ed.set_transition(
        f.seq,
        b,
        Some(dissolve(2.0, Alignment::Center)),
        LinkMode::Follow,
    )
    .unwrap();
    let seq = f.ed.project().sequence(f.seq).unwrap();
    // Before the window: plain clip a.
    let l = seq.video_layers_at(secs(2.0));
    assert_eq!(l.len(), 1);
    assert!(l[0].transition.is_none());
    // In the first half of the window: still inside clip a, but blending.
    let l = seq.video_layers_at(secs(3.0));
    let t = l[0].transition.as_ref().unwrap();
    assert!(t.progress > 0.0 && t.progress < 0.1);
    assert_eq!(t.outgoing.as_ref().unwrap().clip.id, a);
    assert_eq!(t.incoming.as_ref().unwrap().clip.id, b);
    // Incoming clip's picture comes from before its start: 1 s before 5 s.
    assert!((t.incoming.as_ref().unwrap().source_time.as_secs_f64() - 4.0).abs() < 1e-6);
    // The outgoing clip carries on past its end.
    let last = secs(5.0) - fr(1);
    let l = seq.video_layers_at(last);
    let t = l[0].transition.as_ref().unwrap();
    assert!(t.progress > 0.9);
    let out = t.outgoing.as_ref().unwrap().source_time.as_secs_f64();
    assert!((out - (5.0 + last.as_secs_f64())).abs() < 1e-6, "{out}");
    // At the end of the window it is over.
    assert!(seq.video_layers_at(secs(5.0))[0].transition.is_none());
    // Half way is half way.
    let l = seq.video_layers_at(secs(4.0));
    let t = l[0].transition.as_ref().unwrap();
    assert!((t.progress - 0.5).abs() < 0.03);
    // After the window it is plain again.
    let l = seq.video_layers_at(secs(5.5));
    assert!(l[0].transition.is_none());
}

#[test]
fn linked_audio_gets_a_crossfade() {
    let mut f = fx();
    let mk = |f: &mut Fx, start: f64| {
        f.ed.place_asset(
            f.seq,
            f.av,
            secs(start),
            secs(5.0),
            Some(secs(4.0)),
            Some(f.v1),
            Some(f.a1),
            PlaceMode::Overwrite,
        )
        .unwrap()
    };
    let _first = mk(&mut f, 0.0);
    let second = mk(&mut f, 4.0);
    let wipe = Transition {
        kind: TransitionKind::Wipe(Direction::Left),
        duration: secs(1.0),
        alignment: Alignment::Center,
    };
    f.ed.set_transition(f.seq, second.video.unwrap(), Some(wipe), LinkMode::Follow)
        .unwrap();
    let a = f.get(second.audio.unwrap());
    let t = a.transition_in.unwrap();
    assert_eq!(t.kind, TransitionKind::Dissolve, "audio can only crossfade");
    assert_eq!(t.duration, secs(1.0));
    // Asking for a wipe on an audio clip gives the crossfade, the only audio transition.
    f.ed.set_transition(f.seq, second.audio.unwrap(), Some(wipe), LinkMode::Ignore)
        .unwrap();
    let t = f.get(second.audio.unwrap()).transition_in.unwrap();
    assert_eq!(t.kind, TransitionKind::Dissolve);
}

#[test]
fn files_keep_speed_reverse_and_transitions() {
    let mut f = fx();
    let _a = f.clip(0.0, 5.0, 4.0);
    let b = f.clip(4.0, 5.0, 4.0);
    f.ed.set_transition(
        f.seq,
        b,
        Some(dissolve(1.0, Alignment::Center)),
        LinkMode::Follow,
    )
    .unwrap();
    let c = f.clip(8.0, 2.0, 2.0);
    f.ed.set_speed(
        f.seq,
        c,
        Speed::new(5, 4).unwrap(),
        SpeedMode::KeepLength,
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    f.ed.set_reverse(f.seq, c, true, LinkMode::Follow).unwrap();
    let json = f.ed.project().to_json().unwrap();
    let back = Project::from_json(&json).unwrap();
    assert_eq!(&back, f.ed.project());
}

#[test]
fn old_files_load_at_normal_speed() {
    let mut f = fx();
    let _ = f.clip(0.0, 1.0, 2.0);
    let json = f.ed.project().to_json().unwrap();
    let old = json
        .replace(
            &format!("\"version\": {}", file::FORMAT_VERSION),
            "\"version\": 1",
        )
        .replace("\"speed_num\": 1,", "")
        .replace("\"speed_den\": 1,", "")
        .replace("\"reverse\": false", "\"legacy\": 0");
    let back = Project::from_json(&old).unwrap();
    assert_eq!(&back, f.ed.project());
}

#[test]
fn damaged_retime_data_is_refused() {
    let mut f = fx();
    let _ = f.clip(0.0, 1.0, 2.0);
    let json = f.ed.project().to_json().unwrap();
    let zero = json.replace("\"speed_num\": 1", "\"speed_num\": 0");
    assert!(matches!(Project::from_json(&zero), Err(Error::File(_))));
    // A clip that claims to play 50x but only has 2 s of media left.
    let fast = json.replace("\"speed_num\": 1", "\"speed_num\": 50");
    assert!(Project::from_json(&fast).is_err());
}
