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
    Speed {
        pick: usize,
        num: u32,
        den: u32,
        keep_media: bool,
        ripple: u8,
    },
    Reverse {
        pick: usize,
        on: bool,
    },
    Sound {
        pick: usize,
        gain: i32,
        pan: i16,
        fi: i64,
        fo: i64,
    },
    Transition {
        pick: usize,
        dur: i64,
        align: u8,
        kind: u8,
        remove: bool,
    },
    Key {
        pick: usize,
        param: u8,
        offset: i64,
        value: i32,
        interp: u8,
    },
    KeyTrack {
        track: usize,
        param: u8,
        at: i64,
        value: i32,
    },
    KeyMaster {
        at: i64,
        value: i32,
    },
    Unkey {
        pick: usize,
        param: u8,
        clear: bool,
        offset: i64,
    },
    Fx {
        pick: usize,
        action: u8,
        kind: u8,
        value: i32,
        offset: i64,
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
        (0usize..50, 1u32..9, 1u32..9, any::<bool>(), 0u8..3).prop_map(
            |(pick, num, den, keep_media, ripple)| Op::Speed {
                pick,
                num,
                den,
                keep_media,
                ripple
            }
        ),
        (0usize..50, any::<bool>()).prop_map(|(pick, on)| Op::Reverse { pick, on }),
        (0usize..50, -2000i32..1200, -100i16..100, 0i64..30, 0i64..30).prop_map(
            |(pick, gain, pan, fi, fo)| Op::Sound {
                pick,
                gain,
                pan,
                fi,
                fo
            }
        ),
        (0usize..50, 1i64..40, 0u8..3, 0u8..9, any::<bool>()).prop_map(
            |(pick, dur, align, kind, remove)| Op::Transition {
                pick,
                dur,
                align,
                kind,
                remove
            }
        ),
        (0usize..50, 0u8..3, 0i64..80, -1200i32..2400, 0u8..3).prop_map(
            |(pick, param, offset, value, interp)| Op::Key {
                pick,
                param,
                offset,
                value,
                interp
            }
        ),
        (0usize..4, 0u8..2, 0i64..300, -12000i32..2400).prop_map(|(track, param, at, value)| {
            Op::KeyTrack {
                track,
                param,
                at,
                value,
            }
        }),
        (0i64..300, -12000i32..2400).prop_map(|(at, value)| Op::KeyMaster { at, value }),
        (0usize..50, 0u8..6, 0u8..14, -500i32..500, 0i64..80).prop_map(
            |(pick, action, kind, value, offset)| Op::Fx {
                pick,
                action,
                kind,
                value,
                offset
            }
        ),
        (0usize..50, 0u8..3, any::<bool>(), 0i64..80).prop_map(|(pick, param, clear, offset)| {
            Op::Unkey {
                pick,
                param,
                clear,
                offset,
            }
        }),
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
                Op::Speed { pick: p, num, den, keep_media, ripple: r } => match Speed::new(num, den) {
                    Some(sp) => ed.set_speed(seq, pick(p), sp, if keep_media { SpeedMode::KeepMedia } else { SpeedMode::KeepLength }, ripple(r), LinkMode::Follow),
                    None => Ok(()),
                },
                Op::Reverse { pick: p, on } => ed.set_reverse(seq, pick(p), on, LinkMode::Follow),
                Op::Sound { pick: p, gain, pan, fi, fo } => {
                    let fade = |n: i64| Some(if n == 0 { None } else { Some(Fade { duration: frames(n), curve: FadeCurve::Smooth }) });
                    ed.set_clip_audio(seq, &[pick(p)], ClipAudio {
                        gain: Gain::from_db(f64::from(gain) / 100.0),
                        pan: Pan::new(pan),
                        fade_in: fade(fi),
                        fade_out: fade(fo),
                    }, LinkMode::Follow)
                }
                Op::Transition { pick: p, dur, align, kind, remove } => {
                    let names = TransitionKind::all_names();
                    let tr = Transition {
                        kind: TransitionKind::parse(&names[kind as usize]).unwrap(),
                        duration: frames(dur),
                        alignment: [Alignment::Center, Alignment::StartAtCut, Alignment::EndAtCut][align as usize],
                    };
                    ed.set_transition(seq, pick(p), if remove { None } else { Some(tr) }, LinkMode::Follow)
                }
                Op::Key { pick: p, param, offset, value, interp } => {
                    let id = pick(p);
                    let at = before.sequence(seq).unwrap().find_clip(id).map_or(Time::ZERO, |(_, c)| c.start) + frames(offset);
                    let param = Param::ALL[param as usize];
                    // Speeds are kept near 1 so ramps are plausible; the rest use the full scale.
                    let value = if param == Param::Speed { 0.25 + f64::from(value.rem_euclid(400)) / 100.0 } else if param == Param::Pan { f64::from(value % 101) } else { f64::from(value) / 100.0 };
                    let interp = [Interp::Hold, Interp::Linear, Interp::Smooth][interp as usize];
                    ed.set_keyframe(seq, Target::Clip(id), param, at, value, interp, LinkMode::Follow)
                }
                Op::KeyTrack { track, param, at, value } => {
                    let param = [Param::Gain, Param::Pan][param as usize];
                    let value = if param == Param::Pan { f64::from(value % 101) } else { f64::from(value) / 100.0 };
                    ed.set_keyframe(seq, Target::Track(tracks[track]), param, frames(at), value, Interp::Smooth, LinkMode::Follow)
                }
                Op::KeyMaster { at, value } => ed.set_keyframe(seq, Target::Master, Param::Gain, frames(at), f64::from(value) / 100.0, Interp::Linear, LinkMode::Follow),
                Op::Unkey { pick: p, param, clear, offset } => {
                    let id = pick(p);
                    let param = Param::ALL[param as usize];
                    if clear {
                        ed.clear_animation(seq, Target::Clip(id), param, LinkMode::Follow)
                    } else {
                        let at = before.sequence(seq).unwrap().find_clip(id).map_or(Time::ZERO, |(_, c)| c.start) + frames(offset);
                        ed.remove_keyframe(seq, Target::Clip(id), param, at, LinkMode::Follow)
                    }
                }
                Op::Fx { pick: p, action, kind, value, offset } => {
                    let id = pick(p);
                    let clip = before.sequence(seq).unwrap().find_clip(id).map(|(_, c)| c.clone());
                    let first = clip.as_ref().and_then(|c| c.effects.first().map(|e| (e.id, e.kind)));
                    let start = clip.as_ref().map_or(Time::ZERO, |c| c.start);
                    match (action, first) {
                        (0, _) => ed.add_effect(seq, id, EffectKind::ALL[kind as usize], None).map(|_| ()),
                        (1, Some((e, _))) => ed.remove_effect(seq, id, e),
                        (2, Some((e, k))) => {
                            let spec = k.params()[0];
                            ed.set_effect_value(seq, id, e, spec.name, spec.min + (spec.max - spec.min) * f64::from(value + 500) / 1000.0)
                        }
                        (3, Some((e, k))) => {
                            let spec = k.params()[0];
                            ed.key_effect(seq, id, e, spec.name, start + frames(offset), spec.min + (spec.max - spec.min) * f64::from(value + 500) / 1000.0, Interp::Smooth)
                        }
                        (4, Some((e, _))) => ed.set_effect_enabled(seq, id, e, value > 0),
                        (5, Some((e, k))) => ed.clear_effect_animation(seq, id, e, k.params()[0].name),
                        _ => Ok(()),
                    }
                }
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
