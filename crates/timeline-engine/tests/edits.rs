// SPDX-License-Identifier: GPL-3.0-or-later
use timeline_engine::*;

fn secs(s: f64) -> Time {
    Time::from_secs_f64(s)
}

struct Fx {
    ed: Editor,
    seq: SequenceId,
    v1: TrackId,
    v2: TrackId,
    a1: TrackId,
    clip10: AssetId,
    av10: AssetId,
}

fn fx() -> Fx {
    let mut ed = Editor::new("t");
    let seq = ed.add_sequence(SequenceSettings::default()).unwrap();
    let s = ed.project().sequence(seq).unwrap();
    let (v1, v2) = (s.video_tracks()[0].id, s.video_tracks()[1].id);
    let a1 = s.audio_tracks()[0].id;
    let video = |n: &str, d: f64| NewAsset {
        name: n.into(),
        path: format!("/{n}.mp4").into(),
        duration: Some(secs(d)),
        video: vec![VideoProps {
            width: 1920,
            height: 1080,
            frame_rate: Some(FrameRate::FPS_24),
        }],
        audio: vec![],
    };
    let clip10 = ed.add_asset(video("a", 10.0)).unwrap();
    let mut av = video("av", 10.0);
    av.audio = vec![AudioProps {
        sample_rate: 48000,
        channels: 2,
    }];
    let av10 = ed.add_asset(av).unwrap();
    Fx {
        ed,
        seq,
        v1,
        v2,
        a1,
        clip10,
        av10,
    }
}

fn spec(a: AssetId, start: f64, dur: f64) -> ClipSpec {
    let mut s = ClipSpec::new(a, secs(start));
    s.duration = Some(secs(dur));
    s
}

fn layout(f: &Fx, t: TrackId) -> Vec<(f64, f64)> {
    f.ed.project()
        .sequence(f.seq)
        .unwrap()
        .track(t)
        .unwrap()
        .clips()
        .map(|c| (c.start.as_secs_f64(), c.end().as_secs_f64()))
        .collect()
}

fn near(a: &[(f64, f64)], b: &[(f64, f64)]) {
    assert_eq!(a.len(), b.len(), "{a:?} vs {b:?}");
    for (x, y) in a.iter().zip(b) {
        assert!(
            (x.0 - y.0).abs() < 1e-6 && (x.1 - y.1).abs() < 1e-6,
            "{a:?} vs {b:?}"
        );
    }
}

#[test]
fn overwrite_splits_and_trims() {
    let mut f = fx();
    f.ed.place_clip(f.seq, f.v1, spec(f.clip10, 0.0, 10.0), PlaceMode::Overwrite)
        .unwrap();
    f.ed.place_clip(f.seq, f.v1, spec(f.clip10, 4.0, 2.0), PlaceMode::Overwrite)
        .unwrap();
    near(&layout(&f, f.v1), &[(0.0, 4.0), (4.0, 6.0), (6.0, 10.0)]);
    let s = f.ed.project().sequence(f.seq).unwrap();
    let right = s.track(f.v1).unwrap().clips().last().unwrap();
    assert!((right.source_in.as_secs_f64() - 6.0).abs() < 1e-6);
    f.ed.project().validate().unwrap();
}

#[test]
fn insert_pushes_later_clips() {
    let mut f = fx();
    f.ed.place_clip(f.seq, f.v1, spec(f.clip10, 0.0, 4.0), PlaceMode::Overwrite)
        .unwrap();
    f.ed.place_clip(
        f.seq,
        f.v1,
        spec(f.clip10, 1.0, 2.0),
        PlaceMode::Insert { sync: false },
    )
    .unwrap();
    near(&layout(&f, f.v1), &[(0.0, 1.0), (1.0, 3.0), (3.0, 6.0)]);
}

