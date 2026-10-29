// SPDX-License-Identifier: GPL-3.0-or-later
//! Sound settings of clips, tracks and sequences: gain, pan and fades.
//!
//! Values are stored as whole numbers (hundredths of a decibel, percent of pan) so they compare
//! exactly, save exactly and never drift. The sound maths itself is in the Audio Engine.

use std::fmt;

pub use audio_engine::FadeCurve;

use crate::time::Time;

/// A level change in hundredths of a decibel, from -120.00 dB (silence) to +24.00 dB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Gain(i32);

impl Gain {
    /// No change.
    pub const UNITY: Gain = Gain(0);
    /// Silence.
    pub const SILENT: Gain = Gain(-12_000);
    pub const MAX_DB: f64 = 24.0;

    /// A gain of `db` decibels, rounded to 0.01 dB. `None` if it is not a number or above +24 dB;
    /// anything below -120 dB is silence.
    pub fn from_db(db: f64) -> Option<Gain> {
        if !db.is_finite() || db > Gain::MAX_DB {
            return None;
        }
        Some(Gain(((db * 100.0).round() as i32).max(-12_000)))
    }

    pub(crate) const fn from_centibels(v: i32) -> Gain {
        Gain(v)
    }

    /// The value in hundredths of a decibel (what project files store).
    pub const fn centibels(self) -> i32 {
        self.0
    }

    pub fn db(self) -> f32 {
        self.0 as f32 / 100.0
    }

    /// The linear multiplier. Exactly 1.0 for [`Gain::UNITY`] and exactly 0.0 for silence.
    pub fn linear(self) -> f32 {
        audio_engine::db_to_gain(self.db())
    }

    pub fn is_unity(self) -> bool {
        self.0 == 0
    }
}

impl Default for Gain {
    fn default() -> Self {
        Gain::UNITY
    }
}

impl fmt::Display for Gain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Gain::SILENT {
            write!(f, "-inf dB")
        } else {
            write!(f, "{:+.2} dB", self.db())
        }
    }
}

/// A pan position in percent, from -100 (hard left) to 100 (hard right).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pan(i16);

impl Pan {
    pub const CENTRE: Pan = Pan(0);

    /// Pan from -100 to 100 percent. `None` outside that range.
    pub fn new(percent: i16) -> Option<Pan> {
        (-100..=100).contains(&percent).then_some(Pan(percent))
    }

    pub const fn percent(self) -> i16 {
        self.0
    }

    /// -1.0 to 1.0.
    pub fn value(self) -> f32 {
        f32::from(self.0) / 100.0
    }

    pub fn is_centre(self) -> bool {
        self.0 == 0
    }
}

impl Default for Pan {
    fn default() -> Self {
        Pan::CENTRE
    }
}

impl fmt::Display for Pan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            0 => write!(f, "centre"),
            p if p < 0 => write!(f, "{}% left", -p),
            p => write!(f, "{p}% right"),
        }
    }
}

/// A fade at the start or end of a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fade {
    pub duration: Time,
    pub curve: FadeCurve,
}

/// Changes to a clip's sound settings. Fields left as `None` stay as they are; for the fades,
/// `Some(None)` removes the fade.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClipAudio {
    pub gain: Option<Gain>,
    pub pan: Option<Pan>,
    pub fade_in: Option<Option<Fade>>,
    pub fade_out: Option<Option<Fade>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_is_exact_hundredths_of_a_db() {
        assert_eq!(Gain::from_db(-6.0).unwrap().centibels(), -600);
        assert_eq!(Gain::from_db(0.004).unwrap(), Gain::UNITY);
        assert_eq!(Gain::from_db(-300.0).unwrap(), Gain::SILENT);
        assert!(Gain::from_db(30.0).is_none());
        assert!(Gain::from_db(f64::NAN).is_none());
        assert_eq!(Gain::UNITY.linear(), 1.0);
        assert_eq!(Gain::SILENT.linear(), 0.0);
        assert!((Gain::from_db(-6.02).unwrap().linear() - 0.5).abs() < 1e-3);
        assert_eq!(Gain::from_db(-6.0).unwrap().to_string(), "-6.00 dB");
    }

    #[test]
    fn pan_range_is_checked() {
        assert!(Pan::new(101).is_none());
        assert_eq!(Pan::new(-100).unwrap().value(), -1.0);
        assert_eq!(Pan::new(25).unwrap().to_string(), "25% right");
        assert_eq!(Pan::CENTRE.to_string(), "centre");
    }
}
