// SPDX-License-Identifier: GPL-3.0-or-later
//! Loudness measurement: ITU-R BS.1770-4 / EBU R128 integrated loudness in LUFS, plus sample
//! peak and RMS.
//!
//! Feed samples in any size of pieces to a [`LoudnessMeter`]; the answer does not depend on how
//! they were cut up, and memory stays small however long the programme is (ten numbers per
//! second). [`measure`] does it in one call.
//!
//! Channels are weighted equally, which is right for mono and stereo. Surround weighting is not
//! implemented yet.

use crate::gain::gain_to_db;

/// Result of a measurement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// Gated programme loudness. `None` when the sound is silent (nothing above -70 LUFS).
    pub integrated_lufs: Option<f64>,
    /// The loudest 400 ms block. `None` when nothing is above -70 LUFS.
    pub momentary_max_lufs: Option<f64>,
    /// Highest absolute sample value over all channels, in dBFS.
    pub sample_peak_db: f32,
    /// Unweighted RMS over everything, in dBFS.
    pub rms_db: f32,
    pub duration_secs: f64,
}

impl Loudness {
    /// The gain in decibels that would bring the integrated loudness to `target_lufs`.
    pub fn gain_to_reach(&self, target_lufs: f64) -> Option<f32> {
        self.integrated_lufs.map(|l| (target_lufs - l) as f32)
    }

    /// The gain in decibels that would bring the sample peak to `target_db`.
    pub fn gain_to_peak(&self, target_db: f32) -> f32 {
        target_db - self.sample_peak_db
    }
}

#[derive(Clone, Copy, Default)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    fn process(&mut self, x0: f64) -> f64 {
        let y0 = self.b[0] * x0 + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x0, self.x[0]];
        self.y = [y0, self.y[0]];
        y0
    }
}

/// The two K-weighting filters (high shelf, then high pass) for any sample rate.
fn k_weighting(rate: f64) -> [Biquad; 2] {
    // Stage 1: the head-related high shelf.
    let (f0, g, q) = (
        1_681.974_450_955_533,
        3.999_843_853_973_347,
        0.707_175_236_955_419_6,
    );
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        ..Default::default()
    };
    // Stage 2: the RLB high pass.
    let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let high = Biquad {
        b: [1.0, -2.0, 1.0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        ..Default::default()
    };
    [shelf, high]
}

const OFFSET: f64 = -0.691;

fn lufs(power: f64) -> f64 {
    OFFSET + 10.0 * power.log10()
}

/// A streaming loudness meter.
pub struct LoudnessMeter {
    channels: usize,
    rate: u32,
    filters: Vec<[Biquad; 2]>,
    sub_len: usize,
    /// Sum of squares of the K-weighted signal per channel in the current 100 ms piece.
    acc: Vec<f64>,
    in_sub: usize,
    /// Weighted power of each finished 100 ms piece.
    subs: Vec<f64>,
    peak: f32,
    sum_sq: f64,
    frames: u64,
}

impl LoudnessMeter {
    /// A meter for interleaved audio with `channels` channels at `rate` Hz.
    pub fn new(rate: u32, channels: usize) -> LoudnessMeter {
        let channels = channels.max(1);
        LoudnessMeter {
            channels,
            rate,
            filters: vec![k_weighting(f64::from(rate.max(1))); channels],
            sub_len: (rate as usize / 10).max(1),
            acc: vec![0.0; channels],
            in_sub: 0,
            subs: Vec::new(),
            peak: 0.0,
            sum_sq: 0.0,
            frames: 0,
        }
    }

    /// Add interleaved samples. A partial last frame is ignored.
    pub fn push(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(self.channels) {
            for (c, &s) in frame.iter().enumerate() {
                self.peak = self.peak.max(s.abs());
                self.sum_sq += f64::from(s) * f64::from(s);
                let f = &mut self.filters[c];
                let mid = f[0].process(f64::from(s));
                let y = f[1].process(mid);
                self.acc[c] += y * y;
            }
            self.frames += 1;
            self.in_sub += 1;
            if self.in_sub == self.sub_len {
                let power: f64 = self.acc.iter().sum::<f64>() / self.sub_len as f64;
                self.subs.push(power);
                self.acc.iter_mut().for_each(|a| *a = 0.0);
                self.in_sub = 0;
            }
        }
    }

