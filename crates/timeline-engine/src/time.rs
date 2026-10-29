// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact time: integer ticks, frame rates, timecode and time ranges.
//!
//! Every position and length in a project is a [`Time`], an integer count of
//! ticks. There is no floating point in the model, so a cut stays on the same
//! frame after any number of edits, saves and loads.
//!
//! One second is [`TICKS_PER_SECOND`] ticks. That number divides evenly into a
//! frame of every common frame rate (23.976, 24, 25, 29.97, 30, 48, 50, 59.94,
//! 60, 120 and more) and into a sample of every common audio rate (44.1, 48,
//! 88.2, 96, 176.4 and 192 kHz), so both land exactly on a tick.

use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

/// Ticks in one second. Chosen to be divisible by every common frame and sample rate.
pub const TICKS_PER_SECOND: i64 = 254_016_000_000;

/// A point in time, or a length of time, as an integer number of ticks.
///
/// The same type is used for positions and durations, which is how editors
/// treat them. Negative values are allowed in arithmetic but never stored in a
/// valid project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Time(i64);

/// Divide rounding half up (towards positive infinity). `d` must be positive.
fn div_round(n: i128, d: i128) -> i128 {
    let q = n.div_euclid(d);
    let r = n.rem_euclid(d);
    if r * 2 >= d {
        q + 1
    } else {
        q
    }
}

impl Time {
    /// Time zero: the start of a sequence or of a source.
    pub const ZERO: Time = Time(0);

    /// Wrap a raw tick count.
    pub const fn from_ticks(ticks: i64) -> Time {
        Time(ticks)
    }

    /// The raw tick count.
    pub const fn ticks(self) -> i64 {
        self.0
    }

    /// Convert seconds to the nearest tick. NaN becomes zero and infinities saturate.
    pub fn from_secs_f64(secs: f64) -> Time {
        Time((secs * TICKS_PER_SECOND as f64).round() as i64)
    }

    /// Seconds as a float, for display and for handing to the Media Engine.
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / TICKS_PER_SECOND as f64
    }

    /// The time of frame number `frames` (counting from 0) at `rate`.
    /// Exact for every standard rate; otherwise rounded to the nearest tick.
    pub fn from_frames(frames: i64, rate: FrameRate) -> Time {
        let n = i128::from(frames) * i128::from(TICKS_PER_SECOND) * i128::from(rate.den);
        Time(div_round(n, i128::from(rate.num)) as i64)
    }

    /// The frame this time falls inside (rounds down).
    pub fn to_frames_floor(self, rate: FrameRate) -> i64 {
        let n = i128::from(self.0) * i128::from(rate.num);
        n.div_euclid(i128::from(TICKS_PER_SECOND) * i128::from(rate.den)) as i64
    }

    /// The nearest frame boundary, as a frame number.
    pub fn to_frames_round(self, rate: FrameRate) -> i64 {
        let n = i128::from(self.0) * i128::from(rate.num);
        div_round(n, i128::from(TICKS_PER_SECOND) * i128::from(rate.den)) as i64
    }

    /// This time moved to the nearest frame boundary of `rate`.
    pub fn snap(self, rate: FrameRate) -> Time {
        Time::from_frames(self.to_frames_round(rate), rate)
    }

    /// True when this time sits exactly on a frame boundary of `rate`.
    pub fn is_on_frame(self, rate: FrameRate) -> bool {
        self == self.snap(rate)
    }

    /// The time of audio sample number `samples` at `sample_rate` Hz.
    pub fn from_samples(samples: i64, sample_rate: u32) -> Time {
        let n = i128::from(samples) * i128::from(TICKS_PER_SECOND);
        Time(div_round(n, i128::from(sample_rate.max(1))) as i64)
    }

    /// The nearest audio sample number at `sample_rate` Hz.
    pub fn to_samples_round(self, sample_rate: u32) -> i64 {
        let n = i128::from(self.0) * i128::from(sample_rate);
        div_round(n, i128::from(TICKS_PER_SECOND)) as i64
    }

    /// Absolute value.
    pub fn abs(self) -> Time {
        Time(self.0.abs())
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3}s", self.as_secs_f64())
    }
}

