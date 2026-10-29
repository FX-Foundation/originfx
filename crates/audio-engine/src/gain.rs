// SPDX-License-Identifier: GPL-3.0-or-later
//! Decibels, gain and panning.

/// Anything quieter than this many decibels is treated as silence (and `-inf` is never produced).
pub const MIN_DB: f32 = -120.0;

/// Linear gain for a level in decibels. 0 dB is 1.0, -6.02 dB is 0.5, and anything at or below
/// [`MIN_DB`] is exactly 0.
pub fn db_to_gain(db: f32) -> f32 {
    if db <= MIN_DB {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Level in decibels for a linear gain, never lower than [`MIN_DB`].
pub fn gain_to_db(gain: f32) -> f32 {
    if gain <= 0.0 {
        MIN_DB
    } else {
        (20.0 * gain.log10()).max(MIN_DB)
    }
}

/// The left and right gains for a pan position from -1 (hard left) to 1 (hard right).
///
/// This is a **balance** law for stereo material: the centre is exactly 1.0 for both sides (no
/// level change, so an untouched clip is bit-exact), and moving away from the centre lowers the
/// far side along a constant-power curve while the near side stays at 1.0. At hard left the right
/// channel is silent.
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    // Angle 0..pi/2 across the range; the centre is pi/4.
    let theta = (p + 1.0) * std::f32::consts::FRAC_PI_4;
    let l = (theta.cos() * std::f32::consts::SQRT_2).min(1.0);
    let r = (theta.sin() * std::f32::consts::SQRT_2).min(1.0);
    // Exactly 1.0 at the centre despite rounding.
    if p == 0.0 {
        (1.0, 1.0)
    } else {
        (l.max(0.0), r.max(0.0))
    }
}

/// Apply a gain and pan to interleaved stereo samples in place. Does nothing (and so changes no
/// bits) when both are neutral.
pub fn apply_gain_pan(samples: &mut [f32], gain: f32, pan: f32) {
    let (l, r) = pan_gains(pan);
    let (l, r) = (l * gain, r * gain);
    if l == 1.0 && r == 1.0 {
        return;
    }
    for frame in samples.chunks_exact_mut(2) {
        frame[0] *= l;
        frame[1] *= r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decibels_round_trip() {
        assert_eq!(db_to_gain(0.0), 1.0);
        assert!((db_to_gain(-6.0206) - 0.5).abs() < 1e-4);
        assert!((db_to_gain(6.0206) - 2.0).abs() < 1e-3);
        assert_eq!(db_to_gain(-200.0), 0.0);
        assert_eq!(gain_to_db(0.0), MIN_DB);
        for db in [-60.0f32, -12.0, -3.0, 0.0, 3.0, 12.0] {
            assert!((gain_to_db(db_to_gain(db)) - db).abs() < 1e-3);
        }
    }

    #[test]
    fn pan_is_neutral_in_the_middle_and_silences_the_far_side() {
        assert_eq!(pan_gains(0.0), (1.0, 1.0));
        let (l, r) = pan_gains(-1.0);
        assert_eq!((l, r.abs() < 1e-6), (1.0, true));
        let (l, r) = pan_gains(1.0);
        assert!(l.abs() < 1e-6 && r == 1.0);
        // Constant power for the far side: halfway is about -3 dB... of a full swing.
        let (_, r) = pan_gains(-0.5);
        assert!(r > 0.4 && r < 0.8);
        // Out of range is clamped.
        assert_eq!(pan_gains(5.0), pan_gains(1.0));
    }

    #[test]
    fn neutral_gain_changes_no_bits() {
        let mut v = vec![0.123_456_79f32, -0.9, 0.5, 0.25];
        let before = v.clone();
        apply_gain_pan(&mut v, 1.0, 0.0);
        assert_eq!(v, before);
        apply_gain_pan(&mut v, 0.5, 0.0);
        assert_eq!(v[0], before[0] * 0.5);
    }
}
