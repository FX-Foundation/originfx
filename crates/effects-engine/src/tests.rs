// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn flat(w: usize, h: usize, c: [u8; 4]) -> Vec<u8> {
    c.iter().copied().cycle().take(w * h * 4).collect()
}

fn px(buf: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
    let i = (y * w + x) * 4;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

fn noise(w: usize, h: usize) -> Vec<u8> {
    let mut s = 12345u32;
    (0..w * h * 4)
        .map(|i| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            if i % 4 == 3 {
                255
            } else {
                (s >> 24) as u8
            }
        })
        .collect()
}

#[test]
fn neutral_settings_change_no_bits() {
    let src = noise(40, 30);
    for kind in Kind::ALL {
        let neutral: Vec<f64> = kind.params().iter().map(|s| s.neutral).collect();
        assert!(is_neutral(kind, &neutral), "{kind}");
        let mut a = src.clone();
        apply(kind, &neutral, &mut a, 40, 30);
        assert_eq!(a, src, "{kind} changed pixels at its neutral setting");
    }
    // Defaults: the ones that are visible by default really change something.
    for kind in [
        Kind::Invert,
        Kind::Grayscale,
        Kind::Sepia,
        Kind::Blur,
        Kind::Sharpen,
        Kind::Vignette,
    ] {
        assert!(!is_neutral(kind, &kind.defaults()), "{kind}");
    }
}

#[test]
fn colour_effects_never_touch_alpha() {
    let src = noise(16, 16);
    let mut src = src;
    for (i, a) in src.chunks_exact_mut(4).enumerate() {
        a[3] = (i * 16 % 256) as u8;
    }
    for kind in [
        Kind::Brightness,
        Kind::Contrast,
        Kind::Exposure,
        Kind::Gamma,
        Kind::Saturation,
        Kind::Hue,
        Kind::Temperature,
        Kind::Invert,
        Kind::Grayscale,
        Kind::Sepia,
        Kind::Vignette,
    ] {
        let mut a = src.clone();
        let vals: Vec<f64> = kind
            .params()
            .iter()
            .map(|s| (s.min + s.max) / 2.0 + (s.max - s.min) / 4.0)
            .collect();
        apply(kind, &vals, &mut a, 16, 16);
        assert!(
            a.chunks_exact(4)
                .zip(src.chunks_exact(4))
                .all(|(x, y)| x[3] == y[3]),
            "{kind}"
        );
    }
}

#[test]
fn brightness_contrast_exposure_gamma() {
    let mut a = flat(2, 2, [100, 100, 100, 255]);
    apply(Kind::Brightness, &[20.0], &mut a, 2, 2);
    assert_eq!(px(&a, 2, 0, 0)[0], 151); // 100 + 0.2 * 255 = 151
    let mut a = flat(2, 2, [255, 0, 128, 255]);
    apply(Kind::Brightness, &[-100.0], &mut a, 2, 2);
    assert_eq!(px(&a, 2, 0, 0), [0, 0, 0, 255]);
    // Contrast: mid grey stays, the ends move apart; -100 is flat grey.
    let mut a = flat(2, 2, [64, 128, 192, 255]);
    apply(Kind::Contrast, &[50.0], &mut a, 2, 2);
    let p = px(&a, 2, 0, 0);
    assert!(
        p[0] < 64 && (i32::from(p[1]) - 128).abs() <= 1 && p[2] > 192,
        "{p:?}"
    );
    let mut a = flat(2, 2, [10, 200, 90, 255]);
    apply(Kind::Contrast, &[-100.0], &mut a, 2, 2);
    assert_eq!(px(&a, 2, 1, 1), [128, 128, 128, 255]);
    // One stop of exposure doubles linear light: sRGB 128 becomes about 176; black stays black, white stays white.
    let mut a = flat(2, 1, [128, 0, 255, 255]);
    apply(Kind::Exposure, &[1.0], &mut a, 2, 1);
    let p = px(&a, 2, 0, 0);
    assert!(
        (i32::from(p[0]) - 176).abs() <= 1 && p[1] == 0 && p[2] == 255,
        "{p:?}"
    );
    // Gamma above 1 lifts the mid tones; 0 and 255 stay.
    let mut a = flat(2, 1, [128, 0, 255, 255]);
    apply(Kind::Gamma, &[2.0], &mut a, 2, 1);
    let p = px(&a, 2, 0, 0);
    assert!(
        (i32::from(p[0]) - 181).abs() <= 1 && p[1] == 0 && p[2] == 255,
        "{p:?}"
    );
}

