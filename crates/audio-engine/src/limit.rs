// SPDX-License-Identifier: GPL-3.0-or-later
//! Keeping the master output inside full scale.
//!
//! Both limiters look at one sample at a time, so the result for a sample never depends on the
//! samples around it. That is what lets the Render Engine mix any range of a timeline and get
//! the same samples however the range is cut into blocks.

/// How the master output is kept within -1 to 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Limiter {
    /// Cut anything above full scale flat. Exact below full scale, harsh above.
    #[default]
    HardClip,
    /// No limiting at all: samples may go above full scale. For measuring a mix, not for output.
    Off,
    /// Leave everything below 0.8 untouched and bend the rest smoothly towards 1.0, so loud
    /// peaks are rounded instead of flat-topped. Never exceeds 1.0.
    SoftClip,
}

/// Knee: where soft clipping starts.
const KNEE: f32 = 0.8;

/// Limit one sample.
#[inline]
pub fn limit_sample(x: f32, limiter: Limiter) -> f32 {
    match limiter {
        Limiter::HardClip => x.clamp(-1.0, 1.0),
        Limiter::Off => x,
        Limiter::SoftClip => {
            let a = x.abs();
            if a <= KNEE {
                x
            } else {
                // Maps KNEE..inf onto KNEE..1 with matching slope at the knee.
                let room = 1.0 - KNEE;
                let y = KNEE + room * ((a - KNEE) / room).tanh();
                y.copysign(x)
            }
        }
    }
}

/// Limit a block of samples in place.
pub fn apply_limiter(samples: &mut [f32], limiter: Limiter) {
    for s in samples {
        *s = limit_sample(*s, limiter);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_clip_is_exact_below_full_scale() {
        assert_eq!(limit_sample(0.99, Limiter::HardClip), 0.99);
        assert_eq!(limit_sample(3.0, Limiter::HardClip), 1.0);
        assert_eq!(limit_sample(-3.0, Limiter::HardClip), -1.0);
    }

    #[test]
    fn soft_clip_is_smooth_monotonic_and_bounded() {
        assert_eq!(limit_sample(0.5, Limiter::SoftClip), 0.5);
        assert_eq!(limit_sample(0.8, Limiter::SoftClip), 0.8);
        let mut last = 0.0;
        for i in 0..=2000 {
            let x = i as f32 / 100.0;
            let y = limit_sample(x, Limiter::SoftClip);
            assert!(y >= last && y <= 1.0, "{x} -> {y}");
            last = y;
        }
        assert!(limit_sample(100.0, Limiter::SoftClip) <= 1.0);
        assert_eq!(
            limit_sample(-0.9, Limiter::SoftClip),
            -limit_sample(0.9, Limiter::SoftClip)
        );
        // No jump at the knee.
        let below = limit_sample(0.8, Limiter::SoftClip);
        let above = limit_sample(0.800_01, Limiter::SoftClip);
        assert!((above - below).abs() < 1e-4);
    }
}