    /// The result so far. The meter can keep being fed afterwards.
    pub fn result(&self) -> Loudness {
        // 400 ms blocks every 100 ms.
        let blocks: Vec<f64> = self
            .subs
            .windows(4)
            .map(|w| w.iter().sum::<f64>() / 4.0)
            .collect();
        let above: Vec<f64> = blocks
            .iter()
            .copied()
            .filter(|&z| z > 0.0 && lufs(z) > -70.0)
            .collect();
        let integrated = if above.is_empty() {
            None
        } else {
            let mean = above.iter().sum::<f64>() / above.len() as f64;
            let gate = lufs(mean) - 10.0;
            let kept: Vec<f64> = above.iter().copied().filter(|&z| lufs(z) > gate).collect();
            (!kept.is_empty()).then(|| lufs(kept.iter().sum::<f64>() / kept.len() as f64))
        };
        let momentary = above
            .iter()
            .copied()
            .fold(None, |m: Option<f64>, z| Some(m.map_or(z, |m| m.max(z))));
        let n = self.frames as f64 * self.channels as f64;
        Loudness {
            integrated_lufs: integrated,
            momentary_max_lufs: momentary.map(lufs),
            sample_peak_db: gain_to_db(self.peak),
            rms_db: if n > 0.0 {
                gain_to_db((self.sum_sq / n).sqrt() as f32)
            } else {
                crate::gain::MIN_DB
            },
            duration_secs: self.frames as f64 / f64::from(self.rate.max(1)),
        }
    }
}

/// Measure a whole buffer of interleaved samples.
pub fn measure(samples: &[f32], rate: u32, channels: usize) -> Loudness {
    let mut m = LoudnessMeter::new(rate, channels);
    m.push(samples);
    m.result()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, secs: f64, db: f32, hz: f64) -> Vec<f32> {
        let amp = crate::gain::db_to_gain(db);
        let n = (secs * f64::from(rate)) as usize;
        (0..n)
            .flat_map(|i| {
                let v = amp
                    * (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin() as f32;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn k_weighting_matches_the_published_48k_coefficients() {
        let [shelf, high] = k_weighting(48_000.0);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
        assert!(close(shelf.b[0], 1.535_124_859_586_97));
        assert!(close(shelf.b[1], -2.691_696_189_406_38));
        assert!(close(shelf.b[2], 1.198_392_810_852_85));
        assert!(close(shelf.a[0], -1.690_659_293_182_41));
        assert!(close(shelf.a[1], 0.732_480_774_215_85));
        assert!(close(high.a[0], -1.990_047_454_833_98));
        assert!(close(high.a[1], 0.990_072_250_366_21));
    }

    #[test]
    fn a_stereo_sine_at_minus_23_is_minus_23_lufs() {
        // The reference case from EBU Tech 3341.
        let v = sine(48_000, 20.0, -23.0, 997.0);
        let l = measure(&v, 48_000, 2);
        let i = l.integrated_lufs.unwrap();
        assert!((i + 23.0).abs() < 0.1, "{i}");
        assert!((l.sample_peak_db + 23.0).abs() < 0.05);
        // RMS of a sine is 3 dB below its peak.
        assert!((l.rms_db + 26.0).abs() < 0.1, "{}", l.rms_db);
        assert!((l.duration_secs - 20.0).abs() < 1e-9);
        assert!((l.gain_to_reach(-16.0).unwrap() - 7.0).abs() < 0.1);
    }

    #[test]
    fn other_sample_rates_agree() {
        for rate in [44_100u32, 48_000, 96_000] {
            let l = measure(&sine(rate, 10.0, -20.0, 997.0), rate, 2);
            assert!((l.integrated_lufs.unwrap() + 20.0).abs() < 0.15, "{rate}");
        }
    }

    #[test]
    fn silence_has_no_loudness() {
        let l = measure(&vec![0.0; 48_000 * 2 * 3], 48_000, 2);
        assert_eq!(l.integrated_lufs, None);
        assert_eq!(l.momentary_max_lufs, None);
    }

    #[test]
    fn gating_ignores_silence_and_much_quieter_parts() {
        let mut v = sine(48_000, 10.0, -23.0, 997.0);
        v.extend(vec![0.0; 48_000 * 2 * 10]);
        let l = measure(&v, 48_000, 2);
        assert!((l.integrated_lufs.unwrap() + 23.0).abs() < 0.1);
        // 10 s at -20, then 10 s at -40 dB: the quiet part is more than 10 LU down, so ignored.
        let mut v = sine(48_000, 10.0, -20.0, 997.0);
        v.extend(sine(48_000, 10.0, -40.0, 997.0));
        let l = measure(&v, 48_000, 2);
        assert!(
            (l.integrated_lufs.unwrap() + 20.0).abs() < 0.2,
            "{:?}",
            l.integrated_lufs
        );
    }

    #[test]
    fn feeding_in_pieces_gives_the_same_answer() {
        let v = sine(48_000, 7.3, -18.0, 440.0);
        let whole = measure(&v, 48_000, 2);
        let mut m = LoudnessMeter::new(48_000, 2);
        for chunk in v.chunks(2 * 1237) {
            m.push(chunk);
        }
        assert_eq!(m.result(), whole);
    }

    #[test]
    fn mono_is_measured_too() {
        let st = sine(48_000, 10.0, -23.0, 997.0);
        let mono: Vec<f32> = st.chunks(2).map(|f| f[0]).collect();
        let l = measure(&mono, 48_000, 1);
        // One channel carries half the power of two, 3 LU quieter.
        assert!((l.integrated_lufs.unwrap() + 26.0).abs() < 0.1);
    }
}