impl Add for Time {
    type Output = Time;
    fn add(self, rhs: Time) -> Time {
        Time(self.0 + rhs.0)
    }
}

impl Sub for Time {
    type Output = Time;
    fn sub(self, rhs: Time) -> Time {
        Time(self.0 - rhs.0)
    }
}

impl Neg for Time {
    type Output = Time;
    fn neg(self) -> Time {
        Time(-self.0)
    }
}

impl AddAssign for Time {
    fn add_assign(&mut self, rhs: Time) {
        self.0 += rhs.0;
    }
}

impl SubAssign for Time {
    fn sub_assign(&mut self, rhs: Time) {
        self.0 -= rhs.0;
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// A frame rate as a reduced fraction, frames per second = `num / den`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameRate {
    num: u32,
    den: u32,
}

impl FrameRate {
    /// 23.976 fps (24000/1001).
    pub const FPS_23_976: FrameRate = FrameRate::raw(24000, 1001);
    /// 24 fps.
    pub const FPS_24: FrameRate = FrameRate::raw(24, 1);
    /// 25 fps.
    pub const FPS_25: FrameRate = FrameRate::raw(25, 1);
    /// 29.97 fps (30000/1001).
    pub const FPS_29_97: FrameRate = FrameRate::raw(30000, 1001);
    /// 30 fps.
    pub const FPS_30: FrameRate = FrameRate::raw(30, 1);
    /// 48 fps.
    pub const FPS_48: FrameRate = FrameRate::raw(48, 1);
    /// 50 fps.
    pub const FPS_50: FrameRate = FrameRate::raw(50, 1);
    /// 59.94 fps (60000/1001).
    pub const FPS_59_94: FrameRate = FrameRate::raw(60000, 1001);
    /// 60 fps.
    pub const FPS_60: FrameRate = FrameRate::raw(60, 1);

    const fn raw(num: u32, den: u32) -> FrameRate {
        FrameRate { num, den }
    }

    /// A frame rate from a fraction. `None` if either part is zero. The fraction is reduced.
    pub fn new(num: u32, den: u32) -> Option<FrameRate> {
        if num == 0 || den == 0 {
            return None;
        }
        let g = gcd(num, den);
        Some(FrameRate::raw(num / g, den / g))
    }

    /// The closest sensible rate for a float such as `29.97`. Snaps to the
    /// broadcast family (`n * 1000/1001`) when it is within 0.2%, otherwise
    /// uses thousandths. `None` for values that are not positive and finite.
    pub fn from_f64(fps: f64) -> Option<FrameRate> {
        if !fps.is_finite() || fps <= 0.0 {
            return None;
        }
        let ntsc_base = (fps * 1001.0 / 1000.0).round();
        if ntsc_base >= 1.0 && ((ntsc_base * 1000.0 / 1001.0) - fps).abs() < 0.002 * fps {
            // 24 -> 24000/1001 only when the input is not itself the integer rate.
            if (fps - fps.round()).abs() > 0.0005 {
                return FrameRate::new(ntsc_base as u32 * 1000, 1001);
            }
        }
        FrameRate::new((fps * 1000.0).round() as u32, 1000)
    }

    /// Numerator of the fraction.
    pub fn num(self) -> u32 {
        self.num
    }

    /// Denominator of the fraction.
    pub fn den(self) -> u32 {
        self.den
    }

    /// Frames per second as a float.
    pub fn as_f64(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }

    /// The length of one frame.
    pub fn frame_duration(self) -> Time {
        Time::from_frames(1, self)
    }

    /// True when one frame is a whole number of ticks, so frame boundaries are exact.
    pub fn is_tick_exact(self) -> bool {
        (i128::from(TICKS_PER_SECOND) * i128::from(self.den)) % i128::from(self.num) == 0
    }

    /// The whole number of frames counted per timecode second (24 for 23.976, 30 for 29.97).
    pub fn nominal(self) -> u32 {
        self.as_f64().round() as u32
    }

