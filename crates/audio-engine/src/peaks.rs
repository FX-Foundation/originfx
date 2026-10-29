// SPDX-License-Identifier: GPL-3.0-or-later
//! Min and max values for drawing a waveform.

/// The lowest and highest sample of a stretch, over all channels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peak {
    pub min: f32,
    pub max: f32,
}

/// Summarise interleaved samples into one [`Peak`] per `frames_per_bucket` sample frames.
/// The last bucket may cover fewer frames. A zero bucket size or zero channels gives nothing.
pub fn peaks(samples: &[f32], channels: usize, frames_per_bucket: usize) -> Vec<Peak> {
    if channels == 0 || frames_per_bucket == 0 {
        return Vec::new();
    }
    samples
        .chunks(channels * frames_per_bucket)
        .map(|chunk| {
            let mut p = Peak {
                min: f32::INFINITY,
                max: f32::NEG_INFINITY,
            };
            for &s in chunk {
                p.min = p.min.min(s);
                p.max = p.max.max(s);
            }
            p
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_cover_all_channels() {
        // Stereo: 4 frames.
        let v = [0.1, -0.5, 0.2, 0.9, -0.3, 0.0, 0.4, 0.4];
        let p = peaks(&v, 2, 2);
        assert_eq!(p.len(), 2);
        assert_eq!(
            p[0],
            Peak {
                min: -0.5,
                max: 0.9
            }
        );
        assert_eq!(
            p[1],
            Peak {
                min: -0.3,
                max: 0.4
            }
        );
        assert!(peaks(&v, 0, 2).is_empty());
        assert!(peaks(&v, 2, 0).is_empty());
        // A partial last bucket.
        assert_eq!(peaks(&v, 2, 3).len(), 2);
    }
}
