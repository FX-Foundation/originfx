// SPDX-License-Identifier: GPL-3.0-or-later
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
}

fn fade(d: f64) -> Option<Fade> {
    Some(Fade {
        duration: secs(d),
        curve: FadeCurve::Smooth,
    })
}

#[test]
fn gain_pan_and_fades_are_set_and_undone() {
    let mut f = fx();
    let c = f.audio_clip(0.0, 10.0);
    f.ed.set_clip_audio(
        f.seq,
        &[c],
        ClipAudio {
            gain: Gain::from_db(-6.0),
            pan: Pan::new(-30),
            fade_in: Some(fade(1.0)),
            fade_out: Some(fade(2.0)),
        },
        LinkMode::Follow,
    )
    .unwrap();
    let clip = f.get(c);
    assert_eq!(clip.gain, Gain::from_db(-6.0).unwrap());
    assert_eq!(clip.pan, Pan::new(-30).unwrap());
    assert_eq!(clip.fade_out.unwrap().duration, secs(2.0));
    f.ed.undo().unwrap();
    assert_eq!(f.get(c).gain, Gain::UNITY);
    // Partial changes leave the rest alone, and Some(None) removes a fade.
    f.ed.redo().unwrap();
    f.ed.set_clip_audio(
        f.seq,
        &[c],
        ClipAudio {
            fade_in: Some(None),
            ..Default::default()
        },
        LinkMode::Follow,
    )
    .unwrap();
    let clip = f.get(c);
    assert!(clip.fade_in.is_none() && clip.fade_out.is_some());
    assert_eq!(clip.gain, Gain::from_db(-6.0).unwrap());
}

#[test]
fn fades_longer_than_the_clip_and_video_clips_are_refused() {
    let mut f = fx();
    let c = f.audio_clip(0.0, 3.0);
    let err =
        f.ed.set_clip_audio(
            f.seq,
            &[c],
            ClipAudio {
                fade_in: Some(fade(2.0)),
                fade_out: Some(fade(2.0)),
                ..Default::default()
            },
            LinkMode::Follow,
        )
        .unwrap_err();
    assert!(matches!(err, Error::FadesTooLong { .. }));
    assert!(f.get(c).fade_in.is_none(), "a refused edit changes nothing");

    let mut spec = ClipSpec::new(f.av, Time::ZERO);
    spec.duration = Some(secs(3.0));
    let v =
        f.ed.place_clip(f.seq, f.v1, spec, PlaceMode::Overwrite)
            .unwrap();
    let err =
        f.ed.set_clip_audio(
            f.seq,
            &[v],
            ClipAudio {
                gain: Gain::from_db(3.0),
                ..Default::default()
            },
            LinkMode::Follow,
        )
        .unwrap_err();
    assert!(matches!(err, Error::NotAudio(_)));
}

#[test]
fn linked_pair_gets_sound_settings_on_the_sound_only() {
    let mut f = fx();
    let p =
        f.ed.place_asset(
            f.seq,
            f.av,
            Time::ZERO,
            Time::ZERO,
            Some(secs(5.0)),
            Some(f.v1),
            Some(f.a1),
            PlaceMode::Overwrite,
        )
        .unwrap();
    // Asking via the audio clip: the linked picture is skipped, not refused.
    f.ed.set_clip_audio(
        f.seq,
        &[p.audio.unwrap()],
        ClipAudio {
            gain: Gain::from_db(-3.0),
            ..Default::default()
        },
        LinkMode::Follow,
    )
    .unwrap();
    assert_eq!(f.get(p.audio.unwrap()).gain, Gain::from_db(-3.0).unwrap());
    assert_eq!(f.get(p.video.unwrap()).gain, Gain::UNITY);
}

#[test]
fn a_cut_does_not_leave_a_fade_in_the_middle() {
    let mut f = fx();
    let c = f.audio_clip(0.0, 10.0);
    f.ed.set_clip_audio(
        f.seq,
        &[c],
        ClipAudio {
            gain: Gain::from_db(-4.0),
            fade_in: Some(fade(1.0)),
            fade_out: Some(fade(1.0)),
            ..Default::default()
        },
        LinkMode::Follow,
    )
    .unwrap();
    let (l, r) =
        f.ed.split_clip(f.seq, c, secs(4.0), LinkMode::Follow)
            .unwrap();
    let (l, r) = (f.get(l), f.get(r));
    assert!(l.fade_in.is_some() && l.fade_out.is_none());
    assert!(r.fade_in.is_none() && r.fade_out.is_some());
    assert_eq!(l.gain, r.gain, "the level carries over");
}

#[test]
fn trimming_shortens_fades_that_no_longer_fit() {
    let mut f = fx();
    let c = f.audio_clip(0.0, 10.0);
    f.ed.set_clip_audio(
        f.seq,
        &[c],
        ClipAudio {
            fade_in: Some(fade(3.0)),
            fade_out: Some(fade(3.0)),
            ..Default::default()
        },
        LinkMode::Follow,
    )
    .unwrap();
    f.ed.trim(
        f.seq,
        c,
        Edge::Tail,
        secs(-6.0),
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    let c = f.get(c);
    assert_eq!(c.duration, secs(4.0));
    assert_eq!(c.fade_in.unwrap().duration, secs(3.0));
    assert_eq!(
        c.fade_out.unwrap().duration,
        secs(1.0),
        "the fade out gives way first"
    );
}

#[test]
fn track_and_master_levels() {
    let mut f = fx();
    f.ed.update_track(
        f.seq,
        f.a1,
        TrackUpdate {
            gain: Gain::from_db(-2.0),
            pan: Pan::new(50),
            ..Default::default()
        },
    )
    .unwrap();
    f.ed.set_master_gain(f.seq, Gain::from_db(-1.5).unwrap())
        .unwrap();
    let s = f.ed.project().sequence(f.seq).unwrap();
    assert_eq!(s.audio_tracks()[0].gain, Gain::from_db(-2.0).unwrap());
    assert_eq!(s.master_gain, Gain::from_db(-1.5).unwrap());
    // A video track has no level.
    let err = f.ed.update_track(
        f.seq,
        f.v1,
        TrackUpdate {
            gain: Gain::from_db(-2.0),
            ..Default::default()
        },
    );
    assert!(err.is_err());
    // All of it survives a save and load.
    let back = Project::from_json(&f.ed.project().to_json().unwrap()).unwrap();
    assert_eq!(&back, f.ed.project());
}

#[test]
fn files_keep_clip_sound_settings_and_refuse_nonsense() {
    let mut f = fx();
    let c = f.audio_clip(0.0, 10.0);
    f.ed.set_clip_audio(
        f.seq,
        &[c],
        ClipAudio {
            gain: Gain::from_db(5.5),
            pan: Pan::new(-100),
            fade_in: Some(fade(0.5)),
            ..Default::default()
        },
        LinkMode::Follow,
    )
    .unwrap();
    let json = f.ed.project().to_json().unwrap();
    assert_eq!(&Project::from_json(&json).unwrap(), f.ed.project());
    let bad = json.replace("\"gain_cb\": 550", "\"gain_cb\": 99999");
    assert!(matches!(Project::from_json(&bad), Err(Error::File(_))));
    let bad = json.replace("\"pan\": -100", "\"pan\": -500");
    assert!(matches!(Project::from_json(&bad), Err(Error::File(_))));
    let bad = json.replace("\"smooth\"", "\"wobbly\"");
    assert!(matches!(Project::from_json(&bad), Err(Error::File(_))));
}
