// SPDX-License-Identifier: GPL-3.0-or-later
//! Drawn graphics (colours, gradients, shapes and text) as project media.
//!
//! The drawing is in the Motion Graphics Engine. A graphic is an *asset* like a file is: it has a
//! name, can be placed on any picture track any number of times, trimmed, faded, moved, masked and
//! keyframed like any other picture, and changing it changes every clip that uses it.

use crate::error::{Error, Result};
use crate::ids::AssetId;
use crate::model::{Asset, VideoProps};

pub use motion_graphics_engine::{
    Align, Colour, Fill, Generator, Shadow, Shape, ShapeKind, Stroke, Text,
};

fn bad<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::BadGraphic(msg.into()))
}

fn range(name: &str, v: f64, lo: f64, hi: f64) -> Result<()> {
    if v.is_finite() && v >= lo && v <= hi {
        Ok(())
    } else {
        bad(format!("{name} must be between {lo} and {hi}"))
    }
}

fn check_fill(f: &Fill) -> Result<()> {
    if let Fill::Linear { angle_deg, .. } = f {
        range("gradient angle", *angle_deg, -3600.0, 3600.0)?;
    }
    Ok(())
}

fn check_stroke(what: &str, s: &Stroke) -> Result<()> {
    range(&format!("{what} width"), s.width_pct, 0.0, 50.0)
}

/// The rules for a graphic's numbers (sizes are percent of the frame).
pub fn validate(g: &Generator) -> Result<()> {
    match g {
        Generator::Fill(f) => check_fill(f),
        Generator::Shape(s) => {
            range("width", s.width_pct, 0.0, 1000.0)?;
            range("height", s.height_pct, 0.0, 1000.0)?;
            check_fill(&s.fill)?;
            if let Some(k) = &s.stroke {
                check_stroke("stroke", k)?;
            }
            match s.kind {
                ShapeKind::Rect { corner_pct } => range("corner", corner_pct, 0.0, 100.0),
                ShapeKind::Ellipse => Ok(()),
                ShapeKind::Polygon { sides } => {
                    if (3..=32).contains(&sides) {
                        Ok(())
                    } else {
                        bad("a polygon has 3 to 32 sides")
                    }
                }
                ShapeKind::Star { points, inner_pct } => {
                    if !(3..=32).contains(&points) {
                        return bad("a star has 3 to 32 points");
                    }
                    range("inner radius", inner_pct, 5.0, 100.0)
                }
            }
        }
        Generator::Text(t) => {
            if t.text.chars().count() > 2000 {
                return bad("text can be at most 2000 characters");
            }
            if t.font.as_ref().is_some_and(|f| f.len() > 1024) {
                return bad("font path is too long");
            }
            range("text size", t.size_pct, 0.5, 200.0)?;
            range("line gap", t.line_gap_pct, -50.0, 500.0)?;
            range("tracking", t.tracking_pct, -50.0, 200.0)?;
            if let Some(o) = &t.outline {
                check_stroke("outline", o)?;
            }
            if let Some(s) = &t.shadow {
                range("shadow x", s.dx_pct, -50.0, 50.0)?;
                range("shadow y", s.dy_pct, -50.0, 50.0)?;
            }
            Ok(())
        }
    }
}

/// The asset record of a graphic: a still (no length of its own) with no file.
pub(crate) fn asset(id: AssetId, name: String, g: Generator) -> Asset {
    Asset {
        id,
        name,
        path: std::path::PathBuf::new(),
        duration: None,
        video: vec![VideoProps {
            width: 0,
            height: 0,
            frame_rate: None,
        }],
        audio: Vec::new(),
        generator: Some(g),
    }
}
