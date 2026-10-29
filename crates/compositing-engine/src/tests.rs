// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn flat(w: usize, h: usize, c: [u8; 4]) -> Vec<u8> {
    c.iter().copied().cycle().take(w * h * 4).collect()
}

fn px(buf: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
    let i = (y * w + x) * 4;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

#[test]
fn normal_blend_is_over_and_neutral_cases_change_nothing() {
    let layer = flat(2, 2, [200, 100, 50, 255]);
    let mut c = flat(2, 2, [10, 20, 30, 255]);
    composite(&mut c, &layer, 1.0, BlendMode::Normal);
    assert_eq!(c, layer);
    // Opacity 0 and a transparent layer change nothing.
    let mut c = flat(2, 2, [10, 20, 30, 255]);
    composite(&mut c, &layer, 0.0, BlendMode::Multiply);
    assert_eq!(px(&c, 2, 0, 0), [10, 20, 30, 255]);
    composite(
        &mut c,
        &flat(2, 2, [255, 255, 255, 0]),
        1.0,
        BlendMode::Screen,
    );
    assert_eq!(px(&c, 2, 1, 1), [10, 20, 30, 255]);
    // Half opacity is the midpoint.
    let mut c = flat(1, 1, [0, 0, 0, 255]);
    composite(
        &mut c,
        &flat(1, 1, [200, 100, 50, 255]),
        0.5,
        BlendMode::Normal,
    );
    assert_eq!(px(&c, 1, 0, 0), [100, 50, 25, 255]);
    // A transparent canvas takes the layer as it is, at the layer's opacity.
    let mut c = flat(1, 1, [0, 0, 0, 0]);
    composite(
        &mut c,
        &flat(1, 1, [200, 100, 50, 255]),
        0.5,
        BlendMode::Multiply,
    );
    let p = px(&c, 1, 0, 0);
    assert_eq!(&p[..3], &[200, 100, 50]);
    assert!((i32::from(p[3]) - 128).abs() <= 1);
}

#[test]
fn blend_modes_give_the_known_values() {
    let blend = |b: [u8; 3], s: [u8; 3], m| {
        let mut c = flat(1, 1, [b[0], b[1], b[2], 255]);
        composite(&mut c, &flat(1, 1, [s[0], s[1], s[2], 255]), 1.0, m);
        [c[0], c[1], c[2]]
    };
    // Multiply: 50% grey times 50% grey is 25%; white leaves colour; black gives black.
    assert_eq!(blend([128; 3], [128; 3], BlendMode::Multiply), [64, 64, 64]);
    assert_eq!(
        blend([90, 150, 210], [255; 3], BlendMode::Multiply),
        [90, 150, 210]
    );
    assert_eq!(
        blend([90, 150, 210], [0; 3], BlendMode::Multiply),
        [0, 0, 0]
    );
    // Screen: black leaves colour, white gives white.
    assert_eq!(
        blend([90, 150, 210], [0; 3], BlendMode::Screen),
        [90, 150, 210]
    );
    assert_eq!(
        blend([90, 150, 210], [255; 3], BlendMode::Screen),
        [255, 255, 255]
    );
    assert_eq!(
        blend([128; 3], [128; 3], BlendMode::Screen),
        [192, 192, 192]
    );
    // Darken and lighten, per channel.
    assert_eq!(
        blend([10, 200, 100], [50, 100, 100], BlendMode::Darken),
        [10, 100, 100]
    );
    assert_eq!(
        blend([10, 200, 100], [50, 100, 100], BlendMode::Lighten),
        [50, 200, 100]
    );
    // Difference and exclusion.
    assert_eq!(
        blend([200, 50, 100], [50, 200, 100], BlendMode::Difference),
        [150, 150, 0]
    );
    assert_eq!(
        blend([255, 0, 128], [255, 255, 0], BlendMode::Exclusion),
        [0, 255, 128]
    );
    // Add clips at white.
    assert_eq!(
        blend([200, 100, 0], [100, 100, 0], BlendMode::Add),
        [255, 200, 0]
    );
    // Overlay: mid grey leaves the lower layer; a bright layer over a dark one lifts it less than screen.
    let o = blend([100; 3], [128; 3], BlendMode::Overlay);
    assert!((i32::from(o[0]) - 100).abs() <= 2, "{o:?}");
    // Hard light with a mid grey layer also leaves it; overlay and hard light swap roles.
    let h = blend([100; 3], [128; 3], BlendMode::HardLight);
    assert!((i32::from(h[0]) - 100).abs() <= 2, "{h:?}");
    assert_eq!(
        blend([60, 200, 30], [220, 90, 140], BlendMode::Overlay),
        blend([220, 90, 140], [60, 200, 30], BlendMode::HardLight)
    );
    // Soft light with mid grey is neutral; with white it lightens, with black it darkens, never beyond.
    let s = blend([100; 3], [128; 3], BlendMode::SoftLight);
    assert!((i32::from(s[0]) - 100).abs() <= 2);
    assert!(blend([100; 3], [255; 3], BlendMode::SoftLight)[0] > 100);
    assert!(blend([100; 3], [0; 3], BlendMode::SoftLight)[0] < 100);
}

#[test]
fn blend_modes_use_the_layers_transparency() {
    // A half transparent multiply layer darkens half as much.
    let mut c = flat(1, 1, [200, 200, 200, 255]);
    composite(
        &mut c,
        &flat(1, 1, [0, 0, 0, 255]),
        0.5,
        BlendMode::Multiply,
    );
    assert!((i32::from(c[0]) - 100).abs() <= 1, "{c:?}");
    assert_eq!(c[3], 255);
    // Opacity folds into the layer's own alpha.
    let mut d = flat(1, 1, [200, 200, 200, 255]);
    composite(
        &mut d,
        &flat(1, 1, [0, 0, 0, 128]),
        1.0,
        BlendMode::Multiply,
    );
    assert!((i32::from(d[0]) - 100).abs() <= 2, "{d:?}");
}

#[test]
fn names_round_trip() {
    for m in BlendMode::ALL {
        assert_eq!(BlendMode::parse(m.name()), Some(m));
    }
    for s in MaskShape::ALL {
        assert_eq!(MaskShape::parse(s.name()), Some(s));
    }
    assert_eq!(BlendMode::parse("wobble"), None);
    assert_eq!(BlendMode::default(), BlendMode::Normal);
}

fn square(w: usize, h: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> Vec<u8> {
    let mut v = vec![0u8; w * h * 4];
    for y in y0..y1 {
        for x in x0..x1 {
            v[(y * w + x) * 4..][..4].copy_from_slice(&[255, 0, 0, 255]);
        }
    }
    v
}

#[test]
fn identity_transform_is_exact_and_translation_moves_whole_pixels() {
    let src = square(20, 10, 4, 2, 8, 6);
    assert_eq!(transform(&src, 20, 10, &Affine::default()), src);
    // Move right by 5 % of 20 = 1 px and down by 10 % of 10 = 1 px: exact.
    let moved = transform(
        &src,
        20,
        10,
        &Affine {
            x_pct: 5.0,
            y_pct: 10.0,
            ..Affine::default()
        },
    );
    assert_eq!(moved, square(20, 10, 5, 3, 9, 7));
    // Moving off the frame leaves nothing.
    let gone = transform(
        &src,
        20,
        10,
        &Affine {
            x_pct: 100.0,
            ..Affine::default()
        },
    );
    assert!(gone.chunks_exact(4).all(|p| p[3] == 0));
}

fn mass(buf: &[u8]) -> f64 {
    buf.chunks_exact(4).map(|p| f64::from(p[3]) / 255.0).sum()
}

#[test]
fn scale_works_about_the_middle() {
    // A 4x4 red block in the middle of a 20x20 frame (16 pixels of picture).
    let src = square(20, 20, 8, 8, 12, 12);
    let big = transform(
        &src,
        20,
        20,
        &Affine {
            scale_x_pct: 200.0,
            scale_y_pct: 200.0,
            ..Affine::default()
        },
    );
    // Doubled: about 8x8, still in the middle, with soft edges (bilinear).
    assert!((mass(&big) - 64.0).abs() < 3.0, "{}", mass(&big));
    assert_eq!(px(&big, 20, 10, 10)[3], 255);
    assert_eq!(px(&big, 20, 10, 10)[..3], [255, 0, 0]);
    assert_eq!(px(&big, 20, 4, 10)[3], 0);
    assert_eq!(px(&big, 20, 15, 10)[3], 0);
    assert!(px(&big, 20, 6, 10)[3] > 0 && px(&big, 20, 6, 10)[3] < 255);
    // Halved: about 2x2.
    let small = transform(
        &src,
        20,
        20,
        &Affine {
            scale_x_pct: 50.0,
            scale_y_pct: 50.0,
            ..Affine::default()
        },
    );
    assert!((mass(&small) - 4.0).abs() < 1.0, "{}", mass(&small));
    assert!(px(&small, 20, 10, 10)[3] > 128);
    assert_eq!(px(&small, 20, 6, 10)[3], 0);
    // Stretch one way only: 8 wide, 4 tall.
    let wide = transform(
        &src,
        20,
        20,
        &Affine {
            scale_x_pct: 200.0,
            ..Affine::default()
        },
    );
    assert!((mass(&wide) - 32.0).abs() < 2.5, "{}", mass(&wide));
    assert_eq!(px(&wide, 20, 10, 9)[3], 255);
    assert_eq!(px(&wide, 20, 10, 6)[3], 0);
    // Zero size shows nothing.
    assert!(transform(
        &src,
        20,
        20,
        &Affine {
            scale_x_pct: 0.0,
            ..Affine::default()
        }
    )
    .chunks_exact(4)
    .all(|p| p[3] == 0));
}

#[test]
fn rotation_turns_clockwise_about_the_middle() {
    // A wide bar in the middle of a square frame turns into a tall bar at 90 degrees.
    let src = square(20, 20, 4, 8, 16, 12);
    let r = transform(
        &src,
        20,
        20,
        &Affine {
            rotation_deg: 90.0,
            ..Affine::default()
        },
    );
    assert_eq!(r, square(20, 20, 8, 4, 12, 16));
    // A mark on the right edge goes to the bottom when turned clockwise.
    let dot = square(20, 20, 17, 9, 19, 11);
    let r = transform(
        &dot,
        20,
        20,
        &Affine {
            rotation_deg: 90.0,
            ..Affine::default()
        },
    );
    assert_eq!(px(&r, 20, 9, 17), [255, 0, 0, 255]);
    assert_eq!(px(&r, 20, 17, 9)[3], 0);
    // 360 degrees is a full turn: the picture is back (the transform still runs, so allow tiny edge differences).
    let full = transform(
        &src,
        20,
        20,
        &Affine {
            rotation_deg: 360.0,
            ..Affine::default()
        },
    );
    let differ = full.iter().zip(&src).filter(|(a, b)| a != b).count();
    assert!(differ < 20, "{differ}");
}

#[test]
fn transformed_edges_stay_clean() {
    // A white shape on transparent, shifted by half a pixel: edge pixels stay white with less alpha.
    let src = {
        let mut v = vec![0u8; 20 * 10 * 4];
        for y in 3..7 {
            for x in 5..15 {
                v[(y * 20 + x) * 4..][..4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        v
    };
    let moved = transform(
        &src,
        20,
        10,
        &Affine {
            x_pct: 2.5,
            ..Affine::default()
        },
    );
    let edge = px(&moved, 20, 5, 4);
    assert!(edge[3] > 100 && edge[3] < 160 && edge[0] >= 254, "{edge:?}");
}

#[test]
fn crop_cuts_the_edges() {
    let mut a = flat(20, 10, [9, 9, 9, 255]);
    crop(&mut a, 20, 10, 10.0, 25.0, 20.0, 0.0);
    assert_eq!(px(&a, 20, 1, 5)[3], 0); // left 2 px
    assert_eq!(px(&a, 20, 2, 5)[3], 255);
    assert_eq!(px(&a, 20, 14, 5)[3], 255);
    assert_eq!(px(&a, 20, 15, 5)[3], 0); // right 5 px
    assert_eq!(px(&a, 20, 10, 1)[3], 0); // top 2 px
    assert_eq!(px(&a, 20, 10, 9)[3], 255);
    let src = flat(20, 10, [9, 9, 9, 255]);
    let mut b = src.clone();
    crop(&mut b, 20, 10, 0.0, 0.0, 0.0, 0.0);
    assert_eq!(b, src);
    let mut all = src.clone();
    crop(&mut all, 20, 10, 60.0, 60.0, 0.0, 0.0);
    assert!(all.chunks_exact(4).all(|p| p[3] == 0));
}

#[test]
fn masks_show_inside_or_outside_with_a_soft_edge() {
    let spec = MaskSpec {
        shape: MaskShape::Rect,
        invert: false,
        x_pct: 50.0,
        y_pct: 50.0,
        width_pct: 50.0,
        height_pct: 50.0,
        feather_pct: 0.0,
    };
    let mut a = flat(40, 40, [9, 9, 9, 255]);
    mask(&mut a, 40, 40, &spec);
    assert_eq!(px(&a, 40, 20, 20)[3], 255);
    assert_eq!(px(&a, 40, 2, 2)[3], 0);
    assert_eq!(px(&a, 40, 12, 20)[3], 255); // just inside (10..30)
    assert_eq!(px(&a, 40, 8, 20)[3], 0);
    // Inverted.
    let mut b = flat(40, 40, [9, 9, 9, 255]);
    mask(
        &mut b,
        40,
        40,
        &MaskSpec {
            invert: true,
            ..spec
        },
    );
    assert_eq!(px(&b, 40, 20, 20)[3], 0);
    assert_eq!(px(&b, 40, 2, 2)[3], 255);
    // The two add up to the whole picture.
    for i in 0..40 * 40 {
        assert_eq!(u16::from(a[i * 4 + 3]) + u16::from(b[i * 4 + 3]), 255);
    }
    // Soft edge: the alpha falls gradually across the edge, never leaves 0..255, and is half at the edge.
    let mut c = flat(100, 100, [9, 9, 9, 255]);
    mask(
        &mut c,
        100,
        100,
        &MaskSpec {
            feather_pct: 10.0,
            ..spec
        },
    );
    let row: Vec<u8> = (20..30).map(|x| px(&c, 100, x, 50)[3]).collect();
    assert!(row.windows(2).all(|w| w[0] <= w[1]), "{row:?}");
    assert!(row[0] < 50 && row[9] > 240, "{row:?}");
    // Ellipse: the corners of its bounding box are outside, the centre and the axis tips are in.
    let e = MaskSpec {
        shape: MaskShape::Ellipse,
        width_pct: 80.0,
        height_pct: 80.0,
        ..spec
    };
    let mut d = flat(100, 100, [9, 9, 9, 255]);
    mask(&mut d, 100, 100, &e);
    assert_eq!(px(&d, 100, 50, 50)[3], 255);
    assert_eq!(px(&d, 100, 12, 12)[3], 0);
    assert_eq!(px(&d, 100, 50, 12)[3], 255);
    assert_eq!(px(&d, 100, 50, 8)[3], 0);
    // An empty shape hides everything.
    let mut z = flat(10, 10, [9, 9, 9, 255]);
    mask(
        &mut z,
        10,
        10,
        &MaskSpec {
            width_pct: 0.0,
            ..spec
        },
    );
    assert!(z.chunks_exact(4).all(|p| p[3] == 0));
}

#[test]
fn same_input_same_output() {
    let src = square(30, 20, 5, 5, 20, 15);
    let t = Affine {
        x_pct: 3.3,
        y_pct: -2.0,
        scale_x_pct: 130.0,
        scale_y_pct: 80.0,
        rotation_deg: 17.0,
    };
    assert_eq!(transform(&src, 30, 20, &t), transform(&src, 30, 20, &t));
}
