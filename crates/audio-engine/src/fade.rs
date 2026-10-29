// SPDX-License-Identifier: GPL-3.0-or-later
//! Fade curves.

/// The shape of a fade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FadeCurve {
    /// Level rises in a straight line. Sounds like it dips when two fades cross.
    Linear,
    /// Power rises in a straight line (a quarter sine). Two of these crossing keep the level steady.
    #[default]
    EqualPower,
    /// Starts and ends gently (a raised cosine). Good for fading music in and out.
    Smooth,
}

impl FadeCurve {
    pub fn as_str(self) -> &'static str {
        match self {
            FadeCurve::Linear => "linear",
            FadeCurve::EqualPower => "equal-power",
            FadeCurve::Smooth => "smooth",
        }
    }

    pub fn parse(name: &str) -> Option<FadeCurve> {
        Some(match name {
            "linear" => FadeCurve::Linear,
            "equal-power" | "power" => FadeCurve::EqualPower,
            "smooth" => FadeCurve::Smooth,
            _ => return None,
        })
    }
}

/// Gain at fraction `t` (0 to 1) of a fade **in**: 0 at the start, 1 at the end.
pub fn fade_in_gain(curve: FadeCurve, t: f64) -> f32 {
    let t = t.clamp(0.0, 1.0);
    (match curve {
        FadeCurve::Linear => t,
        FadeCurve::EqualPower => (t * std::f64::consts::FRAC_PI_2).sin(),
        FadeCurve::Smooth => 0.5 - 0.5 * (t * std::f64::consts::PI).cos(),
    }) as f32
}

/// Gain at fraction `t` of a fade **out**: 1 at the start, 0 at the end.
pub fn fade_out_gain(curve: FadeCurve, t: f64) -> f32 {
    fade_in_gain(curve, 1.0 - t.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_curve_runs_from_silence_to_full() {
        for c in [FadeCurve::Linear, FadeCurve::EqualPower, FadeCurve::Smooth] {
            assert_eq!(fade_in_gain(c, 0.0), 0.0);
            assert!((fade_in_gain(c, 1.0) - 1.0).abs() < 1e-6);
            assert!((fade_out_gain(c, 0.0) - 1.0).abs() < 1e-6);
            assert_eq!(fade_out_gain(c, 1.0), 0.0);
            let mut last = -1.0;
            for i in 0..=100 {
                let g = fade_in_gain(c, f64::from(i) / 100.0);
                assert!(g >= last, "{c:?} must never fall");
                last = g;
            }
        }
    }

    #[test]
    fn the_midpoints_are_what_the_names_say() {
        assert_eq!(fade_in_gain(FadeCurve::Linear, 0.5), 0.5);
        assert!((fade_in_gain(FadeCurve::Smooth, 0.5) - 0.5).abs() < 1e-6);
        assert!(
            (fade_in_gain(FadeCurve::EqualPower, 0.5) - std::f32::consts::FRAC_1_SQRT_2).abs()
                < 1e-5
        );
        // Equal power: the in and out of a crossing pair always add up to the same power.
        for i in 0..=10 {
            let t = f64::from(i) / 10.0;
            let (a, b) = (
                fade_in_gain(FadeCurve::EqualPower, t),
                fade_out_gain(FadeCurve::EqualPower, t),
            );
            assert!((a * a + b * b - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn names_round_trip() {
        for c in [FadeCurve::Linear, FadeCurve::EqualPower, FadeCurve::Smooth] {
            assert_eq!(FadeCurve::parse(c.as_str()), Some(c));
        }
        assert_eq!(FadeCurve::parse("wobbly"), None);
    }
}