#[test]
fn saturation_grayscale_sepia_invert_hue_temperature() {
    let mut a = flat(1, 1, [200, 100, 50, 255]);
    apply(Kind::Saturation, &[0.0], &mut a, 1, 1);
    let p = px(&a, 1, 0, 0);
    assert!(p[0] == p[1] && p[1] == p[2], "{p:?}");
    let luma = (0.2126 * 200.0 + 0.7152 * 100.0 + 0.0722 * 50.0f32).round() as i32;
    assert!((i32::from(p[0]) - luma).abs() <= 1);
    let mut b = flat(1, 1, [200, 100, 50, 255]);
    apply(Kind::Grayscale, &[100.0], &mut b, 1, 1);
    assert_eq!(a, b);
    // Boosting saturation pulls the channels further apart.
    let mut c = flat(1, 1, [200, 100, 50, 255]);
    apply(Kind::Saturation, &[200.0], &mut c, 1, 1);
    assert!(c[0] > 200 && c[2] < 50);
    // Sepia of white stays light and warm.
    let mut s = flat(1, 1, [255, 255, 255, 255]);
    apply(Kind::Sepia, &[100.0], &mut s, 1, 1);
    assert!(s[0] == 255 && s[2] < s[1] && s[2] < 250);
    // Invert is its own opposite.
    let src = noise(8, 8);
    let mut i = src.clone();
    apply(Kind::Invert, &[100.0], &mut i, 8, 8);
    assert_eq!(px(&i, 8, 0, 0)[0], 255 - px(&src, 8, 0, 0)[0]);
    apply(Kind::Invert, &[100.0], &mut i, 8, 8);
    assert_eq!(i, src);
    let mut half = flat(1, 1, [0, 0, 0, 255]);
    apply(Kind::Invert, &[50.0], &mut half, 1, 1);
    assert_eq!(px(&half, 1, 0, 0)[0], 128);
    // Hue: 120 degrees turns red into green; 360 would be the same as 0, and greys do not move.
    let mut r = flat(1, 1, [255, 0, 0, 255]);
    apply(Kind::Hue, &[120.0], &mut r, 1, 1);
    let p = px(&r, 1, 0, 0);
    assert!(p[1] > p[0] && p[1] > p[2], "{p:?}");
    let mut g = flat(1, 1, [90, 90, 90, 255]);
    apply(Kind::Hue, &[77.0], &mut g, 1, 1);
    assert!(px(&g, 1, 0, 0)[..3]
        .iter()
        .all(|v| (i32::from(*v) - 90).abs() <= 1));
    // Warm adds red and removes blue; cool does the opposite.
    let mut w = flat(1, 1, [100, 100, 100, 255]);
    apply(Kind::Temperature, &[100.0], &mut w, 1, 1);
    assert!(w[0] > 100 && w[2] < 100 && w[1] == 100);
    let mut c = flat(1, 1, [100, 100, 100, 255]);
    apply(Kind::Temperature, &[-100.0], &mut c, 1, 1);
    assert!(c[0] < 100 && c[2] > 100);
}

#[test]
fn blur_keeps_flat_pictures_and_spreads_an_impulse() {
    let mut a = flat(40, 30, [90, 140, 200, 255]);
    apply(Kind::Blur, &[5.0], &mut a, 40, 30);
    assert!(a.chunks_exact(4).all(|p| p == [90, 140, 200, 255]));
    // A single white pixel on black becomes a symmetric soft blob that keeps its total light.
    let (w, h) = (61, 61);
    let mut a = flat(w, h, [0, 0, 0, 255]);
    let i = (30 * w + 30) * 4;
    a[i..i + 3].copy_from_slice(&[255, 255, 255]);
    apply(Kind::Blur, &[5.0], &mut a, w, h); // sigma about 3 px
    assert!(px(&a, w, 30, 30)[0] < 255 && px(&a, w, 30, 30)[0] > 0);
    assert_eq!(px(&a, w, 28, 30), px(&a, w, 32, 30));
    assert_eq!(px(&a, w, 30, 27), px(&a, w, 30, 33));
    assert!(px(&a, w, 30, 30)[0] > px(&a, w, 33, 30)[0]);
    let total: u32 = a.chunks_exact(4).map(|p| u32::from(p[0])).sum();
    assert!((i64::from(total) - 255).abs() < 60, "{total}");
    // A bigger radius spreads further and flatter.
    let mut b = flat(w, h, [0, 0, 0, 255]);
    b[i..i + 3].copy_from_slice(&[255, 255, 255]);
    apply(Kind::Blur, &[10.0], &mut b, w, h);
    assert!(px(&b, w, 30, 30)[0] < px(&a, w, 30, 30)[0]);
}

