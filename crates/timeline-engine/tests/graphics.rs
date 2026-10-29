// SPDX-License-Identifier: GPL-3.0-or-later
//! Drawn graphics as project media: adding, editing, placing, saving and the rules.
use timeline_engine::*;

fn secs(s: f64) -> Time {
    Time::from_secs_f64(s)
}

fn red() -> Generator {
    Generator::Fill(Fill::Solid(Colour::rgb(255, 0, 0)))
}

fn star() -> Generator {
    Generator::Shape(Shape {
        kind: ShapeKind::Star {
            points: 5,
            inner_pct: 40.0,
        },
        width_pct: 20.0,
        height_pct: 30.0,
        fill: Fill::Linear {
            from: Colour::rgb(255, 215, 0),
            to: Colour::parse("#ff8c0080").unwrap(),
            angle_deg: 45.5,
        },
        stroke: Some(Stroke {
            colour: Colour::WHITE,
            width_pct: 0.6,
        }),
    })
}

fn title() -> Generator {
    Generator::Text(Text {
        text: "ORIGIN\nFX".into(),
        font: Some("/fonts/x.ttf".into()),
        size_pct: 12.5,
        colour: Colour::WHITE,
        align: Align::Right,
        line_gap_pct: 10.0,
        tracking_pct: -2.0,
        outline: Some(Stroke {
            colour: Colour::BLACK,
            width_pct: 0.4,
        }),
        shadow: Some(Shadow {
            colour: Colour::parse("#000000aa").unwrap(),
            dx_pct: 0.5,
            dy_pct: 0.7,
        }),
    })
}

fn setup() -> (Editor, SequenceId, TrackId) {
    let mut ed = Editor::new("t");
    let seq = ed.add_sequence(SequenceSettings::default()).unwrap();
    let v1 = ed.project().sequence(seq).unwrap().video_tracks()[0].id;
    (ed, seq, v1)
}

fn place(
    ed: &mut Editor,
    seq: SequenceId,
    v1: TrackId,
    a: AssetId,
    start: f64,
    len: f64,
) -> ClipId {
    let mut s = ClipSpec::new(a, secs(start));
    s.duration = Some(secs(len));
    ed.place_clip(seq, v1, s, PlaceMode::Overwrite).unwrap()
}

#[test]
fn a_graphic_is_a_still_asset_with_no_file_and_can_be_held_any_length() {
    let (mut ed, seq, v1) = setup();
    let a = ed.add_generated("red", red()).unwrap();
    let asset = ed.project().asset(a).unwrap();
    assert!(asset.is_generated() && asset.is_still() && asset.has_video() && !asset.has_audio());
    assert!(ed.project().missing_assets().is_empty());
    // A very long hold is fine for a graphic.
    let c = place(&mut ed, seq, v1, a, 0.0, 3600.0);
    ed.split_clip(seq, c, secs(10.0), LinkMode::Follow).unwrap();
    ed.project().validate().unwrap();
    // It can use the whole picture toolbox.
    ed.set_look_value(seq, c, Param::Opacity, 40.0).unwrap();
    ed.set_mask(seq, c, MaskShape::Ellipse, false).unwrap();
    let e = ed.add_effect(seq, c, EffectKind::Blur, None).unwrap();
    let _ = e;
}