#[test]
fn rejects_past_media_and_locked() {
    let mut f = fx();
    assert!(matches!(
        f.ed.place_clip(f.seq, f.v1, spec(f.clip10, 0.0, 11.0), PlaceMode::Overwrite),
        Err(Error::OutsideSource { .. })
    ));
    f.ed.update_track(
        f.seq,
        f.v1,
        TrackUpdate {
            locked: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(
        f.ed.place_clip(f.seq, f.v1, spec(f.clip10, 0.0, 1.0), PlaceMode::Overwrite),
        Err(Error::TrackLocked(_))
    ));
    assert!(!f.ed.can_undo() || f.ed.undo_label() == Some("Track settings"));
}

#[test]
fn linked_pair_and_split() {
    let mut f = fx();
    let p =
        f.ed.place_asset(
            f.seq,
            f.av10,
            Time::ZERO,
            Time::ZERO,
            Some(secs(5.0)),
            Some(f.v1),
            Some(f.a1),
            PlaceMode::Overwrite,
        )
        .unwrap();
    let (v, a) = (p.video.unwrap(), p.audio.unwrap());
    let (l, _r) =
        f.ed.split_clip(f.seq, v, secs(2.0), LinkMode::Follow)
            .unwrap();
    assert_eq!(l, v);
    near(&layout(&f, f.v1), &[(0.0, 2.0), (2.0, 5.0)]);
    near(&layout(&f, f.a1), &[(0.0, 2.0), (2.0, 5.0)]);
    let s = f.ed.project().sequence(f.seq).unwrap();
    let la = s.find_clip(a).unwrap().1.link;
    assert_eq!(la, s.find_clip(v).unwrap().1.link);
    f.ed.project().validate().unwrap();
}

#[test]
fn ripple_delete_closes_gap() {
    let mut f = fx();
    let c1 =
        f.ed.place_clip(f.seq, f.v1, spec(f.clip10, 0.0, 2.0), PlaceMode::Overwrite)
            .unwrap();
    f.ed.place_clip(f.seq, f.v1, spec(f.clip10, 2.0, 3.0), PlaceMode::Overwrite)
        .unwrap();
    f.ed.delete_clips(f.seq, &[c1], Ripple::Track, LinkMode::Follow)
        .unwrap();
    near(&layout(&f, f.v1), &[(0.0, 3.0)]);
}

#[test]
fn sync_ripple_blocked_by_other_track() {
    let mut f = fx();
    let c1 =
        f.ed.place_clip(f.seq, f.v1, spec(f.clip10, 0.0, 2.0), PlaceMode::Overwrite)
            .unwrap();
    f.ed.place_clip(f.seq, f.v2, spec(f.clip10, 0.0, 5.0), PlaceMode::Overwrite)
        .unwrap();
    let before = f.ed.project().clone();
    let r =
        f.ed.delete_clips(f.seq, &[c1], Ripple::Sync, LinkMode::Follow);
    assert!(matches!(r, Err(Error::Blocked { .. })));
    assert_eq!(&before, f.ed.project());
}

#[test]
fn move_overwrites_and_changes_track() {
    let mut f = fx();
    let c =
        f.ed.place_clip(f.seq, f.v1, spec(f.clip10, 0.0, 2.0), PlaceMode::Overwrite)
            .unwrap();
    f.ed.place_clip(f.seq, f.v2, spec(f.clip10, 3.0, 4.0), PlaceMode::Overwrite)
        .unwrap();
    f.ed.move_clips(f.seq, &[c], secs(4.0), 1, LinkMode::Follow)
        .unwrap();
    assert!(layout(&f, f.v1).is_empty());
    near(&layout(&f, f.v2), &[(3.0, 4.0), (4.0, 6.0), (6.0, 7.0)]);
    assert!(f
        .ed
        .move_clips(f.seq, &[c], secs(-100.0), 0, LinkMode::Follow)
        .is_err());
    assert!(matches!(
        f.ed.move_clips(f.seq, &[c], secs(0.0), 5, LinkMode::Follow),
        Err(Error::NoTrackThere(_))
    ));
}

#[test]
fn trim_ripple_roll_slip_slide() {
    let mut f = fx();
    let a =
        f.ed.place_clip(f.seq, f.v1, spec(f.clip10, 0.0, 3.0), PlaceMode::Overwrite)
            .unwrap();
    let mut s2 = spec(f.clip10, 3.0, 3.0);
    s2.source_in = secs(2.0);
    let b =
        f.ed.place_clip(f.seq, f.v1, s2, PlaceMode::Overwrite)
            .unwrap();
    let mut s3 = spec(f.clip10, 6.0, 2.0);
    s3.source_in = secs(1.0);
    let c =
        f.ed.place_clip(f.seq, f.v1, s3, PlaceMode::Overwrite)
            .unwrap();

    // tail trim collides without ripple
    assert!(matches!(
        f.ed.trim(
            f.seq,
            a,
            Edge::Tail,
            secs(1.0),
            Ripple::Off,
            LinkMode::Follow
        ),
        Err(Error::Collision { .. })
    ));
    f.ed.trim(
        f.seq,
        a,
        Edge::Tail,
        secs(-1.0),
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    near(&layout(&f, f.v1), &[(0.0, 2.0), (3.0, 6.0), (6.0, 8.0)]);
    // ripple tail
    f.ed.trim(
        f.seq,
        a,
        Edge::Tail,
        secs(1.0),
        Ripple::Track,
        LinkMode::Follow,
    )
    .unwrap();
    near(&layout(&f, f.v1), &[(0.0, 3.0), (4.0, 7.0), (7.0, 9.0)]);
    f.ed.undo().unwrap();
    // roll b/c: b ends 6, c starts 6
    f.ed.roll(f.seq, b, c, secs(1.0)).unwrap();
    near(&layout(&f, f.v1), &[(0.0, 2.0), (3.0, 7.0), (7.0, 8.0)]);
    f.ed.undo().unwrap();
    // slip
    f.ed.slip(f.seq, b, secs(1.0), LinkMode::Follow).unwrap();
    let si =
        f.ed.project()
            .sequence(f.seq)
            .unwrap()
            .find_clip(b)
            .unwrap()
            .1
            .source_in;
    assert!((si.as_secs_f64() - 3.0).abs() < 1e-6);
    assert!(f.ed.slip(f.seq, b, secs(-5.0), LinkMode::Follow).is_err());
    // slide b right by 1 with neighbours: a:(0,2) not touching b(3,6): needs neighbours
    assert!(matches!(
        f.ed.slide(f.seq, b, secs(1.0)),
        Err(Error::NeedsNeighbours(_))
    ));
    // make touching, then slide
    f.ed.trim(
        f.seq,
        a,
        Edge::Tail,
        secs(1.0),
        Ripple::Off,
        LinkMode::Follow,
    )
    .unwrap();
    f.ed.slide(f.seq, b, secs(1.0)).unwrap();
    near(&layout(&f, f.v1), &[(0.0, 4.0), (4.0, 7.0), (7.0, 8.0)]);
    f.ed.project().validate().unwrap();
}

#[test]
fn undo_redo_dirty_and_group() {
    let mut f = fx();
    f.ed.mark_saved();
    assert!(!f.ed.is_dirty());
    f.ed.place_clip(f.seq, f.v1, spec(f.clip10, 0.0, 2.0), PlaceMode::Overwrite)
        .unwrap();
    assert!(f.ed.is_dirty());
    f.ed.undo().unwrap();
    assert!(!f.ed.is_dirty());
    f.ed.redo().unwrap();
    assert!(f.ed.is_dirty());

    let before = f.ed.project().clone();
    let r: Result<()> = f.ed.group("two", |e| {
        e.place_clip(f.seq, f.v1, spec(f.clip10, 5.0, 1.0), PlaceMode::Overwrite)?;
        e.place_clip(f.seq, f.v1, spec(f.clip10, 0.0, 99.0), PlaceMode::Overwrite)?;
        Ok(())
    });
    assert!(r.is_err());
    assert_eq!(&before, f.ed.project());
    f.ed.group("two", |e| {
        e.place_clip(f.seq, f.v1, spec(f.clip10, 5.0, 1.0), PlaceMode::Overwrite)?;
        e.place_clip(f.seq, f.v1, spec(f.clip10, 7.0, 1.0), PlaceMode::Overwrite)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(f.ed.undo_label(), Some("two"));
    f.ed.undo().unwrap();
    assert_eq!(&before, f.ed.project());
}

#[test]
fn remove_asset_in_use_and_ripple_range() {
    let mut f = fx();
    f.ed.place_clip(f.seq, f.v1, spec(f.clip10, 0.0, 6.0), PlaceMode::Overwrite)
        .unwrap();
    assert!(matches!(
        f.ed.remove_asset(f.clip10),
        Err(Error::AssetInUse(_))
    ));
    f.ed.ripple_delete_range(f.seq, TimeRange::new(secs(2.0), secs(4.0)).unwrap(), None)
        .unwrap();
    near(&layout(&f, f.v1), &[(0.0, 2.0), (2.0, 4.0)]);
    f.ed.project().validate().unwrap();
}

#[test]
fn markers_sorted() {
    let mut f = fx();
    f.ed.add_marker(f.seq, secs(5.0), Time::ZERO, "b", MarkerColour::Red)
        .unwrap();
    f.ed.add_marker(f.seq, secs(1.0), Time::ZERO, "a", MarkerColour::Blue)
        .unwrap();
    let m: Vec<_> =
        f.ed.project()
            .sequence(f.seq)
            .unwrap()
            .markers()
            .iter()
            .map(|m| m.name.clone())
            .collect();
    assert_eq!(m, ["a", "b"]);
}