#[test]
fn blur_does_not_darken_the_edge_of_a_transparent_shape() {
    // A white square on a transparent background: blurred edge pixels must stay white (alpha falls, colour does not).
    let (w, h) = (40, 40);
    let mut a = vec![0u8; w * h * 4];
    for y in 10..30 {
        for x in 10..30 {
            let i = (y * w + x) * 4;
            a[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
        }
    }
    apply(Kind::Blur, &[5.0], &mut a, w, h);
    let edge = px(&a, w, 9, 20);
    assert!(edge[3] > 0 && edge[3] < 255 && edge[0] >= 253, "{edge:?}");
}

#[test]
fn sharpen_leaves_flat_areas_and_strengthens_edges() {
    let mut a = flat(30, 10, [120, 120, 120, 255]);
    apply(Kind::Sharpen, &[200.0, 1.0], &mut a, 30, 10);
    assert!(a.chunks_exact(4).all(|p| p == [120, 120, 120, 255]));
    // A soft edge gets steeper: darker on the dark side, lighter on the light side.
    let (w, h) = (40, 100);
    let mut e = vec![255u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let v = if x < 18 {
                80
            } else if x < 22 {
                80 + (x as u8 - 17) * 25
            } else {
                180
            };
            let i = (y * w + x) * 4;
            e[i..i + 3].copy_from_slice(&[v, v, v]);
        }
    }
    let before = e.clone();
    apply(Kind::Sharpen, &[300.0, 2.0], &mut e, w, h);
    assert!(px(&e, w, 16, 50)[0] < px(&before, w, 16, 50)[0]);
    assert!(px(&e, w, 24, 50)[0] > px(&before, w, 24, 50)[0]);
    assert!(e.chunks_exact(4).all(|p| p[3] == 255));
}

#[test]
fn vignette_darkens_corners_only() {
    let mut a = flat(100, 60, [200, 200, 200, 255]);
    apply(Kind::Vignette, &[100.0, 50.0], &mut a, 100, 60);
    assert_eq!(px(&a, 100, 50, 30), [200, 200, 200, 255]);
    assert!(px(&a, 100, 0, 0)[0] < 20);
    assert!(px(&a, 100, 99, 59)[0] < 20);
    assert_eq!(px(&a, 100, 0, 0), px(&a, 100, 99, 59));
    // Never brighter anywhere, and alpha untouched.
    assert!(a.chunks_exact(4).all(|p| p[0] <= 200 && p[3] == 255));
    // Less amount, less dark.
    let mut b = flat(100, 60, [200, 200, 200, 255]);
    apply(Kind::Vignette, &[40.0, 50.0], &mut b, 100, 60);
    assert!(px(&b, 100, 0, 0)[0] > px(&a, 100, 0, 0)[0]);
}

#[test]
fn pixelate_makes_flat_blocks_with_the_average_colour() {
    let (w, h) = (20, 20);
    let mut a = vec![255u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let v = if (x + y) % 2 == 0 { 0 } else { 200 };
            let i = (y * w + x) * 4;
            a[i..i + 3].copy_from_slice(&[v, v, v]);
        }
    }
    apply(Kind::Pixelate, &[20.0], &mut a, w, h); // 4 px blocks
    for by in 0..5 {
        for bx in 0..5 {
            let first = px(&a, w, bx * 4, by * 4);
            for y in 0..4 {
                for x in 0..4 {
                    assert_eq!(px(&a, w, bx * 4 + x, by * 4 + y), first);
                }
            }
            assert_eq!(first[0], 100);
        }
    }
    // A block of one pixel is no change.
    let src = noise(10, 10);
    let mut b = src.clone();
    apply(Kind::Pixelate, &[5.0], &mut b, 10, 10);
    assert_eq!(b, src);
}

#[test]
fn values_are_clamped_and_missing_ones_default() {
    let mut a = flat(4, 4, [10, 10, 10, 255]);
    apply(Kind::Brightness, &[9999.0], &mut a, 4, 4);
    assert_eq!(px(&a, 4, 0, 0), [255, 255, 255, 255]);
    assert_eq!(clamp_values(Kind::Sharpen, &[]), Kind::Sharpen.defaults());
    assert_eq!(clamp_values(Kind::Gamma, &[0.0])[0], 0.1);
    // Same input, same output, every time.
    let src = noise(32, 32);
    let (mut x, mut y) = (src.clone(), src);
    apply(Kind::Blur, &[3.0], &mut x, 32, 32);
    apply(Kind::Blur, &[3.0], &mut y, 32, 32);
    assert_eq!(x, y);
}

#[test]
fn names_round_trip_and_params_are_sane() {
    for k in Kind::ALL {
        assert_eq!(Kind::parse(k.name()), Some(k));
        for (i, s) in k.params().iter().enumerate() {
            assert!(s.min <= s.default && s.default <= s.max, "{k} {}", s.name);
            assert!(s.min <= s.neutral && s.neutral <= s.max, "{k} {}", s.name);
            assert_eq!(k.param_index(s.name), Some(i));
        }
    }
    assert_eq!(Kind::parse("wobble"), None);
}
