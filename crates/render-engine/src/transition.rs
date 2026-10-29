// SPDX-License-Identifier: GPL-3.0-or-later
//! Blending two pictures into one: the picture side of transitions, and the audio crossfade
//! curves.
//!
//! Every function works on whole canvases (RGBA8, straight alpha, same size) and is a pure
//! function of its inputs, so the same transition gives the same pixels on every machine.

use timeline_engine::{Direction, TransitionKind};

use crate::scale::par_rows;

/// Blend `outgoing` into `incoming`. `progress` runs from 0 (all outgoing) to 1 (all incoming).
/// Both canvases are `w` x `h`; transparent areas stay transparent where both are.
pub fn blend(
    kind: TransitionKind,
    progress: f64,
    outgoing: &[u8],
    incoming: &[u8],
    w: usize,
    h: usize,
) -> Vec<u8> {
    debug_assert_eq!(outgoing.len(), w * h * 4);
    debug_assert_eq!(incoming.len(), w * h * 4);
    let p = progress.clamp(0.0, 1.0) as f32;
    match kind {
        TransitionKind::Dissolve => lerp_all(outgoing, incoming, w, p),
        TransitionKind::DipToBlack => dip(outgoing, incoming, w, h, p, [0, 0, 0, 255]),
        TransitionKind::DipToWhite => dip(outgoing, incoming, w, h, p, [255, 255, 255, 255]),
        TransitionKind::Wipe(d) => wipe(outgoing, incoming, w, h, p, d),
        TransitionKind::Push(d) => push(outgoing, incoming, w, h, p, d),
    }
}

/// Mix two pixels: `a` weighted `1 - t`, `b` weighted `t`, in premultiplied space so that a
/// transparent pixel contributes no colour.
#[inline]
fn lerp_px(a: &[u8], b: &[u8], t: f32, out: &mut [u8]) {
    if t <= 0.0 {
        out.copy_from_slice(a);
        return;
    }
    if t >= 1.0 {
        out.copy_from_slice(b);
        return;
    }
    let (aa, ba) = (f32::from(a[3]) / 255.0, f32::from(b[3]) / 255.0);
    let (wa, wb) = (aa * (1.0 - t), ba * t);
    let alpha = wa + wb;
    if alpha <= 0.0 {
        out.copy_from_slice(&[0, 0, 0, 0]);
        return;
    }
    for c in 0..3 {
        let v = (f32::from(a[c]) * wa + f32::from(b[c]) * wb) / alpha;
        out[c] = (v + 0.5) as u8;
    }
    out[3] = (alpha * 255.0 + 0.5) as u8;
}

fn lerp_all(a: &[u8], b: &[u8], w: usize, t: f32) -> Vec<u8> {
    let mut out = vec![0u8; a.len()];
    par_rows(&mut out, w * 4, |y, row| {
        let base = y * w * 4;
        for x in 0..w {
            let i = base + x * 4;
            lerp_px(&a[i..i + 4], &b[i..i + 4], t, &mut row[x * 4..x * 4 + 4]);
        }
    });
    out
}

fn dip(a: &[u8], b: &[u8], w: usize, h: usize, p: f32, colour: [u8; 4]) -> Vec<u8> {
    let flat = crate::composite::solid(w, h, colour);
    if p < 0.5 {
        lerp_all(a, &flat, w, p * 2.0)
    } else {
        lerp_all(&flat, b, w, (p - 0.5) * 2.0)
    }
}

fn wipe(a: &[u8], b: &[u8], w: usize, h: usize, p: f32, dir: Direction) -> Vec<u8> {
    let horizontal = matches!(dir, Direction::Left | Direction::Right);
    let length = if horizontal { w } else { h } as f32;
    // A soft edge a little wider than a pixel, so the sweep does not shimmer.
    let feather = (length * 0.01).max(2.0);
    let edge = -feather / 2.0 + p * (length + feather);
    let mut out = vec![0u8; a.len()];
    par_rows(&mut out, w * 4, |y, row| {
        let base = y * w * 4;
        for x in 0..w {
            let pos = if horizontal { x } else { y } as f32 + 0.5;
            // Distance from the edge, measured from the side the picture arrives from.
            let from_side = match dir {
                Direction::Left | Direction::Up => pos,
                Direction::Right | Direction::Down => length - pos,
            };
            let t = ((edge - from_side) / feather + 0.5).clamp(0.0, 1.0);
            let i = base + x * 4;
            lerp_px(&a[i..i + 4], &b[i..i + 4], t, &mut row[x * 4..x * 4 + 4]);
        }
    });
    out
}

fn push(a: &[u8], b: &[u8], w: usize, h: usize, p: f32, dir: Direction) -> Vec<u8> {
    let (wi, hi) = (w as i64, h as i64);
    // How far the outgoing picture has moved, and which way.
    let (dx, dy, bx, by) = match dir {
        Direction::Left => {
            let o = (p * w as f32).round() as i64;
            (o, 0, o - wi, 0)
        }
        Direction::Right => {
            let o = (p * w as f32).round() as i64;
            (-o, 0, wi - o, 0)
        }
        Direction::Up => {
            let o = (p * h as f32).round() as i64;
            (0, o, 0, o - hi)
        }
        Direction::Down => {
            let o = (p * h as f32).round() as i64;
            (0, -o, 0, hi - o)
        }
    };
    let mut out = vec![0u8; a.len()];
    par_rows(&mut out, w * 4, |y, row| {
        let yi = y as i64;
        for x in 0..w {
            let xi = x as i64;
            let (ax, ay) = (xi - dx, yi - dy);
            let src = if ax >= 0 && ax < wi && ay >= 0 && ay < hi {
                Some((a, ax, ay))
            } else {
                let (qx, qy) = (xi - bx, yi - by);
                (qx >= 0 && qx < wi && qy >= 0 && qy < hi).then_some((b, qx, qy))
            };
            if let Some((buf, sx, sy)) = src {
                let i = (sy as usize * w + sx as usize) * 4;
                row[x * 4..x * 4 + 4].copy_from_slice(&buf[i..i + 4]);
            }
        }
    });
    out
}

