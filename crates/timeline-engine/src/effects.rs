// SPDX-License-Identifier: GPL-3.0-or-later
//! Effects on clips: a stack of picture effects, each with settings that can be keyframed.
//!
//! The effects themselves (the pixel maths) are in the Effects Engine. This module holds what the
//! project remembers: which effects a clip has, in which order, their settings and their curves.
//! Settings are plain numbers in the order of `Kind::params()`. Times in an effect's curves are
//! measured from the start of the clip, like the clip's own animation.

use std::fmt;
use std::sync::Arc;

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

/// One setting of an effect, by value (built-in effects and plugins alike).
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub name: String,
    pub unit: String,
    pub default: f64,
    pub min: f64,
    pub max: f64,
}

impl Eq for Setting {}

/// An effect that comes from a plugin: who it is and what settings it has. The project keeps
/// this copy so it can be edited, checked and saved without the plugin installed; the plugin
/// itself is only needed to render.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginSpec {
    /// The plugin's unique id, like `com.example.glow`.
    pub id: String,
    pub name: String,
    pub version: String,
    pub settings: Vec<Setting>,
}

impl Eq for PluginSpec {}

/// The most settings a plugin effect can have.
pub const MAX_PLUGIN_SETTINGS: usize = 16;

impl PluginSpec {
    /// The rules for a plugin's description (also applied to files).
    pub fn problem(&self) -> Option<String> {
        let word = |s: &str| {
            !s.is_empty()
                && s.len() <= 64
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        };
        if !word(&self.id) {
            return Some("a plugin id is 1 to 64 letters, digits, '.', '-' or '_'".into());
        }
        if self.name.is_empty() || self.name.len() > 100 || self.version.len() > 64 {
            return Some("a plugin needs a name of 1 to 100 characters".into());
        }
        if self.settings.len() > MAX_PLUGIN_SETTINGS {
            return Some(format!(
                "a plugin has at most {MAX_PLUGIN_SETTINGS} settings"
            ));
        }
        for (i, s) in self.settings.iter().enumerate() {
            if !word(&s.name) || self.settings[..i].iter().any(|o| o.name == s.name) {
                return Some(format!("setting {i} needs a unique simple name"));
            }
            let ok = s.default.is_finite() && s.min.is_finite() && s.max.is_finite();
            if !ok || s.min > s.max || s.default < s.min || s.default > s.max || s.unit.len() > 16 {
                return Some(format!("setting '{}' has a bad range", s.name));
            }
        }
        None
    }
}

/// One effect on a clip.
#[derive(Debug, Clone, PartialEq)]
pub struct Effect {
    pub id: EffectId,
    /// For a plugin effect this is a placeholder and is never used: look at `plugin` first.
    pub kind: EffectKind,
    /// Set when the effect comes from a plugin.
    pub plugin: Option<Arc<PluginSpec>>,
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
            plugin: None,
            enabled: true,
            values: kind.defaults(),
            curves: vec![None; n],
        }
    }

    pub(crate) fn new_plugin(id: EffectId, spec: PluginSpec) -> Effect {
        let n = spec.settings.len();
        Effect {
            id,
            kind: EffectKind::Invert,
            enabled: true,
            values: spec.settings.iter().map(|s| s.default).collect(),
            curves: vec![None; n],
            plugin: Some(Arc::new(spec)),
        }
    }

    /// The effect's name for display: `blur`, or `plugin:com.example.glow`.
    pub fn label(&self) -> String {
        match &self.plugin {
            Some(p) => format!("plugin:{}", p.id),
            None => self.kind.name().to_string(),
        }
    }

    /// The effect's settings, in order.
    pub fn settings(&self) -> Vec<Setting> {
        match &self.plugin {
            Some(p) => p.settings.clone(),
            None => self
                .kind
                .params()
                .iter()
                .map(|s| Setting {
                    name: s.name.to_string(),
                    unit: s.unit.to_string(),
                    default: s.default,
                    min: s.min,
                    max: s.max,
                })
                .collect(),
        }
    }

    /// The position of the setting called `name`.
    pub fn setting_index(&self, name: &str) -> Option<usize> {
        match &self.plugin {
            Some(p) => p.settings.iter().position(|s| s.name == name),
            None => self.kind.param_index(name),
        }
    }

    /// Rebuild from stored parts (used when loading files).
    pub(crate) fn from_parts(
        id: EffectId,
        kind: EffectKind,
        plugin: Option<PluginSpec>,
        enabled: bool,
        values: Vec<f64>,
        curves: Vec<Option<Curve>>,
    ) -> Effect {
        Effect {
            id,
            kind,
            plugin: plugin.map(Arc::new),
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
            && (self.plugin.is_some()
                || self.curves.iter().any(Option::is_some)
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
        if let Some(p) = &self.plugin {
            if let Some(m) = p.problem() {
                return Some(m);
            }
        }
        let specs = self.settings();
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