    /// True for 29.97 and 59.94, the only rates that have drop-frame timecode.
    pub fn supports_drop_frame(self) -> bool {
        self.den == 1001 && (self.num == 30000 || self.num == 60000)
    }
}

impl fmt::Display for FrameRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            let s = format!("{:.3}", self.as_f64());
            write!(f, "{}", s.trim_end_matches('0').trim_end_matches('.'))
        }
    }
}

/// A SMPTE timecode, `HH:MM:SS:FF` (or `HH:MM:SS;FF` for drop frame).
///
/// Drop-frame timecode skips frame *labels* (never frames) so that a
/// 29.97 fps clock stays in step with the wall clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timecode {
    pub hours: u32,
    pub minutes: u32,
    pub seconds: u32,
    pub frames: u32,
    pub drop_frame: bool,
}

impl Timecode {
    /// The timecode of frame number `count` (counting from 0).
    ///
    /// `None` if `count` is negative, or if `drop_frame` is asked for at a rate that has none.
    pub fn from_frame_count(count: i64, rate: FrameRate, drop_frame: bool) -> Option<Timecode> {
        if count < 0 || (drop_frame && !rate.supports_drop_frame()) {
            return None;
        }
        let n = i64::from(rate.nominal());
        if n == 0 {
            return None;
        }
        let mut adjusted = count;
        if drop_frame {
            let drop = n / 15;
            let per_minute = n * 60 - drop;
            let per_ten_minutes = n * 600 - drop * 9;
            let tens = count / per_ten_minutes;
            let rest = count % per_ten_minutes;
            adjusted = count + drop * 9 * tens;
            if rest >= drop {
                adjusted += drop * ((rest - drop) / per_minute);
            }
        }
        Some(Timecode {
            hours: (adjusted / (n * 3600)) as u32,
            minutes: ((adjusted / (n * 60)) % 60) as u32,
            seconds: ((adjusted / n) % 60) as u32,
            frames: (adjusted % n) as u32,
            drop_frame,
        })
    }

    /// The timecode of a time, rounded to the nearest frame.
    pub fn from_time(time: Time, rate: FrameRate, drop_frame: bool) -> Option<Timecode> {
        Timecode::from_frame_count(time.to_frames_round(rate), rate, drop_frame)
    }

    /// The frame number (counting from 0) this timecode names.
    ///
    /// `None` if the timecode does not exist at this rate (frame number too large, or a
    /// label that drop-frame skips).
    pub fn frame_count(&self, rate: FrameRate) -> Option<i64> {
        let n = i64::from(rate.nominal());
        if n == 0 || i64::from(self.frames) >= n || self.seconds >= 60 || self.minutes >= 60 {
            return None;
        }
        if self.drop_frame && !rate.supports_drop_frame() {
            return None;
        }
        let total_minutes = i64::from(self.hours) * 60 + i64::from(self.minutes);
        let mut count = n * 3600 * i64::from(self.hours)
            + n * 60 * i64::from(self.minutes)
            + n * i64::from(self.seconds)
            + i64::from(self.frames);
        if self.drop_frame {
            let drop = n / 15;
            if self.seconds == 0
                && i64::from(self.frames) < drop
                && !self.minutes.is_multiple_of(10)
            {
                return None; // this label is skipped
            }
            count -= drop * (total_minutes - total_minutes / 10);
        }
        Some(count)
    }

    /// The time of the start of this timecode's frame.
    pub fn to_time(&self, rate: FrameRate) -> Option<Time> {
        self.frame_count(rate).map(|c| Time::from_frames(c, rate))
    }

