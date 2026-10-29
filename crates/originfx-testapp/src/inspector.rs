// SPDX-License-Identifier: GPL-3.0-or-later
//! The inspector: pick the clip under the playhead and change what it looks like.
//! Every change is an ordinary engine edit, so undo, redo and save all work.

use eframe::egui::{self, Ui};
use plugin_engine::Host;
use timeline_engine::{
    BlendMode, ClipId, ClipSpec, Colour, Editor, EffectKind, Fill, Generator, MaskShape, Param,
    PlaceMode, PluginSpec, SequenceId, Setting, Shape, ShapeKind, Text, Time, TrackKind,
};

/// What the user changed this frame (the app then restarts the preview worker).
pub struct Outcome {
    pub changed: bool,
    pub message: Option<String>,
}

/// The top-most picture clip under the playhead.
pub fn clip_at(ed: &Editor, seq: SequenceId, t: Time) -> Option<ClipId> {
    let s = ed.project().sequence(seq)?;
    s.video_tracks().iter().rev().find_map(|tr| {
        tr.clips()
            .find(|c| c.enabled && c.start <= t && t < c.start + c.duration)
            .map(|c| c.id)
    })
}

const PICTURE: [(Param, &str); 9] = [
    (Param::Opacity, "Opacity"),
    (Param::PositionX, "Position X"),
    (Param::PositionY, "Position Y"),
    (Param::ScaleX, "Scale X"),
    (Param::ScaleY, "Scale Y"),
    (Param::Rotation, "Rotation"),
    (Param::CropLeft, "Crop left"),
    (Param::CropRight, "Crop right"),
    (Param::CropTop, "Crop top"),
];
const MASK: [(Param, &str); 5] = [
    (Param::MaskX, "Mask X"),
    (Param::MaskY, "Mask Y"),
    (Param::MaskWidth, "Mask width"),
    (Param::MaskHeight, "Mask height"),
    (Param::MaskFeather, "Mask feather"),
];

fn slider(ui: &mut Ui, label: &str, value: &mut f64, min: f64, max: f64, unit: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(format!("{label:<12}"));
        let r = ui.add(egui::Slider::new(value, min..=max).suffix(unit));
        changed = r.drag_stopped() || (r.changed() && !r.dragged());
    });
    changed
}

