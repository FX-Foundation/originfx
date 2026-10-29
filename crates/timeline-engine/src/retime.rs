// SPDX-License-Identifier: GPL-3.0-or-later
//! Speed, reverse and transitions: the data that changes *when* a clip's media plays and how
//! one clip hands over to the next.
//!
//! Nothing here touches pixels or samples. A [`Speed`] and a reverse flag say how a clip's
//! timeline time maps to media time; a [`Transition`] says how the clip before blends into
//! this one. The Render Engine does the blending.

use crate::time::Time;

/// The slowest and fastest speeds a clip can have (1/100 and 100 times).
pub const MIN_SPEED: f64 = 0.01;
pub const MAX_SPEED: f64 = 100.0;

/// The longest a transition may be, in seconds.
pub const MAX_TRANSITION_SECS: f64 = 60.0;

/// An exact playback speed: `num / den` times normal. Stored as a reduced fraction so that
/// 50%, 200% and 1/3 are exact and never drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Speed {
    num: u32,
    den: u32,
}

impl Speed {
    /// Normal speed.
    pub const NORMAL: Speed = Speed { num: 1, den: 1 };

    /// The speed `num / den`, or `None` if either is zero or the speed is outside
    /// [`MIN_SPEED`]..=[`MAX_SPEED`].
    pub fn new(num: u32, den: u32) -> Option<Speed> {
        if num == 0 || den == 0 {
            return None;
        }
        let g = gcd(num, den);
        let s = Speed {
            num: num / g,
            den: den / g,
        };
        let v = s.as_f64();
        (MIN_SPEED - 1e-12..=MAX_SPEED + 1e-12)
            .contains(&v)
            .then_some(s)
    }

    /// The nearest simple fraction to `value` (for example 0.5 → 1/2, 1.25 → 5/4, 0.333 → 333/1000).
    /// Values within 1e-6 of a fraction with denominator up to 1000 snap to it.
    pub fn from_f64(value: f64) -> Option<Speed> {
        if !value.is_finite() || value <= 0.0 {
            return None;
        }
        let mut best: Option<(f64, u32, u32)> = None;
        for den in 1..=1000u32 {
            let num = (value * f64::from(den)).round();
            if num < 1.0 || num > f64::from(u32::MAX) {
                continue;
            }
            let err = (num / f64::from(den) - value).abs();
            if best.is_none_or(|(e, _, _)| err < e - 1e-12) {
                best = Some((err, num as u32, den));
            }
            if err < 1e-9 {
                break;
            }
        }
        let (_, n, d) = best?;
        Speed::new(n, d)
    }

    /// Parse `"2"`, `"0.5"`, `"250%"`, `"3/2"`.
    pub fn parse(text: &str) -> Option<Speed> {
        let t = text.trim();
        if let Some(p) = t.strip_suffix('%') {
            return Speed::from_f64(p.trim().parse::<f64>().ok()? / 100.0);
        }
        if let Some((a, b)) = t.split_once('/') {
            return Speed::new(a.trim().parse().ok()?, b.trim().parse().ok()?);
        }
        Speed::from_f64(t.trim_end_matches('x').parse::<f64>().ok()?)
    }

    pub fn num(self) -> u32 {
        self.num
    }

    pub fn den(self) -> u32 {
        self.den
    }

    pub fn as_f64(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }

    pub fn is_normal(self) -> bool {
        self == Speed::NORMAL
    }

    /// How much media a stretch of timeline plays: `t × speed`, rounded to a tick.
    pub fn media_len(self, t: Time) -> Time {
        let n = i128::from(t.ticks()) * i128::from(self.num);
        Time::from_ticks(div_round(n, i128::from(self.den)) as i64)
    }

    /// How much timeline a stretch of media takes: `m ÷ speed`, rounded down to a tick.
    pub fn timeline_len(self, m: Time) -> Time {
        let n = i128::from(m.ticks()) * i128::from(self.den);
        Time::from_ticks(n.div_euclid(i128::from(self.num)) as i64)
    }
}