    /// Parse `HH:MM:SS:FF` or `HH:MM:SS;FF`. A `;` or `.` before the frames means drop frame.
    pub fn parse(text: &str) -> Option<Timecode> {
        let text = text.trim();
        let drop_frame = text.contains(';') || text.contains('.');
        let parts: Vec<&str> = text.split([':', ';', '.']).collect();
        if parts.len() != 4 {
            return None;
        }
        let num = |s: &str| s.parse::<u32>().ok();
        Some(Timecode {
            hours: num(parts[0])?,
            minutes: num(parts[1])?,
            seconds: num(parts[2])?,
            frames: num(parts[3])?,
            drop_frame,
        })
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sep = if self.drop_frame { ';' } else { ':' };
        write!(
            f,
            "{:02}:{:02}:{:02}{}{:02}",
            self.hours, self.minutes, self.seconds, sep, self.frames
        )
    }
}

/// A half-open span of time, `start` included and `end` excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeRange {
    pub start: Time,
    pub end: Time,
}

impl TimeRange {
    /// A range from `start` to `end`. `None` if `end` is before `start`.
    pub fn new(start: Time, end: Time) -> Option<TimeRange> {
        (end >= start).then_some(TimeRange { start, end })
    }

    /// A range from `start` lasting `duration`. `None` if `duration` is negative.
    pub fn from_start_duration(start: Time, duration: Time) -> Option<TimeRange> {
        TimeRange::new(start, start + duration)
    }

    /// Length of the range.
    pub fn duration(&self) -> Time {
        self.end - self.start
    }