pub fn show(
    ui: &mut Ui,
    ed: &mut Editor,
    seq: SequenceId,
    playhead: Time,
    plugins: &Host,
    font_note: &mut String,
) -> Outcome {
    let mut out = Outcome {
        changed: false,
        message: None,
    };
    let apply = |out: &mut Outcome, r: Result<(), String>| match r {
        Ok(()) => out.changed = true,
        Err(e) => out.message = Some(e),
    };

    // Add graphics: each goes on a new top track from the playhead for 4 seconds.
    ui.collapsing("Add a graphic layer", |ui| {
        ui.horizontal_wrapped(|ui| {
            let layers: [(&str, Generator); 5] = [
                (
                    "Red fill",
                    Generator::Fill(Fill::Solid(Colour::rgb(200, 30, 30))),
                ),
                (
                    "Gradient",
                    Generator::Fill(Fill::Linear {
                        from: Colour::rgb(20, 20, 80),
                        to: Colour::rgb(230, 120, 30),
                        angle_deg: 45.0,
                    }),
                ),
                (
                    "Circle",
                    Generator::Shape(Shape {
                        kind: ShapeKind::Ellipse,
                        width_pct: 40.0,
                        height_pct: 40.0,
                        fill: Fill::Solid(Colour::rgb(40, 120, 255)),
                        stroke: None,
                    }),
                ),
                (
                    "Star",
                    Generator::Shape(Shape {
                        kind: ShapeKind::Star {
                            points: 5,
                            inner_pct: 45.0,
                        },
                        width_pct: 40.0,
                        height_pct: 40.0,
                        fill: Fill::Solid(Colour::rgb(255, 210, 40)),
                        stroke: None,
                    }),
                ),
                (
                    "Text",
                    Generator::Text(Text {
                        text: "OriginFX".into(),
                        font: None,
                        size_pct: 18.0,
                        colour: Colour::WHITE,
                        align: timeline_engine::Align::Centre,
                        line_gap_pct: 0.0,
                        tracking_pct: 0.0,
                        outline: None,
                        shadow: None,
                    }),
                ),
            ];
            for (name, g) in layers {
                if ui.button(name).clicked() {
                    let r = (|| -> Result<(), String> {
                        let a = ed.add_generated(name, g).map_err(|e| e.to_string())?;
                        let tr = ed
                            .add_track(seq, TrackKind::Video, None)
                            .map_err(|e| e.to_string())?;
                        let mut spec = ClipSpec::new(a, playhead);
                        spec.duration = Some(Time::from_secs_f64(4.0));
                        ed.place_clip(seq, tr, spec, PlaceMode::Overwrite)
                            .map_err(|e| e.to_string())?;
                        Ok(())
                    })();
                    if matches!(name, "Text") {
                        *font_note =
                            "Text needs a system font; the Self-check says if one was found."
                                .into();
                    }
                    apply(&mut out, r);
                }
            }
        });
    });
    ui.separator();

    let Some(id) = clip_at(ed, seq, playhead) else {
        ui.label("No picture clip under the playhead.");
        return out;
    };
    let Some((_, clip)) = ed.project().sequence(seq).and_then(|s| s.find_clip(id)) else {
        return out;
    };
    let clip = clip.clone();
    ui.strong(format!("Clip: {}", clip.name));

    // Picture settings.
    for (p, label) in PICTURE {
        let (min, max) = p.range();
        let mut v = clip.fixed_value(p);
        if clip.anim.get(p).is_some() {
            ui.label(format!("{label}: animated"));
            continue;
        }
        if slider(ui, label, &mut v, min, max, p.unit()) {
            let r = ed.set_look_value(seq, id, p, v).map_err(|e| e.to_string());
            apply(&mut out, r);
        }
    }

    // Blend mode.
    ui.horizontal(|ui| {
        ui.label("Blend mode  ");
        let mut mode = clip.blend;
        egui::ComboBox::from_id_salt("blend")
            .selected_text(mode.name())
            .show_ui(ui, |ui| {
                for m in BlendMode::ALL {
                    ui.selectable_value(&mut mode, m, m.name());
                }
            });
        if mode != clip.blend {
            let r = ed.set_blend_mode(seq, id, mode).map_err(|e| e.to_string());
            apply(&mut out, r);
        }
    });

    // Mask.
    ui.separator();
    match &clip.mask {
        None => {
            ui.horizontal(|ui| {
                ui.label("Mask:");
                for s in MaskShape::ALL {
                    if ui.button(format!("add {}", s.name())).clicked() {
                        let r = ed.set_mask(seq, id, s, false).map_err(|e| e.to_string());
                        apply(&mut out, r);
                    }
                }
            });
        }
        Some(m) => {
            let (shape, invert) = (m.shape, m.invert);
            ui.horizontal(|ui| {
                let mut inv = invert;
                if ui.checkbox(&mut inv, "Invert mask").changed() {
                    let r = ed.set_mask(seq, id, shape, inv).map_err(|e| e.to_string());
                    apply(&mut out, r);
                }
                if ui.button("Remove mask").clicked() {
                    let r = ed.remove_mask(seq, id).map_err(|e| e.to_string());
                    apply(&mut out, r);
                }
            });
            for (p, label) in MASK {
                if clip.anim.get(p).is_some() {
                    continue;
                }
                let (min, max) = p.range();
                let mut v = clip.fixed_value(p);
                if slider(ui, label, &mut v, min, max, p.unit()) {
                    let r = ed.set_look_value(seq, id, p, v).map_err(|e| e.to_string());
                    apply(&mut out, r);
                }
            }
        }
    }

    // Effects.
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Effects:");
        egui::ComboBox::from_id_salt("addfx")
            .selected_text("add…")
            .show_ui(ui, |ui| {
                for k in EffectKind::ALL {
                    if ui.selectable_label(false, k.name()).clicked() {
                        let r = ed
                            .add_effect(seq, id, k, None)
                            .map(|_| ())
                            .map_err(|e| e.to_string());
                        apply(&mut out, r);
                    }
                }
                for (m, _) in plugins.list() {
                    if ui
                        .selectable_label(false, format!("plugin: {}", m.name))
                        .clicked()
                    {
                        let spec = PluginSpec {
                            id: m.id.clone(),
                            name: m.name.clone(),
                            version: m.version.clone(),
                            settings: m
                                .params
                                .iter()
                                .map(|d| Setting {
                                    name: d.name.clone(),
                                    unit: d.unit.clone(),
                                    default: d.default,
                                    min: d.min,
                                    max: d.max,
                                })
                                .collect(),
                        };
                        let r = ed
                            .add_plugin_effect(seq, id, spec, None)
                            .map(|_| ())
                            .map_err(|e| e.to_string());
                        apply(&mut out, r);
                    }
                }
            });
    });
    for fx in &clip.effects {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                let mut on = fx.enabled;
                if ui.checkbox(&mut on, fx.label()).changed() {
                    let r = ed
                        .set_effect_enabled(seq, id, fx.id, on)
                        .map_err(|e| e.to_string());
                    apply(&mut out, r);
                }
                if ui.small_button("remove").clicked() {
                    let r = ed.remove_effect(seq, id, fx.id).map_err(|e| e.to_string());
                    apply(&mut out, r);
                }
            });
            let settings = fx.settings();
            let now = fx.values_at(0);
            for (i, s) in settings.iter().enumerate() {
                let mut v = now.get(i).copied().unwrap_or(s.default);
                if slider(ui, &s.name, &mut v, s.min, s.max, &format!(" {}", s.unit)) {
                    let r = ed
                        .set_effect_value(seq, id, fx.id, &s.name, v)
                        .map_err(|e| e.to_string());
                    apply(&mut out, r);
                }
            }
        });
    }
    out
}
