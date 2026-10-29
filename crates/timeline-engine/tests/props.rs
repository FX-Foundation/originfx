// SPDX-License-Identifier: GPL-3.0-or-later
//! Random edits must never break the rules, never half-apply, and always undo cleanly.
use proptest::prelude::*;
use timeline_engine::*;

#[derive(Debug, Clone)]
enum Op {
    Place {
        track: usize,
        start: i64,
        dur: i64,
        insert: bool,
        pair: bool,
    },
    Split {
        pick: usize,
        at: i64,
    },
    Delete {
        pick: usize,
        ripple: u8,
    },
    Move {
        pick: usize,
        delta: i64,
        tracks: i32,
    },
    Trim {
        pick: usize,
        tail: bool,
        delta: i64,
        ripple: u8,
    },
    Roll {
        pick: usize,
        delta: i64,
    },
    Slip {
        pick: usize,
        delta: i64,
    },
    Slide {
        pick: usize,
        delta: i64,
    },
    RangeDelete {
        a: i64,
        b: i64,
    },
}

fn op() -> impl Strategy<Value = Op> {
    let f = -30i64..60;
    prop_oneof![
        (0usize..4, 0i64..200, 1i64..80, any::<bool>(), any::<bool>()).prop_map(
            |(track, start, dur, insert, pair)| Op::Place {
                track,
                start,
                dur,
                insert,
                pair
            }
        ),
        (0usize..50, 0i64..300).prop_map(|(pick, at)| Op::Split { pick, at }),
        (0usize..50, 0u8..3).prop_map(|(pick, ripple)| Op::Delete { pick, ripple }),
        (0usize..50, -60i64..60, -1i32..2).prop_map(|(pick, delta, tracks)| Op::Move {
            pick,
            delta,
            tracks
        }),
        (0usize..50, any::<bool>(), f.clone(), 0u8..3).prop_map(|(pick, tail, delta, ripple)| {
            Op::Trim {
                pick,
                tail,
                delta,
                ripple,
            }
        }),
        (0usize..50, f.clone()).prop_map(|(pick, delta)| Op::Roll { pick, delta }),
        (0usize..50, f.clone()).prop_map(|(pick, delta)| Op::Slip { pick, delta }),
        (0usize..50, f).prop_map(|(pick, delta)| Op::Slide { pick, delta }),
        (0i64..300, 0i64..300).prop_map(|(a, b)| Op::RangeDelete { a, b }),
    ]
}

fn ripple(n: u8) -> Ripple {
    [Ripple::Off, Ripple::Track, Ripple::Sync][n as usize]
}

fn frames(n: i64) -> Time {
    Time::from_frames(n, FrameRate::FPS_24)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]
    #[test]
    fn random_edits_keep_invariants(ops in prop::collection::vec(op(), 1..60)) {
        let mut ed = Editor::new("p");
        let seq = ed.add_sequence(SequenceSettings::default()).unwrap();
        let s = ed.project().sequence(seq).unwrap();
        let tracks: Vec<TrackId> = vec![s.video_tracks()[0].id, s.video_tracks()[1].id, s.audio_tracks()[0].id, s.audio_tracks()[1].id];
        let asset = ed.add_asset(NewAsset {
            name: "m".into(), path: "/m.mp4".into(), duration: Some(frames(240)),
            video: vec![VideoProps { width: 1920, height: 1080, frame_rate: Some(FrameRate::FPS_24) }],
            audio: vec![AudioProps { sample_rate: 48000, channels: 2 }],
        }).unwrap();
        let mut ed = Editor::from_project(ed.project().clone());
        let start_project = ed.project().clone();

        for o in ops {
            let before = ed.project().clone();
            let ids: Vec<ClipId> = before.sequence(seq).unwrap().clips().map(|(_, c)| c.id).collect();
            let pick = |i: usize| ids.get(i % ids.len().max(1)).copied().unwrap_or(ClipId(u64::MAX));
            let r: Result<()> = match o {
                Op::Place { track, start, dur, insert, pair } => {
                    let mode = if insert { PlaceMode::Insert { sync: pair } } else { PlaceMode::Overwrite };
                    if pair {
                        ed.place_asset(seq, asset, frames(start), Time::ZERO, Some(frames(dur)), Some(tracks[0]), Some(tracks[2]), mode).map(|_| ())
                    } else {
                        let mut sp = ClipSpec::new(asset, frames(start));
                        sp.duration = Some(frames(dur));
                        sp.stream = 0;
                        ed.place_clip(seq, tracks[track], sp, mode).map(|_| ())
                    }
                }
                Op::Split { pick: p, at } => ed.split_clip(seq, pick(p), frames(at), LinkMode::Follow).map(|_| ()),
                Op::Delete { pick: p, ripple: r } => ed.delete_clips(seq, &[pick(p)], ripple(r), LinkMode::Follow),
                Op::Move { pick: p, delta, tracks: t } => ed.move_clips(seq, &[pick(p)], frames(delta), t, LinkMode::Follow),
                Op::Trim { pick: p, tail, delta, ripple: r } => ed.trim(seq, pick(p), if tail { Edge::Tail } else { Edge::Head }, frames(delta), ripple(r), LinkMode::Follow),
                Op::Roll { pick: p, delta } => ed.roll(seq, pick(p), pick(p + 1), frames(delta)),
                Op::Slip { pick: p, delta } => ed.slip(seq, pick(p), frames(delta), LinkMode::Follow),
                Op::Slide { pick: p, delta } => ed.slide(seq, pick(p), frames(delta)),
                Op::RangeDelete { a, b } => match TimeRange::new(frames(a.min(b)), frames(a.max(b))) {
                    Some(range) => ed.ripple_delete_range(seq, range, None),
                    None => Ok(()),
                },
            };
            if r.is_err() {
                prop_assert_eq!(&before, ed.project(), "a failed edit changed the project");
            }
            prop_assert!(ed.project().validate().is_ok(), "{:?}", ed.project().validate());
        }

        // The file format round-trips exactly.
        let text = ed.project().to_json().unwrap();
        prop_assert_eq!(&Project::from_json(&text).unwrap(), ed.project());

        // Undoing everything gets back to the start.
        while ed.can_undo() { ed.undo().unwrap(); }
        prop_assert_eq!(&start_project, ed.project());
    }
}
