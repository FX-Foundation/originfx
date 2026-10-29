// SPDX-License-Identifier: GPL-3.0-or-later
//! The look of a picture clip: editing, animating, cutting, saving and the rules.
use timeline_engine::*;

fn secs(s: f64) -> Time {
    Time::from_secs_f64(s)
}

fn setup() -> (Editor, SequenceId, ClipId, ClipId) {
    let mut ed = Editor::new("t");
    let seq = ed.add_sequence(SequenceSettings::default()).unwrap();
    let s = ed.project().sequence(seq).unwrap();
    let (v1, a1) = (s.video_tracks()[0].id, s.audio_tracks()[0].id);
    let asset = ed
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
    let p = ed
        .place_asset(
            seq,
            asset,
            Time::ZERO,
            Time::ZERO,
            Some(secs(10.0)),
            Some(v1),
            Some(a1),
            PlaceMode::Overwrite,
        )
        .unwrap();
    (ed, seq, p.video.unwrap(), p.audio.unwrap())
}

fn clip(ed: &Editor, seq: SequenceId, id: ClipId) -> Clip {
    ed.project()
        .sequence(seq)
        .unwrap()
        .find_clip(id)
        .unwrap()
        .1
        .clone()
}

#[test]
fn defaults_are_plain_and_values_are_checked() {
    let (mut ed, seq, v, a) = setup();
    assert!(clip(&ed, seq, v).look_at(Time::ZERO).is_plain());
    ed.set_look_value(seq, v, Param::Opacity, 40.0).unwrap();
    assert_eq!(clip(&ed, seq, v).look_at(secs(1.0)).opacity, 0.4);
    assert!(ed.set_look_value(seq, v, Param::Opacity, 101.0).is_err());
    assert!(ed.set_look_value(seq, v, Param::Opacity, f64::NAN).is_err());
    assert!(ed.set_look_value(seq, v, Param::Gain, 0.0).is_err());
    // Sound clips have no look.
    assert!(ed.set_look_value(seq, a, Param::Opacity, 50.0).is_err());
    assert!(ed.set_blend_mode(seq, a, BlendMode::Screen).is_err());
    assert!(ed.set_mask(seq, a, MaskShape::Rect, false).is_err());
    // Mask values need a mask.
    assert!(ed.set_look_value(seq, v, Param::MaskWidth, 30.0).is_err());
    ed.set_mask(seq, v, MaskShape::Ellipse, true).unwrap();
    ed.set_look_value(seq, v, Param::MaskWidth, 30.0).unwrap();
    let m = clip(&ed, seq, v).mask.unwrap();
    assert!(m.invert && m.shape == MaskShape::Ellipse && m.width == 30.0);
    // Changing the shape keeps the geometry.
    ed.set_mask(seq, v, MaskShape::Rect, false).unwrap();
    assert_eq!(clip(&ed, seq, v).mask.unwrap().width, 30.0);
    ed.remove_mask(seq, v).unwrap();
    assert!(clip(&ed, seq, v).mask.is_none());
    assert!(ed.remove_mask(seq, v).is_err());
}

#[test]
fn keyframes_move_the_look_and_fixed_values_are_refused_while_animated() {
    let (mut ed, seq, v, a) = setup();
    let t = Target::Clip(v);
    ed.set_look_value(seq, v, Param::PositionX, 10.0).unwrap();
    // First key seeds from the fixed value at the clip start.
    ed.set_keyframe(
        seq,
        t,
        Param::PositionX,
        secs(4.0),
        50.0,
        Interp::Linear,
        LinkMode::Follow,
    )
    .unwrap();
    let c = clip(&ed, seq, v);
    assert_eq!(c.look_at(Time::ZERO).affine.x_pct, 10.0);
    assert!((c.look_at(secs(2.0)).affine.x_pct - 30.0).abs() < 1e-9);
    assert_eq!(c.look_at(secs(9.0)).affine.x_pct, 50.0);
    assert!(ed.set_look_value(seq, v, Param::PositionX, 0.0).is_err());
    // Out of range and wrong kind are refused.
    assert!(ed
        .set_keyframe(
            seq,
            t,
            Param::Opacity,
            secs(1.0),
            200.0,
            Interp::Linear,
            LinkMode::Follow
        )
        .is_err());
    assert!(ed
        .set_keyframe(
            seq,
            Target::Clip(a),
            Param::Opacity,
            secs(1.0),
            5.0,
            Interp::Linear,
            LinkMode::Follow
        )
        .is_err());
    assert!(ed
        .set_keyframe(
            seq,
            t,
            Param::MaskX,
            secs(1.0),
            5.0,
            Interp::Linear,
            LinkMode::Follow
        )
        .is_err());
    // A look key never touches the linked sound clip.
    assert!(clip(&ed, seq, a).anim.is_empty());
    ed.clear_animation(seq, t, Param::PositionX, LinkMode::Follow)
        .unwrap();
    ed.set_look_value(seq, v, Param::PositionX, 0.0).unwrap();
}