/// The gain of the incoming clip's sound at `progress` through a crossfade (equal power, so the
/// level stays steady across the join).
pub fn fade_in_gain(progress: f64) -> f32 {
    audio_engine::fade_in_gain(audio_engine::FadeCurve::EqualPower, progress)
}

/// The gain of the outgoing clip's sound at `progress` through a crossfade.
pub fn fade_out_gain(progress: f64) -> f32 {
    audio_engine::fade_out_gain(audio_engine::FadeCurve::EqualPower, progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composite::solid;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    fn px(buf: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * w + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn dissolve_ends_are_exact_and_middle_is_even() {
        let (a, b) = (solid(4, 2, RED), solid(4, 2, BLUE));
        assert_eq!(blend(TransitionKind::Dissolve, 0.0, &a, &b, 4, 2), a);
        assert_eq!(blend(TransitionKind::Dissolve, 1.0, &a, &b, 4, 2), b);
        let mid = blend(TransitionKind::Dissolve, 0.5, &a, &b, 4, 2);
        assert_eq!(px(&mid, 4, 0, 0), [128, 0, 128, 255]);
    }

    #[test]
    fn dissolve_ignores_the_colour_of_transparent_pixels() {
        let a = solid(2, 1, [0, 255, 0, 0]);
        let b = solid(2, 1, BLUE);
        let mid = blend(TransitionKind::Dissolve, 0.5, &a, &b, 2, 1);
        // Half transparent blue, not a muddy blue-green.
        assert_eq!(px(&mid, 2, 0, 0), [0, 0, 255, 128]);
    }

    #[test]
    fn dip_goes_through_the_colour() {
        let (a, b) = (solid(2, 2, RED), solid(2, 2, BLUE));
        let mid = blend(TransitionKind::DipToBlack, 0.5, &a, &b, 2, 2);
        assert_eq!(px(&mid, 2, 1, 1), [0, 0, 0, 255]);
        let white = blend(TransitionKind::DipToWhite, 0.5, &a, &b, 2, 2);
        assert_eq!(px(&white, 2, 0, 0), [255, 255, 255, 255]);
        let early = blend(TransitionKind::DipToBlack, 0.25, &a, &b, 2, 2);
        assert_eq!(px(&early, 2, 0, 0), [128, 0, 0, 255]);
    }

    #[test]
    fn wipe_sweeps_from_the_named_side() {
        let (a, b) = (solid(100, 4, RED), solid(100, 4, BLUE));
        let m = blend(TransitionKind::Wipe(Direction::Left), 0.5, &a, &b, 100, 4);
        assert_eq!(
            px(&m, 100, 5, 1),
            BLUE,
            "left side is already the new picture"
        );
        assert_eq!(px(&m, 100, 95, 1), RED);
        let m = blend(TransitionKind::Wipe(Direction::Right), 0.5, &a, &b, 100, 4);
        assert_eq!(px(&m, 100, 5, 1), RED);
        assert_eq!(px(&m, 100, 95, 1), BLUE);
        let (a, b) = (solid(4, 100, RED), solid(4, 100, BLUE));
        let m = blend(TransitionKind::Wipe(Direction::Up), 0.5, &a, &b, 4, 100);
        assert_eq!(px(&m, 4, 1, 5), BLUE);
        assert_eq!(px(&m, 4, 1, 95), RED);
        // The ends are exact.
        assert_eq!(
            blend(TransitionKind::Wipe(Direction::Up), 0.0, &a, &b, 4, 100),
            a
        );
        assert_eq!(
            blend(TransitionKind::Wipe(Direction::Down), 1.0, &a, &b, 4, 100),
            b
        );
    }

    #[test]
    fn push_moves_both_pictures() {
        // Left half red / right half green outgoing; all blue incoming.
        let w = 8;
        let mut a = solid(w, 1, RED);
        for x in 4..w {
            a[x * 4..x * 4 + 4].copy_from_slice(&[0, 255, 0, 255]);
        }
        let b = solid(w, 1, BLUE);
        let m = blend(TransitionKind::Push(Direction::Left), 0.5, &a, &b, w, 1);
        // Moved right by 4: new picture on the left, old picture's left half on the right.
        assert_eq!(px(&m, w, 0, 0), BLUE);
        assert_eq!(px(&m, w, 3, 0), BLUE);
        assert_eq!(px(&m, w, 4, 0), RED);
        assert_eq!(px(&m, w, 7, 0), RED);
        let m = blend(TransitionKind::Push(Direction::Right), 0.5, &a, &b, w, 1);
        assert_eq!(px(&m, w, 0, 0), [0, 255, 0, 255]);
        assert_eq!(px(&m, w, 3, 0), [0, 255, 0, 255]);
        assert_eq!(px(&m, w, 4, 0), BLUE);
    }

    #[test]
    fn crossfade_keeps_power_steady() {
        for i in 0..=10 {
            let p = f64::from(i) / 10.0;
            let (a, b) = (fade_out_gain(p), fade_in_gain(p));
            assert!((a * a + b * b - 1.0).abs() < 1e-5);
        }
        assert_eq!(fade_in_gain(0.0), 0.0);
        assert!((fade_in_gain(1.0) - 1.0).abs() < 1e-6);
    }
}