impl Default for Speed {
    fn default() -> Self {
        Speed::NORMAL
    }
}

impl std::fmt::Display for Speed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.den == 1 {
            write!(f, "{}x", self.num)
        } else {
            write!(f, "{}x", self.as_f64())
        }
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn div_round(n: i128, d: i128) -> i128 {
    let q = n.div_euclid(d);
    let r = n.rem_euclid(d);
    if r * 2 >= d {
        q + 1
    } else {
        q
    }
}

/// A side of the picture, for wipes and pushes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// The new picture arrives from the left edge and moves right.
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Left => "left",
            Direction::Right => "right",
            Direction::Up => "up",
            Direction::Down => "down",
        }
    }

    pub fn parse(name: &str) -> Option<Direction> {
        Some(match name {
            "left" => Direction::Left,
            "right" => Direction::Right,
            "up" => Direction::Up,
            "down" => Direction::Down,
            _ => return None,
        })
    }
}

/// How a transition blends the outgoing clip into the incoming one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransitionKind {
    /// A cross dissolve. On audio, an equal-power crossfade.
    Dissolve,
    /// Fade to black, then up from black.
    DipToBlack,
    /// Fade to white, then down from white.
    DipToWhite,
    /// The incoming picture is revealed by an edge sweeping across.
    Wipe(Direction),
    /// The incoming picture pushes the outgoing one out of frame.
    Push(Direction),
}

impl TransitionKind {
    /// The name used in project files and on the command line, such as `dissolve` or `wipe-left`.
    pub fn name(self) -> String {
        match self {
            TransitionKind::Dissolve => "dissolve".into(),
            TransitionKind::DipToBlack => "dip-black".into(),
            TransitionKind::DipToWhite => "dip-white".into(),
            TransitionKind::Wipe(d) => format!("wipe-{}", d.as_str()),
            TransitionKind::Push(d) => format!("push-{}", d.as_str()),
        }
    }

    pub fn parse(name: &str) -> Option<TransitionKind> {
        Some(match name {
            "dissolve" => TransitionKind::Dissolve,
            "dip-black" => TransitionKind::DipToBlack,
            "dip-white" => TransitionKind::DipToWhite,
            _ => {
                if let Some(d) = name.strip_prefix("wipe-") {
                    TransitionKind::Wipe(Direction::parse(d)?)
                } else {
                    TransitionKind::Push(Direction::parse(name.strip_prefix("push-")?)?)
                }
            }
        })
    }

    /// Every name [`TransitionKind::parse`] accepts.
    pub fn all_names() -> Vec<String> {
        let mut v = vec![
            "dissolve".to_string(),
            "dip-black".into(),
            "dip-white".into(),
        ];
        for d in ["left", "right", "up", "down"] {
            v.push(format!("wipe-{d}"));
        }
        for d in ["left", "right", "up", "down"] {
            v.push(format!("push-{d}"));
        }
        v
    }

    /// Whether the transition makes sense on an audio track (only the crossfade does).
    pub fn works_on_audio(self) -> bool {
        self == TransitionKind::Dissolve
    }
}

/// Where a transition sits around the cut between two clips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Alignment {
    /// Half before the cut and half after. Both clips need spare media ("handles").
    Center,
    /// Starts at the cut and runs into the incoming clip. The outgoing clip needs a handle.
    StartAtCut,
    /// Finishes at the cut. The incoming clip needs a handle before its first frame.
    EndAtCut,
}

impl Alignment {
    pub fn as_str(self) -> &'static str {
        match self {
            Alignment::Center => "center",
            Alignment::StartAtCut => "start",
            Alignment::EndAtCut => "end",
        }
    }

    pub fn parse(name: &str) -> Option<Alignment> {
        Some(match name {
            "center" | "centre" => Alignment::Center,
            "start" => Alignment::StartAtCut,
            "end" => Alignment::EndAtCut,
            _ => return None,
        })
    }
}

