// SPDX-License-Identifier: GPL-3.0-or-later
use timeline_engine::*;

fn sample() -> Project {
    let mut ed = Editor::new("film");
    let seq = ed
        .add_sequence(SequenceSettings {
            frame_rate: FrameRate::FPS_29_97,
            drop_frame: true,
            ..Default::default()
        })
        .unwrap();
    let a = ed
        .add_asset(NewAsset {
            name: "a".into(),
            path: "/a.mov".into(),
            duration: Some(Time::from_secs_f64(8.0)),
            video: vec![VideoProps {
                width: 1280,
                height: 720,
                frame_rate: Some(FrameRate::FPS_29_97),
            }],
            audio: vec![AudioProps {
                sample_rate: 44100,
                channels: 1,
            }],
        })
        .unwrap();
    let s = ed.project().sequence(seq).unwrap();
    let (v, au) = (s.video_tracks()[0].id, s.audio_tracks()[0].id);
    ed.place_asset(
        seq,
        a,
        Time::ZERO,
        Time::ZERO,
        None,
        Some(v),
        Some(au),
        PlaceMode::Overwrite,
    )
    .unwrap();
    ed.add_marker(
        seq,
        Time::from_secs_f64(2.0),
        Time::ZERO,
        "x",
        MarkerColour::Pink,
    )
    .unwrap();
    ed.project().clone()
}

#[test]
fn save_load_round_trip() {
    let p = sample();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.originfx");
    p.save(&path).unwrap();
    assert!(!dir.path().join("p.originfx.tmp").exists());
    assert_eq!(Project::load(&path).unwrap(), p);
}

#[test]
fn refuses_damaged_and_future_files() {
    assert!(matches!(Project::from_json("{nope"), Err(Error::File(_))));
    let p = sample();
    let text = p.to_json().unwrap();
    let future = text.replace("\"version\": 1", "\"version\": 99");
    assert!(matches!(
        Project::from_json(&future),
        Err(Error::UnsupportedVersion { found: 99, .. })
    ));
    // Make two clips overlap by hand-editing the next id too low.
    let bad = text.replace(&format!("\"next_id\": {}", p.next_id()), "\"next_id\": 2");
    assert!(Project::from_json(&bad).is_err());
    let wrong = text.replace("originfx-project", "something-else");
    assert!(matches!(Project::from_json(&wrong), Err(Error::File(_))));
}

#[test]
fn layers_and_audio_queries() {
    let p = sample();
    let seq = p.sequences().values().next().unwrap();
    let t = Time::from_secs_f64(1.0);
    assert_eq!(seq.video_layers_at(t).len(), 1);
    assert_eq!(seq.audio_at(t).len(), 1);
    assert!(seq.video_layers_at(Time::from_secs_f64(9.0)).is_empty());
    assert_eq!(p.missing_assets().len(), 1);
}