    /// True when the range contains no time.
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }

    /// True when `t` is inside the range (end excluded).
    pub fn contains(&self, t: Time) -> bool {
        t >= self.start && t < self.end
    }

    /// True when `other` lies completely inside this range.
    pub fn contains_range(&self, other: TimeRange) -> bool {
        other.start >= self.start && other.end <= self.end
    }

    /// True when the two ranges share some time (touching ends do not count).
    pub fn overlaps(&self, other: TimeRange) -> bool {
        self.start < other.end && other.start < self.end
    }

    /// The shared part of two ranges, if any.
    pub fn intersect(&self, other: TimeRange) -> Option<TimeRange> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        (end > start).then_some(TimeRange { start, end })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATES: [FrameRate; 9] = [
        FrameRate::FPS_23_976,
        FrameRate::FPS_24,
        FrameRate::FPS_25,
        FrameRate::FPS_29_97,
        FrameRate::FPS_30,
        FrameRate::FPS_48,
        FrameRate::FPS_50,
        FrameRate::FPS_59_94,
        FrameRate::FPS_60,
    ];

    #[test]
    fn every_standard_rate_is_tick_exact() {
        for r in RATES {
            assert!(r.is_tick_exact(), "{r} is not tick exact");
        }
        // 120 and 119.88 too, and the unusual cinema rates.
        for (n, d) in [(120, 1), (120000, 1001), (72, 1), (100, 1), (240, 1)] {
            assert!(FrameRate::new(n, d).unwrap().is_tick_exact(), "{n}/{d}");
        }
    }

    #[test]
    fn every_standard_sample_rate_is_tick_exact() {
        for sr in [
            8000, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000,
        ] {
            assert_eq!(TICKS_PER_SECOND % i64::from(sr), 0, "{sr} Hz");
            assert_eq!(
                Time::from_samples(1, sr).ticks(),
                TICKS_PER_SECOND / i64::from(sr)
            );
            assert_eq!(Time::from_samples(sr as i64 * 10, sr).as_secs_f64(), 10.0);
        }
    }

    #[test]
    fn frames_round_trip_exactly() {
        for r in RATES {
            for frame in [0, 1, 2, 23, 24, 1000, 107_892, 1_000_000] {
                let t = Time::from_frames(frame, r);
                assert_eq!(t.to_frames_floor(r), frame, "{r} frame {frame}");
                assert_eq!(t.to_frames_round(r), frame);
                assert!(t.is_on_frame(r));
            }
        }
    }

    #[test]
    fn floor_and_round_differ_inside_a_frame() {
        let r = FrameRate::FPS_25;
        let t = Time::from_frames(10, r) + Time::from_ticks(r.frame_duration().ticks() * 3 / 4);
        assert_eq!(t.to_frames_floor(r), 10);
        assert_eq!(t.to_frames_round(r), 11);
        assert_eq!(t.snap(r), Time::from_frames(11, r));
        assert!(!t.is_on_frame(r));
    }

    #[test]
    fn negative_times_floor_correctly() {
        let r = FrameRate::FPS_24;
        let t = Time::from_frames(-1, r) + Time::from_ticks(1);
        assert_eq!(t.to_frames_floor(r), -1);
    }

    #[test]
    fn seconds_conversion() {
        assert_eq!(Time::from_secs_f64(1.0).ticks(), TICKS_PER_SECOND);
        assert_eq!(Time::from_secs_f64(f64::NAN), Time::ZERO);
        assert!((Time::from_secs_f64(3723.5).as_secs_f64() - 3723.5).abs() < 1e-9);
    }

    #[test]
    fn frame_rate_from_float() {
        assert_eq!(FrameRate::from_f64(29.97), Some(FrameRate::FPS_29_97));
        assert_eq!(FrameRate::from_f64(23.976), Some(FrameRate::FPS_23_976));
        assert_eq!(FrameRate::from_f64(59.94), Some(FrameRate::FPS_59_94));
        assert_eq!(FrameRate::from_f64(24.0), Some(FrameRate::FPS_24));
        assert_eq!(FrameRate::from_f64(25.0), Some(FrameRate::FPS_25));
        assert_eq!(FrameRate::from_f64(30.0), Some(FrameRate::FPS_30));
        assert_eq!(FrameRate::from_f64(60.0), Some(FrameRate::FPS_60));
        assert_eq!(FrameRate::from_f64(12.5), FrameRate::new(25, 2));
        assert_eq!(FrameRate::from_f64(0.0), None);
        assert_eq!(FrameRate::from_f64(f64::NAN), None);
    }

    #[test]
    fn frame_rate_is_reduced_and_displayed() {
        assert_eq!(FrameRate::new(48, 2), Some(FrameRate::FPS_24));
        assert_eq!(FrameRate::new(0, 1), None);
        assert_eq!(FrameRate::FPS_24.to_string(), "24");
        assert_eq!(FrameRate::FPS_29_97.to_string(), "29.97");
        assert_eq!(FrameRate::FPS_23_976.to_string(), "23.976");
    }

    #[test]
    fn non_drop_timecode() {
        let r = FrameRate::FPS_24;
        let tc = Timecode::from_frame_count(24 * 3600 + 24 * 61 + 5, r, false).unwrap();
        assert_eq!(tc.to_string(), "01:01:01:05");
        assert_eq!(tc.frame_count(r), Some(24 * 3600 + 24 * 61 + 5));
        // 23.976 counts 24 frames per timecode second.
        let tc = Timecode::from_frame_count(24, FrameRate::FPS_23_976, false).unwrap();
        assert_eq!(tc.to_string(), "00:00:01:00");
    }

    #[test]
    fn drop_frame_known_values() {
        let r = FrameRate::FPS_29_97;
        let tc = |n| Timecode::from_frame_count(n, r, true).unwrap().to_string();
        assert_eq!(tc(0), "00:00:00;00");
        assert_eq!(tc(1799), "00:00:59;29");
        // Labels ;00 and ;01 are skipped at the start of minute one.
        assert_eq!(tc(1800), "00:01:00;02");
        // Every tenth minute keeps its labels.
        assert_eq!(tc(17_982), "00:10:00;00");
        // One hour of drop-frame timecode is 107 892 frames.
        assert_eq!(tc(107_892), "01:00:00;00");
        assert_eq!(tc(107_891), "00:59:59;29");
    }

    #[test]
    fn drop_frame_59_94_known_values() {
        let r = FrameRate::FPS_59_94;
        let tc = |n| Timecode::from_frame_count(n, r, true).unwrap().to_string();
        assert_eq!(tc(3599), "00:00:59;59");
        assert_eq!(tc(3600), "00:01:00;04"); // four labels skipped at 59.94
        assert_eq!(tc(215_784), "01:00:00;00");
    }

    #[test]
    fn drop_frame_round_trips_every_frame_of_an_hour() {
        let r = FrameRate::FPS_29_97;
        for count in 0..=110_000 {
            let tc = Timecode::from_frame_count(count, r, true).unwrap();
            assert_eq!(tc.frame_count(r), Some(count), "{tc}");
        }
    }

    #[test]
    fn drop_frame_labels_are_strictly_increasing() {
        let r = FrameRate::FPS_29_97;
        let mut last = String::new();
        for count in 0..40_000 {
            let tc = Timecode::from_frame_count(count, r, true)
                .unwrap()
                .to_string();
            assert!(tc > last, "{tc} after {last}");
            last = tc;
        }
    }

    #[test]
    fn skipped_drop_frame_labels_do_not_exist() {
        let r = FrameRate::FPS_29_97;
        assert_eq!(Timecode::parse("00:01:00;00").unwrap().frame_count(r), None);
        assert_eq!(Timecode::parse("00:01:00;01").unwrap().frame_count(r), None);
        assert_eq!(
            Timecode::parse("00:01:00;02").unwrap().frame_count(r),
            Some(1800)
        );
        // Minute ten is not dropped.
        assert_eq!(
            Timecode::parse("00:10:00;00").unwrap().frame_count(r),
            Some(17_982)
        );
    }

    #[test]
    fn drop_frame_is_refused_where_it_does_not_exist() {
        assert!(Timecode::from_frame_count(10, FrameRate::FPS_24, true).is_none());
        assert!(Timecode::from_frame_count(10, FrameRate::FPS_25, true).is_none());
        assert!(Timecode::from_frame_count(-1, FrameRate::FPS_25, false).is_none());
    }

    #[test]
    fn timecode_parse_and_display() {
        let tc = Timecode::parse("01:02:03:04").unwrap();
        assert_eq!((tc.hours, tc.minutes, tc.seconds, tc.frames), (1, 2, 3, 4));
        assert!(!tc.drop_frame);
        assert_eq!(tc.to_string(), "01:02:03:04");
        assert!(Timecode::parse("01:02:03;04").unwrap().drop_frame);
        assert!(Timecode::parse("1:2:3").is_none());
        assert!(Timecode::parse("aa:bb:cc:dd").is_none());
        // Frames beyond the rate do not exist.
        assert_eq!(
            Timecode::parse("00:00:00:30")
                .unwrap()
                .frame_count(FrameRate::FPS_30),
            None
        );
    }

    #[test]
    fn timecode_to_time_matches_frames() {
        let r = FrameRate::FPS_25;
        let tc = Timecode::parse("00:00:10:12").unwrap();
        assert_eq!(tc.to_time(r), Some(Time::from_frames(262, r)));
    }

    #[test]
    fn range_logic() {
        let t = |s: i64| Time::from_ticks(s);
        let a = TimeRange::new(t(10), t(20)).unwrap();
        let b = TimeRange::new(t(20), t(30)).unwrap();
        let c = TimeRange::new(t(15), t(25)).unwrap();
        assert!(!a.overlaps(b), "touching ranges do not overlap");
        assert!(a.overlaps(c) && c.overlaps(b));
        assert_eq!(a.intersect(c), TimeRange::new(t(15), t(20)));
        assert_eq!(a.intersect(b), None);
        assert!(a.contains(t(10)) && !a.contains(t(20)));
        assert!(TimeRange::new(t(0), t(100)).unwrap().contains_range(a));
        assert_eq!(a.duration(), t(10));
        assert!(TimeRange::new(t(5), t(4)).is_none());
        assert!(TimeRange::new(t(5), t(5)).unwrap().is_empty());
    }

    #[test]
    fn arithmetic() {
        let a = Time::from_ticks(10);
        let b = Time::from_ticks(3);
        assert_eq!(a + b, Time::from_ticks(13));
        assert_eq!(a - b, Time::from_ticks(7));
        assert_eq!(-a, Time::from_ticks(-10));
        let mut c = a;
        c += b;
        c -= Time::from_ticks(1);
        assert_eq!(c, Time::from_ticks(12));
        assert_eq!(Time::from_ticks(-4).abs(), Time::from_ticks(4));
    }
}
