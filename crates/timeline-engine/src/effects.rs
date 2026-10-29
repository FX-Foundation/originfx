// SPDX-License-Identifier: GPL-3.0-or-later
//! Effects on clips: a stack of picture effects, each with settings that can be keyframed.
//!
//! The effects themselves (the pixel maths) are in the Effects Engine. This module holds what the
//! project remembers: which effects a clip has, in which order, their settings and their curves.
//! Settings are plain numbers in the order of `Kind::params()`. Times in an effect's curves are
//! measured from the start of the clip, like the clip's own animation.

use std::fmt;

pub use effects_engine::Kind as EffectKind;
pub use effects_engine::ParamSpec;

use crate::animation::Curve;

/// The most effects one clip can have.
pub const MAX_EFFECTS: usize = 32;

/// Names an effect on one clip. Unique within the clip only (a cut clip's halves share ids).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EffectId(pub u32);

impl fmt::Display for EffectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fx{}", self.0)
    }
}

/// One effect on a clip.
#[derive(Debug, Clone, PartialEq)]
pub struct Effect {
    pub id: EffectId,
    pub kind: EffectKind,
    /// A disabled effect stays in the stack but does nothing.
    pub enabled: bool,
    pub(crate) values: Vec<f64>,
    pub(crate) curves: Vec<Option<Curve>>,
}

// Values are always finite (edits and loading refuse anything else), so equality is total.
impl Eq for Effect {}

impl Effect {
    pub(crate) fn new(id: EffectId, kind: EffectKind) -> Effect {
        let n = kind.params().len();
        Effect {
            id,
            kind,
            enabled: true,
            values: kind.defaults(),
            curves: vec![None; n],
        }
    }

    /// Rebuild from stored parts (used when loading files).
    pub(crate) fn from_parts(
        id: EffectId,
        kind: EffectKind,
        enabled: bool,
        values: Vec<f64>,
        curves: Vec<Option<Curve>>,
    ) -> Effect {
        Effect {
            id,
            kind,
            enabled,
            values,
            curves,
        }
    }

    /// The fixed value of setting `index` (used when it has no curve).
    pub fn value(&self, index: usize) -> f64 {
        self.values[index]
    }

    /// The curve of setting `index`, if it is animated.
    pub fn curve(&self, index: usize) -> Option<&Curve> {
        self.curves.get(index).and_then(Option::as_ref)
    }

    pub fn is_animated(&self, index: usize) -> bool {
        self.curve(index).is_some()
    }

    /// Every setting's value `off` ticks into the clip.
    pub fn values_at(&self, off: i64) -> Vec<f64> {
        (0..self.values.len())
            .map(|i| {
                self.curve(i)
                    .and_then(|c| c.eval(off))
                    .unwrap_or(self.values[i])
            })
            .collect()
    }

    /// True when the effect can change the picture at some time: enabled, and not neutral at every
    /// moment (a curve is assumed to leave neutral at some point).
    pub fn is_live(&self) -> bool {
        self.enabled
            && (self.curves.iter().any(Option::is_some)
                || !effects_engine::is_neutral(self.kind, &self.values))
    }

    /// The effect cut to clip times `a..b` (ticks) and moved so `a` is zero.
    pub(crate) fn slice(&self, a: i64, b: i64) -> Effect {
        let mut e = self.clone();
        e.curves = self
            .curves
            .iter()
            .map(|c| c.as_ref().map(|c| c.slice(a, b)))
            .collect();
        e
    }

    /// The first problem with the effect, if any, for a clip lasting `max` ticks.
    pub(crate) fn problem(&self, max: i64) -> Option<String> {
        let specs = self.kind.params();
        if self.values.len() != specs.len() || self.curves.len() != specs.len() {
            return Some("has the wrong number of settings".into());
        }
        for (i, s) in specs.iter().enumerate() {
            let ok = |v: f64| v.is_finite() && v >= s.min && v <= s.max;
            if !ok(self.values[i]) {
                return Some(format!("{} is outside {} to {}", s.name, s.min, s.max));
            }
            if let Some(c) = self.curve(i) {
                if c.keys().iter().any(|k| !ok(k.value)) {
                    return Some(format!(
                        "a {} keyframe is outside {} to {}",
                        s.name, s.min, s.max
                    ));
                }
                if c.keys().iter().any(|k| k.t < 0 || k.t > max) {
                    return Some(format!("a {} keyframe lies outside the clip", s.name));
                }
            }
        }
        None
    }
}
