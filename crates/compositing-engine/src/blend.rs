// SPDX-License-Identifier: GPL-3.0-or-later
//! Opacity and blend modes.

use std::fmt;

/// How a layer's colour combines with the colour beneath it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum BlendMode {
    /// The layer covers what is beneath (by its opacity).
    #[default]
    Normal,
    /// Darkens: colours multiply. White has no effect.
    Multiply,
    /// Lightens: the opposite of multiply. Black has no effect.
    Screen,
    /// Multiply the dark parts, screen the light parts of what is beneath.
    Overlay,
    /// The darker of the two, channel by channel.
    Darken,
    /// The lighter of the two, channel by channel.
    Lighten,
    /// Overlay with the layers swapped: the layer decides.
    HardLight,
    /// A gentle version of hard light.
    SoftLight,
    /// The distance between the two colours.
    Difference,
    /// Like difference, with less contrast.
    Exclusion,
    /// The colours add (clipped at white). Good for light and glow.
    Add,
}

impl BlendMode {
    pub const ALL: [BlendMode; 11] = [
        BlendMode::Normal,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::HardLight,
        BlendMode::SoftLight,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Add,
    ];

    /// The name used in project files and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            BlendMode::Normal => "normal",
            BlendMode::Multiply => "multiply",
            BlendMode::Screen => "screen",
            BlendMode::Overlay => "overlay",
            BlendMode::Darken => "darken",
            BlendMode::Lighten => "lighten",
            BlendMode::HardLight => "hard-light",
            BlendMode::SoftLight => "soft-light",
            BlendMode::Difference => "difference",
            BlendMode::Exclusion => "exclusion",
            BlendMode::Add => "add",
        }
    }

    pub fn parse(name: &str) -> Option<BlendMode> {
        BlendMode::ALL.into_iter().find(|m| m.name() == name)
    }

    /// The blended colour for one channel, `b` beneath and `s` the layer, both 0 to 1.
    fn channel(self, b: f32, s: f32) -> f32 {
        match self {
            BlendMode::Normal => s,
            BlendMode::Multiply => b * s,
            BlendMode::Screen => b + s - b * s,
            BlendMode::Overlay => BlendMode::HardLight.channel(s, b),
            BlendMode::Darken => b.min(s),
            BlendMode::Lighten => b.max(s),
            BlendMode::HardLight => {
                if s <= 0.5 {
                    b * 2.0 * s
                } else {
                    let s2 = 2.0 * s - 1.0;
                    b + s2 - b * s2
                }
            }
            BlendMode::SoftLight => {
                if s <= 0.5 {
                    b - (1.0 - 2.0 * s) * b * (1.0 - b)
                } else {
                    let d = if b <= 0.25 {
                        ((16.0 * b - 12.0) * b + 4.0) * b
                    } else {
                        b.sqrt()
                    };
                    b + (2.0 * s - 1.0) * (d - b)
                }
            }
            BlendMode::Difference => (b - s).abs(),
            BlendMode::Exclusion => b + s - 2.0 * b * s,
            BlendMode::Add => (b + s).min(1.0),
        }
    }
}

impl fmt::Display for BlendMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Lay `layer` over `canvas` (same size, straight alpha) with `opacity` (0 to 1) and a blend mode.
///
/// The standard compositing formula: where the canvas is transparent the layer shows as it is;
/// where both have colour the blend mode decides how they mix, in proportion to how much of the
/// canvas is there.
pub fn composite(canvas: &mut [u8], layer: &[u8], opacity: f64, mode: BlendMode) {
    debug_assert_eq!(canvas.len(), layer.len());
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }
    for (d, s) in canvas.chunks_exact_mut(4).zip(layer.chunks_exact(4)) {
        if s[3] == 0 {
            continue;
        }
        if mode == BlendMode::Normal {
            normal(d, s, opacity);
        } else {
            general(d, s, opacity, mode);
        }
    }
}

/// "Over" in integers, exactly as the Render Engine does it when opacity is 1.
fn normal(d: &mut [u8], s: &[u8], opacity: f64) {
    let sa = if opacity >= 1.0 {
        u32::from(s[3])
    } else {
        (f64::from(s[3]) * opacity).round() as u32
    };
    if sa == 0 {
        return;
    }
    if sa == 255 {
        d[..3].copy_from_slice(&s[..3]);
        d[3] = 255;
        return;
    }
    let da = u32::from(d[3]);
    if da == 255 {
        for c in 0..3 {
            d[c] = ((u32::from(s[c]) * sa + u32::from(d[c]) * (255 - sa) + 127) / 255) as u8;
        }
        return;
    }
    let dst_weight = (da * (255 - sa) + 127) / 255;
    let oa = sa + dst_weight;
    if oa == 0 {
        return;
    }
    for c in 0..3 {
        d[c] = ((u32::from(s[c]) * sa + u32::from(d[c]) * dst_weight + oa / 2) / oa) as u8;
    }
    d[3] = oa as u8;
}

fn general(d: &mut [u8], s: &[u8], opacity: f64, mode: BlendMode) {
    let sa = f32::from(s[3]) / 255.0 * opacity as f32;
    let ba = f32::from(d[3]) / 255.0;
    let ra = sa + ba * (1.0 - sa);
    if ra <= 0.0 {
        return;
    }
    for c in 0..3 {
        let cs = f32::from(s[c]) / 255.0;
        let cb = f32::from(d[c]) / 255.0;
        let mixed = (1.0 - ba) * cs + ba * mode.channel(cb, cs);
        let r = (1.0 - sa / ra) * cb + sa / ra * mixed;
        d[c] = (r * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    }
    d[3] = (ra * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
}
