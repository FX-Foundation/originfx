//! Exact frame timeline for variable-frame-rate footage.
//!
//! Phone clips and screen recordings often have frames at uneven intervals.
//! A plain `time * fps` lookup then drifts. The index reads every packet's
//! timestamp from the file (no decoding, so it is quick) and keeps them in
//! display order: frame `n` is `times[n]`.

use std::path::Path;

use ffmpeg_next as ffmpeg;

use crate::decode::start_offset_secs;
use crate::error::{Error, Result};

/// Display time of every frame of one video stream, in seconds from the
/// stream start, in presentation order.
#[derive(Debug, Clone)]
pub struct FrameIndex {
    times: Vec<f64>,
    /// Typical gap between frames, used for the final frame.
    typical_gap: f64,
}

impl FrameIndex {
    /// Read the timestamps of video stream `stream_index` in `path`.
    pub fn build(path: impl AsRef<Path>, stream_index: usize) -> Result<Self> {
        crate::init();
        let mut ictx = ffmpeg::format::input(&path.as_ref())?;
        let (tb, origin) = {
            let s = ictx.stream(stream_index).ok_or(Error::NoStream("video"))?;
            let tb = s.time_base();
            (
                f64::from(tb.numerator()) / f64::from(tb.denominator().max(1)),
                start_offset_secs(&s),
            )
        };

        let mut times: Vec<f64> = Vec::new();
        for (stream, packet) in ictx.packets() {
            if stream.index() != stream_index {
                continue;
            }
            if let Some(ts) = packet.pts().or_else(|| packet.dts()) {
                times.push(ts as f64 * tb - origin);
            }
        }
        times.sort_by(|a, b| a.total_cmp(b));
        times.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        if times.is_empty() {
            return Err(Error::NoStream("video"));
        }

        let typical_gap = if times.len() > 1 {
            let mut gaps: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
            gaps.sort_by(|a, b| a.total_cmp(b));
            gaps[gaps.len() / 2].max(1e-6)
        } else {
            1.0 / 25.0
        };
        Ok(Self { times, typical_gap })
    }

    /// Cheap header-only check: does the container say the frame rate varies?
    /// (`r_frame_rate` is the lowest rate that fits every timestamp; when it
    /// differs from the average, spacing is uneven.)
    pub fn looks_variable(path: impl AsRef<Path>, stream_index: usize) -> bool {
        crate::init();
        let Ok(ictx) = ffmpeg::format::input(&path.as_ref()) else {
            return false;
        };
        let Some(s) = ictx.stream(stream_index) else {
            return false;
        };
        let (r, a) = (s.rate(), s.avg_frame_rate());
        if r.denominator() == 0 || a.denominator() == 0 || a.numerator() == 0 {
            return true; // the file does not tell us: be safe and index it
        }
        let (r, a) = (
            f64::from(r.numerator()) / f64::from(r.denominator()),
            f64::from(a.numerator()) / f64::from(a.denominator()),
        );
        (r - a).abs() > 0.005 * r.max(a)
    }

    /// Number of frames in the stream.
    pub fn len(&self) -> u64 {
        self.times.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    /// Display time of frame `index` (clamped to the last frame).
    pub fn time_of(&self, index: u64) -> f64 {
        let i = (index as usize).min(self.times.len() - 1);
        self.times[i]
    }

    /// How long frame `index` stays on screen.
    pub fn duration_of(&self, index: u64) -> f64 {
        let i = index as usize;
        match (self.times.get(i), self.times.get(i + 1)) {
            (Some(a), Some(b)) => b - a,
            _ => self.typical_gap,
        }
    }

    /// Index of the frame on screen at `secs` (the last frame whose time is `<= secs`).
    pub fn index_at(&self, secs: f64) -> u64 {
        let n = self.times.partition_point(|t| *t <= secs + 1e-6);
        n.saturating_sub(1) as u64
    }

    /// Index of the frame whose time is closest to `secs`.
    pub fn nearest(&self, secs: f64) -> u64 {
        let n = self.times.partition_point(|t| *t < secs);
        if n == 0 {
            return 0;
        }
        if n >= self.times.len() {
            return (self.times.len() - 1) as u64;
        }
        if secs - self.times[n - 1] <= self.times[n] - secs {
            (n - 1) as u64
        } else {
            n as u64
        }
    }

    /// True when frame gaps differ by more than `tolerance` (fraction of the typical gap).
    pub fn is_variable(&self, tolerance: f64) -> bool {
        self.times
            .windows(2)
            .any(|w| ((w[1] - w[0]) - self.typical_gap).abs() > self.typical_gap * tolerance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx(times: &[f64]) -> FrameIndex {
        FrameIndex {
            times: times.to_vec(),
            typical_gap: 0.1,
        }
    }

    #[test]
    fn maps_time_to_the_frame_on_screen() {
        let i = idx(&[0.0, 0.1, 0.5, 0.6]);
        assert_eq!(i.index_at(0.0), 0);
        assert_eq!(i.index_at(0.09), 0);
        assert_eq!(i.index_at(0.1), 1);
        assert_eq!(i.index_at(0.49), 1); // frame 1 holds across the gap
        assert_eq!(i.index_at(0.5), 2);
        assert_eq!(i.index_at(99.0), 3);
        assert_eq!(i.index_at(-1.0), 0);
    }

    #[test]
    fn nearest_picks_the_closest_frame() {
        let i = idx(&[0.0, 0.1, 0.5]);
        assert_eq!(i.nearest(0.04), 0);
        assert_eq!(i.nearest(0.06), 1);
        assert_eq!(i.nearest(0.4), 2);
        assert_eq!(i.nearest(9.0), 2);
    }

    #[test]
    fn detects_uneven_spacing() {
        assert!(!idx(&[0.0, 0.1, 0.2, 0.3]).is_variable(0.2));
        assert!(idx(&[0.0, 0.1, 0.5, 0.6]).is_variable(0.2));
    }
}