/// A transition from the clip before into the clip that carries it.
///
/// The two clips must touch. During the transition both are on screen (or both are heard):
/// the outgoing clip keeps playing past its end and the incoming one plays from before its
/// start, using media beyond what the clips show. Without enough spare media the transition is
/// refused (or, after some other edit removes the spare media, dropped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub kind: TransitionKind,
    /// Total length, a whole number of frames.
    pub duration: Time,
    pub alignment: Alignment,
}

impl Transition {
    /// How much of the transition lies before the cut, given the length of a frame.
    pub fn before_cut(&self, frame: Time) -> Time {
        match self.alignment {
            Alignment::Center => {
                let frames = (self.duration.ticks() / frame.ticks().max(1)) / 2;
                Time::from_ticks(frames * frame.ticks())
            }
            Alignment::StartAtCut => Time::ZERO,
            Alignment::EndAtCut => self.duration,
        }
    }

    /// How much of the transition lies after the cut.
    pub fn after_cut(&self, frame: Time) -> Time {
        self.duration - self.before_cut(frame)
    }

    /// The span of timeline the transition covers when the cut is at `cut`.
    pub fn window(&self, cut: Time, frame: Time) -> (Time, Time) {
        (cut - self.before_cut(frame), cut + self.after_cut(frame))
    }
}

/// How a speed change treats the clip's length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeedMode {
    /// Keep the same stretch of media, so the clip gets shorter when sped up and longer when
    /// slowed down. This is "rate stretch".
    KeepMedia,
    /// Keep the clip's length on the timeline, so it shows more or less of the media.
    KeepLength,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_are_exact_fractions() {
        assert_eq!(Speed::new(4, 2), Speed::new(2, 1));
        assert_eq!(Speed::from_f64(0.5), Speed::new(1, 2));
        assert_eq!(Speed::from_f64(1.25), Speed::new(5, 4));
        assert_eq!(Speed::from_f64(2.0).unwrap().as_f64(), 2.0);
        assert!(Speed::new(0, 1).is_none());
        assert!(Speed::new(1, 1000).is_none());
        assert!(Speed::from_f64(1000.0).is_none());
        assert!(Speed::from_f64(f64::NAN).is_none());
    }

    #[test]
    fn speeds_parse() {
        assert_eq!(Speed::parse("200%"), Speed::new(2, 1));
        assert_eq!(Speed::parse("50%"), Speed::new(1, 2));
        assert_eq!(Speed::parse("3/2"), Speed::new(3, 2));
        assert_eq!(Speed::parse("0.25"), Speed::new(1, 4));
        assert_eq!(Speed::parse("2x"), Speed::new(2, 1));
        assert!(Speed::parse("fast").is_none());
    }

    #[test]
    fn lengths_scale_both_ways() {
        let s = Speed::new(3, 2).unwrap();
        let one = Time::from_secs_f64(1.0);
        assert_eq!(s.media_len(one), Time::from_secs_f64(1.5));
        assert_eq!(
            s.timeline_len(Time::from_secs_f64(3.0)),
            Time::from_secs_f64(2.0)
        );
        assert_eq!(Speed::NORMAL.media_len(one), one);
    }

    #[test]
    fn transition_names_round_trip() {
        for name in TransitionKind::all_names() {
            assert_eq!(TransitionKind::parse(&name).unwrap().name(), name);
        }
        assert!(TransitionKind::parse("sparkle").is_none());
    }

    #[test]
    fn transition_windows() {
        let frame = Time::from_secs_f64(1.0 / 24.0);
        let d = Time::from_ticks(frame.ticks() * 10);
        let cut = Time::from_secs_f64(5.0);
        let t = |a| Transition {
            kind: TransitionKind::Dissolve,
            duration: d,
            alignment: a,
        };
        let (a, b) = t(Alignment::Center).window(cut, frame);
        assert_eq!(
            (cut - a, b - cut),
            (
                Time::from_ticks(frame.ticks() * 5),
                Time::from_ticks(frame.ticks() * 5)
            )
        );
        assert_eq!(t(Alignment::StartAtCut).window(cut, frame), (cut, cut + d));
        assert_eq!(t(Alignment::EndAtCut).window(cut, frame), (cut - d, cut));
    }
}
