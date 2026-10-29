// SPDX-License-Identifier: GPL-3.0-or-later
//! What a plugin says about itself.

use serde::Deserialize;

use crate::{Error, Result};

/// The plugin interface version this host speaks.
pub const ABI_VERSION: u32 = 1;

/// The most settings one plugin effect can have.
pub const MAX_PARAMS: usize = 16;

/// One setting of a plugin effect.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ParamDef {
    pub name: String,
    /// What the number means, for display (`%`, `px`, `deg`...).
    #[serde(default)]
    pub unit: String,
    pub default: f64,
    pub min: f64,
    pub max: f64,
}

/// A plugin's identity and settings, read from the plugin itself.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Manifest {
    /// Must be [`ABI_VERSION`].
    pub abi: u32,
    /// A unique name like `com.example.glow` (letters, digits, `.`, `-`, `_`).
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub params: Vec<ParamDef>,
}

fn word_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Manifest> {
        let m: Manifest = serde_json::from_slice(bytes)
            .map_err(|e| Error::Manifest(format!("not valid JSON for a manifest: {e}")))?;
        m.check()?;
        Ok(m)
    }

    fn check(&self) -> Result<()> {
        let bad = |m: String| Err(Error::Manifest(m));
        if self.abi != crate::ABI_VERSION {
            return bad(format!(
                "plugin speaks interface version {}, this OriginFX speaks {}",
                self.abi,
                crate::ABI_VERSION
            ));
        }
        if !word_ok(&self.id) {
            return bad("id must be 1 to 64 letters, digits, '.', '-' or '_'".into());
        }
        if self.name.trim().is_empty() || self.name.len() > 100 {
            return bad("name must be 1 to 100 characters".into());
        }
        if self.params.len() > MAX_PARAMS {
            return bad(format!("at most {MAX_PARAMS} settings"));
        }
        for (i, p) in self.params.iter().enumerate() {
            if !word_ok(&p.name) {
                return bad(format!(
                    "setting {i}: name must be letters, digits, '.', '-' or '_'"
                ));
            }
            if self.params[..i].iter().any(|q| q.name == p.name) {
                return bad(format!("setting '{}' appears twice", p.name));
            }
            let ok = p.default.is_finite() && p.min.is_finite() && p.max.is_finite();
            if !ok || p.min > p.max || p.default < p.min || p.default > p.max {
                return bad(format!(
                    "setting '{}': needs finite min <= default <= max",
                    p.name
                ));
            }
        }
        Ok(())
    }

    /// The default value of every setting, in order.
    pub fn defaults(&self) -> Vec<f64> {
        self.params.iter().map(|p| p.default).collect()
    }
}