#[test]
fn editing_a_graphic_changes_it_for_every_clip_and_wrong_use_is_refused() {
    let (mut ed, seq, v1) = setup();
    let a = ed.add_generated("g", red()).unwrap();
    let c = place(&mut ed, seq, v1, a, 0.0, 2.0);
    ed.set_generated(a, star()).unwrap();
    assert_eq!(ed.project().asset(a).unwrap().generator, Some(star()));
    // In use, so it cannot be removed; not a file, so it cannot be relinked.
    assert!(ed.remove_asset(a).is_err());
    assert!(ed.relink_asset(a, "/x.png").is_err());
    ed.delete_clips(seq, &[c], Ripple::Off, LinkMode::Follow)
        .unwrap();
    ed.remove_asset(a).unwrap();
    // A file is not a graphic.
    let f = ed
        .add_asset(NewAsset {
            name: "f".into(),
            path: "/f.png".into(),
            duration: None,
            video: vec![VideoProps {
                width: 10,
                height: 10,
                frame_rate: None,
            }],
            audio: vec![],
        })
        .unwrap();
    assert!(ed.set_generated(f, red()).is_err());
    assert!(ed.set_generated(AssetId(9999), red()).is_err());
    // Undo and redo work like any edit.
    let g = ed.add_generated("g2", red()).unwrap();
    ed.undo().unwrap();
    assert!(ed.project().asset(g).is_none());
}

#[test]
fn numbers_are_checked() {
    let (mut ed, ..) = setup();
    let mut s = match star() {
        Generator::Shape(s) => s,
        _ => unreachable!(),
    };
    let ok = |g: &Generator| validate_graphic(g).is_ok();
    assert!(ok(&Generator::Shape(s)));
    s.width_pct = f64::NAN;
    assert!(!ok(&Generator::Shape(s)));
    s.width_pct = 20.0;
    s.height_pct = 1001.0;
    assert!(!ok(&Generator::Shape(s)));
    s.height_pct = 20.0;
    s.kind = ShapeKind::Polygon { sides: 2 };
    assert!(!ok(&Generator::Shape(s)));
    s.kind = ShapeKind::Star {
        points: 5,
        inner_pct: 1.0,
    };
    assert!(!ok(&Generator::Shape(s)));
    s.kind = ShapeKind::Rect { corner_pct: 101.0 };
    assert!(!ok(&Generator::Shape(s)));
    let mut t = match title() {
        Generator::Text(t) => t,
        _ => unreachable!(),
    };
    assert!(ok(&Generator::Text(t.clone())));
    t.size_pct = 0.0;
    assert!(!ok(&Generator::Text(t.clone())));
    t.size_pct = 10.0;
    t.text = "x".repeat(2001);
    assert!(!ok(&Generator::Text(t.clone())));
    t.text = "ok".into();
    t.shadow = Some(Shadow {
        colour: Colour::BLACK,
        dx_pct: 99.0,
        dy_pct: 0.0,
    });
    assert!(!ok(&Generator::Text(t)));
    assert!(ed
        .add_generated(
            "bad",
            Generator::Fill(Fill::Linear {
                from: Colour::BLACK,
                to: Colour::WHITE,
                angle_deg: f64::INFINITY
            })
        )
        .is_err());
}

#[test]
fn graphics_survive_saving_exactly_and_old_files_still_open() {
    let (mut ed, seq, v1) = setup();
    for (n, g) in [("a", red()), ("b", star()), ("c", title())] {
        let id = ed.add_generated(n, g).unwrap();
        place(&mut ed, seq, v1, id, 0.0, 1.0);
        ed.undo().unwrap(); // takes the clip back off ...
        ed.redo().unwrap(); // ... and on again
    }
    let json = ed.project().to_json().unwrap();
    let back = Project::from_json(&json).unwrap();
    assert_eq!(&back, ed.project());
    assert_eq!(back.to_json().unwrap(), json);
    // A project with no graphics writes no generator fields, and a version 6 file opens.
    let (plain, ..) = setup();
    let pj = plain.project().to_json().unwrap();
    assert!(!pj.contains("generator"));
    let v6 = pj.replace(
        &format!("\"version\": {}", file::FORMAT_VERSION),
        "\"version\": 6",
    );
    assert_eq!(&Project::from_json(&v6).unwrap(), plain.project());
    // A broken graphic in a file is refused, not drawn.
    let bad = json.replace("\"size_pct\": 12.5", "\"size_pct\": 5000.0");
    assert!(Project::from_json(&bad).is_err());
    let bad = json.replace("#ff0000", "not-a-colour");
    assert!(Project::from_json(&bad).is_err());
}