#[test]
fn cutting_slices_the_look_and_removing_the_mask_drops_its_keys() {
    let (mut ed, seq, v, _a) = setup();
    let t = Target::Clip(v);
    ed.set_keyframe(
        seq,
        t,
        Param::Opacity,
        secs(0.0),
        0.0,
        Interp::Linear,
        LinkMode::Follow,
    )
    .unwrap();
    ed.set_keyframe(
        seq,
        t,
        Param::Opacity,
        secs(10.0),
        100.0,
        Interp::Linear,
        LinkMode::Follow,
    )
    .unwrap();
    ed.set_blend_mode(seq, v, BlendMode::Multiply).unwrap();
    let parts = ed.split_clip(seq, v, secs(5.0), LinkMode::Follow).unwrap();
    let s = ed.project().sequence(seq).unwrap();
    let tr = &s.video_tracks()[0];
    let clips: Vec<&Clip> = tr.clips().collect();
    assert_eq!(clips.len(), 2);
    let _ = parts;
    // Same opacity either side of the cut, and the second half carries on from 50.
    assert!((clips[0].look_at(secs(5.0)).opacity - 0.5).abs() < 1e-9);
    assert!((clips[1].look_at(secs(5.0)).opacity - 0.5).abs() < 1e-9);
    assert!((clips[1].look_at(secs(10.0)).opacity - 1.0).abs() < 1e-9);
    assert!(clips.iter().all(|c| c.blend == BlendMode::Multiply));
    // Mask keys go with the mask.
    let c2 = clips[1].id;
    ed.set_mask(seq, c2, MaskShape::Rect, false).unwrap();
    ed.set_keyframe(
        seq,
        Target::Clip(c2),
        Param::MaskX,
        secs(6.0),
        80.0,
        Interp::Linear,
        LinkMode::Follow,
    )
    .unwrap();
    ed.remove_mask(seq, c2).unwrap();
    assert!(!clip(&ed, seq, c2).anim.is_animated(Param::MaskX));
}

#[test]
fn undo_and_save_round_trip_every_setting() {
    let (mut ed, seq, v, _a) = setup();
    ed.set_look_value(seq, v, Param::ScaleX, 120.5).unwrap();
    ed.set_look_value(seq, v, Param::Rotation, -12.25).unwrap();
    ed.set_look_value(seq, v, Param::CropLeft, 7.0).unwrap();
    ed.set_blend_mode(seq, v, BlendMode::SoftLight).unwrap();
    ed.set_mask(seq, v, MaskShape::Ellipse, true).unwrap();
    ed.set_look_value(seq, v, Param::MaskFeather, 3.5).unwrap();
    ed.set_keyframe(
        seq,
        Target::Clip(v),
        Param::Opacity,
        secs(2.0),
        25.0,
        Interp::Smooth,
        LinkMode::Follow,
    )
    .unwrap();
    let json = ed.project().to_json().unwrap();
    let back = Project::from_json(&json).unwrap();
    assert_eq!(&back, ed.project());
    assert_eq!(back.to_json().unwrap(), json);
    // Undo walks back one edit at a time.
    ed.undo().unwrap();
    assert!(!clip(&ed, seq, v).anim.is_animated(Param::Opacity));
    ed.undo().unwrap();
    assert_eq!(clip(&ed, seq, v).mask.unwrap().feather, 0.0);
}

#[test]
fn a_plain_clip_saves_without_look_fields_and_old_files_load() {
    let (ed, _seq, _v, _a) = setup();
    let json = ed.project().to_json().unwrap();
    assert!(
        !json.contains("\"transform\"")
            && !json.contains("\"blend\"")
            && !json.contains("\"mask\"")
    );
    // A version-5 file (no look fields at all) still opens.
    let v5 = json.replace(
        &format!("\"version\": {}", file::FORMAT_VERSION),
        "\"version\": 5",
    );
    let p = Project::from_json(&v5).unwrap();
    assert_eq!(&p, ed.project());
    // A broken file is refused cleanly.
    let bad = json.replace(
        "\"enabled\": true,",
        "\"enabled\": true, \"blend\": \"bogus\",",
    );
    assert!(Project::from_json(&bad).is_err());
}
