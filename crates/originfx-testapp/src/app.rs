// SPDX-License-Identifier: GPL-3.0-or-later
//! The window: top bar, video view, transport and a side panel.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use plugin_engine::Host;
use timeline_engine::{Editor, PlaceMode, Project, SequenceId, SequenceSettings, Time};

use crate::audio::Audio;
use crate::inspector;
use crate::player::Worker;
use crate::selfcheck;

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Inspector,
    SelfCheck,
    Info,
}

pub struct App {
    editor: Editor,
    seq: Option<SequenceId>,
    worker: Option<Worker>,
    audio: Audio,
    gpu: bool,
    /// Preview at 1/this of the sequence size (1 = full).
    div: u32,
    texture: Option<TextureHandle>,
    plugins: Arc<Host>,
    path_text: String,
    status: String,
    // playback
    playing: bool,
    play_start: Instant,
    play_from: f64,
    pos_secs: f64,
    shown_index: i64,
    asked_index: i64,
    last_ms: f64,
    shown_count: u32,
    fps_since: Instant,
    fps: f64,
    // side panel
    tab: Tab,
    report: String,
    check_rx: Option<Receiver<String>>,
    sample: Option<PathBuf>,
    project_path: Option<PathBuf>,
    note: String,
}

impl App {
    pub fn new() -> App {
        let mut host = Host::new();
        if let Ok(p) = plugin_engine::Plugin::load(&plugin_engine::example_invert_mix_wasm()) {
            host.add("invert-mix.wasm", p);
        }
        App {
            editor: Editor::new("untitled"),
            seq: None,
            worker: None,
            audio: Audio::new(),
            gpu: false,
            div: 1,
            texture: None,
            plugins: Arc::new(host),
            path_text: String::new(),
            status: "Open a video or a project to begin.".into(),
            playing: false,
            play_start: Instant::now(),
            play_from: 0.0,
            pos_secs: 0.0,
            shown_index: -1,
            asked_index: -1,
            last_ms: 0.0,
            shown_count: 0,
            fps_since: Instant::now(),
            fps: 0.0,
            tab: Tab::Inspector,
            report: String::new(),
            check_rx: None,
            sample: None,
            project_path: None,
            note: String::new(),
        }
    }

    /// Open a project or a media file, going by the file name.
    pub fn open_path(&mut self, p: PathBuf) {
        if p.extension().is_some_and(|e| e == "originfx") {
            self.open_project(p);
        } else {
            self.open_media(p);
        }
    }

    fn open_media(&mut self, path: PathBuf) {
        let result = (|| -> Result<(Editor, SequenceId), String> {
            let asset = timeline_engine::media::probe_asset(&path).map_err(|e| e.to_string())?;
            let (w, h) = asset
                .video
                .first()
                .map_or((1280, 720), |v| (v.width, v.height));
            // Keep the picture's shape; shrink only if it is bigger than 1080p.
            let k = (1920.0 / f64::from(w)).min(1920.0 / f64::from(h)).min(1.0);
            let even = |n: f64| ((n * k / 2.0).round() as u32 * 2).max(2);
            let (w, h) = (even(f64::from(w)), even(f64::from(h)));
            let mut ed = Editor::new("test");
            let mut settings = SequenceSettings {
                width: w,
                height: h,
                ..Default::default()
            };
            // Play at the clip's own frame rate, not the default 24.
            if let Some(fr) = asset.video.first().and_then(|p| p.frame_rate) {
                settings.frame_rate = fr;
            }
            let seq = ed.add_sequence(settings).map_err(|e| e.to_string())?;
            let has_audio = !asset.audio.is_empty();
            let id = ed.add_asset(asset).map_err(|e| e.to_string())?;
            let s = ed.project().sequence(seq).ok_or("no sequence")?;
            let v1 = s.video_tracks()[0].id;
            let a1 = s.audio_tracks().front().map(|t| t.id).filter(|_| has_audio);
            ed.place_asset(
                seq,
                id,
                Time::ZERO,
                Time::ZERO,
                None,
                Some(v1),
                a1,
                PlaceMode::Overwrite,
            )
            .map_err(|e| e.to_string())?;
            Ok((ed, seq))
        })();
        match result {
            Ok((ed, seq)) => {
                self.sample = Some(path.clone());
                self.load(ed, seq, format!("Opened {}", path.display()));
            }
            Err(e) => self.status = format!("Could not open: {e}"),
        }
    }

    fn open_project(&mut self, path: PathBuf) {
        match Project::load(&path) {
            Ok(p) => {
                let ed = Editor::from_project(p);
                let seq = ed.project().sequences().values().next().map(|s| s.id);
                match seq {
                    Some(s) => self.load(ed, s, format!("Opened project {}", path.display())),
                    None => self.status = "That project has no sequence.".into(),
                }
            }
            Err(e) => self.status = format!("Could not open: {e}"),
        }
    }

    fn load(&mut self, ed: Editor, seq: SequenceId, status: String) {
        self.editor = ed;
        self.seq = Some(seq);
        self.status = status;
        self.pos_secs = 0.0;
        self.playing = false;
        self.refresh_worker();
    }

    fn refresh_worker(&mut self) {
        if let Some(seq) = self.seq {
            self.worker = Some(Worker::new(
                scaled(self.editor.project(), self.div),
                seq,
                Arc::clone(&self.plugins),
                self.gpu,
            ));
            self.shown_index = -1;
            self.asked_index = -1;
            self.audio.load(
                self.editor.project().clone(),
                seq,
                Arc::clone(&self.plugins),
            );
            if self.playing {
                self.audio.play(self.pos_secs);
            }
        }
    }

    fn duration_secs(&self) -> f64 {
        self.seq
            .and_then(|s| self.editor.project().sequence(s))
            .map_or(0.0, |s| s.duration().as_secs_f64())
    }

    fn frame_rate(&self) -> f64 {
        self.seq
            .and_then(|s| self.editor.project().sequence(s))
            .map_or(24.0, |s| s.frame_rate.as_f64())
    }
}

/// The project with every sequence drawn at 1/`div` of its size. Everything else in the project
/// is measured in percent of the frame, so it looks the same, only smaller and faster.
fn scaled(p: &Project, div: u32) -> Project {
    if div <= 1 {
        return p.clone();
    }
    fn walk(v: &mut serde_json::Value, div: u64) {
        match v {
            serde_json::Value::Object(m) => {
                if m.contains_key("sample_rate") {
                    for k in ["width", "height"] {
                        if let Some(n) = m.get(k).and_then(serde_json::Value::as_u64) {
                            m.insert(k.into(), ((n / div / 2) * 2).max(2).into());
                        }
                    }
                }
                m.values_mut().for_each(|x| walk(x, div));
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(|x| walk(x, div)),
            _ => {}
        }
    }
    let Ok(text) = p.to_json() else {
        return p.clone();
    };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return p.clone();
    };
    walk(&mut v, u64::from(div));
    Project::from_json(&v.to_string()).unwrap_or_else(|_| p.clone())
}

fn timecode(s: f64, fps: f64) -> String {
    let total = (s * fps).round() as i64;
    let f = fps.round().max(1.0) as i64;
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        total / f / 3600,
        total / f / 60 % 60,
        total / f % 60,
        total % f
    )
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        let dur = self.duration_secs();
        let fps = self.frame_rate();

        // Playhead follows the wall clock while playing.
        if self.playing {
            self.pos_secs = self.play_from + self.play_start.elapsed().as_secs_f64();
            if self.pos_secs >= dur {
                self.pos_secs = dur;
                self.playing = false;
                self.audio.stop();
            }
            ctx.request_repaint();
        }
        let want = (self.pos_secs * fps).floor() as i64;
        let last = ((dur * fps).ceil() as i64 - 1).max(0);
        let want = want.clamp(0, last);
        if let Some(w) = &self.worker {
            if want != self.asked_index {
                w.want(want);
                self.asked_index = want;
            }
            while let Ok(r) = w.rx.try_recv() {
                match r.frame {
                    Ok(f) => {
                        let img = ColorImage::from_rgba_unmultiplied(
                            [f.width as usize, f.height as usize],
                            &f.data,
                        );
                        match &mut self.texture {
                            Some(t) => t.set(img, TextureOptions::LINEAR),
                            None => {
                                self.texture =
                                    Some(ctx.load_texture("frame", img, TextureOptions::LINEAR))
                            }
                        }
                        self.shown_index = r.index;
                        self.last_ms = r.millis;
                        self.shown_count += 1;
                    }
                    Err(e) => self.status = format!("Render error: {e}"),
                }
                ctx.request_repaint();
            }
        }
        if self.fps_since.elapsed().as_secs_f64() >= 1.0 {
            self.fps = f64::from(self.shown_count) / self.fps_since.elapsed().as_secs_f64();
            self.shown_count = 0;
            self.fps_since = Instant::now();
        }

        // Self-check results stream in.
        if let Some(rx) = &self.check_rx {
            let mut done = false;
            loop {
                match rx.try_recv() {
                    Ok(line) => {
                        self.report.push_str(&line);
                        self.report.push('\n');
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(_) => {
                        done = true;
                        break;
                    }
                }
            }
            if done {
                self.check_rx = None;
            }
            ctx.request_repaint();
        }

        egui::Panel::top("top").show(root, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open media…").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_file() {
                        self.open_media(p);
                    }
                }
                if ui.button("Open project…").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("OriginFX project", &["originfx"])
                        .pick_file()
                    {
                        self.open_project(p);
                    }
                }
                ui.separator();
                if ui
                    .add_enabled(self.editor.can_undo(), egui::Button::new("Undo"))
                    .clicked()
                {
                    let _ = self.editor.undo();
                    self.refresh_worker();
                }
                if ui
                    .add_enabled(self.editor.can_redo(), egui::Button::new("Redo"))
                    .clicked()
                {
                    let _ = self.editor.redo();
                    self.refresh_worker();
                }
                if ui
                    .add_enabled(self.seq.is_some(), egui::Button::new("Save project…"))
                    .clicked()
                {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("OriginFX project", &["originfx"])
                        .set_file_name("test.originfx")
                        .save_file()
                    {
                        self.status = match self.editor.project().save(&p) {
                            Ok(()) => format!("Saved {}", p.display()),
                            Err(e) => format!("Could not save: {e}"),
                        };
                        self.project_path = Some(p);
                    }
                }
                ui.separator();
                if ui
                    .checkbox(&mut self.gpu, "GPU decode")
                    .on_hover_text("Decode video on the graphics card when it can")
                    .changed()
                {
                    self.refresh_worker();
                }
                let before = self.div;
                egui::ComboBox::from_id_salt("pq")
                    .selected_text(match self.div {
                        1 => "Preview: full",
                        2 => "Preview: half",
                        _ => "Preview: quarter",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.div, 1, "full size (sharp, slower)");
                        ui.selectable_value(&mut self.div, 2, "half size");
                        ui.selectable_value(&mut self.div, 4, "quarter size (fastest)");
                    });
                if self.div != before {
                    self.refresh_worker();
                }
                ui.separator();
                ui.label("or path:");
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.path_text)
                        .desired_width(300.0)
                        .hint_text("/path/to/video-or-project"),
                );
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.open_path(PathBuf::from(self.path_text.trim()));
                }
            });
            ui.label(&self.status);
        });

        egui::Panel::bottom("transport").show(root, |ui| {
            ui.horizontal(|ui| {
                let label = if self.playing { "Pause" } else { "Play" };
                if ui
                    .add_enabled(self.seq.is_some(), egui::Button::new(label))
                    .clicked()
                {
                    if self.playing {
                        self.playing = false;
                        self.audio.stop();
                    } else {
                        if self.pos_secs >= dur {
                            self.pos_secs = 0.0;
                        }
                        self.play_from = self.pos_secs;
                        self.play_start = Instant::now();
                        self.playing = true;
                        self.audio.play(self.pos_secs);
                    }
                }
                let mut p = self.pos_secs;
                ui.spacing_mut().slider_width = (ui.available_width() - 460.0).max(100.0);
                if ui
                    .add(egui::Slider::new(&mut p, 0.0..=dur.max(0.001)).show_value(false))
                    .changed()
                {
                    self.pos_secs = p;
                    self.play_from = p;
                    self.play_start = Instant::now();
                    if self.playing {
                        self.audio.play(p);
                    }
                }
                ui.monospace(format!(
                    "{} / {}",
                    timecode(self.pos_secs, fps),
                    timecode(dur, fps)
                ));
                let behind = (want - self.shown_index).max(0);
                ui.label(format!(
                    "{:.0} fps shown | {:.0} ms/frame | {} behind",
                    self.fps, self.last_ms, behind
                ));
            });
        });

        egui::Panel::right("side").min_size(380.0).show(root, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Inspector, "Inspector");
                ui.selectable_value(&mut self.tab, Tab::SelfCheck, "Self-check");
                ui.selectable_value(&mut self.tab, Tab::Info, "Hardware / info");
            });
            ui.separator();
            match self.tab {
                Tab::Inspector => {
                    egui::ScrollArea::vertical().show(ui, |ui| match self.seq {
                        Some(seq) => {
                            let t = Time::from_secs_f64(self.pos_secs.min((dur - 0.001).max(0.0)));
                            let out = inspector::show(
                                ui,
                                &mut self.editor,
                                seq,
                                t,
                                &self.plugins,
                                &mut self.note,
                            );
                            if let Some(m) = out.message {
                                self.status = format!("Refused: {m}");
                            }
                            if out.changed {
                                self.refresh_worker();
                            }
                            if !self.note.is_empty() {
                                ui.label(&self.note);
                            }
                        }
                        None => {
                            ui.label("Open a video or a project first.");
                        }
                    });
                }
                Tab::SelfCheck => {
                    ui.horizontal(|ui| {
                        let running = self.check_rx.is_some();
                        if ui
                            .add_enabled(!running, egui::Button::new("Run self-check"))
                            .clicked()
                        {
                            self.report.clear();
                            let (tx, rx) = channel();
                            let sample = self.sample.clone();
                            std::thread::spawn(move || {
                                selfcheck::run(sample, |l| {
                                    let _ = tx.send(l.to_string());
                                });
                            });
                            self.check_rx = Some(rx);
                        }
                        if ui.button("Copy report").clicked() {
                            ui.ctx().copy_text(self.report.clone());
                        }
                        if self.check_rx.is_some() {
                            ui.spinner();
                        }
                    });
                    ui.label("Tests every engine here. Open a video first to include it.");
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.report.as_str())
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY),
                        );
                    });
                }
                Tab::Info => {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.monospace(format!(
                            "OriginFX {}  {} / {}",
                            env!("CARGO_PKG_VERSION"),
                            std::env::consts::OS,
                            std::env::consts::ARCH
                        ));
                        ui.monospace(format!("Sound: {}", self.audio.device));
                        ui.monospace("Hardware backends:");
                        for b in media_engine::hwaccel::probe_backends() {
                            match b.error {
                                None => {
                                    ui.monospace(format!("  [usable] {} ({})", b.name, b.vendor))
                                }
                                Some(e) => {
                                    ui.monospace(format!("  [no] {} ({}) {e}", b.name, b.vendor))
                                }
                            };
                        }
                    });
                }
            }
        });

        egui::CentralPanel::default().show(root, |ui| {
            if let Some(t) = &self.texture {
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new(t).shrink_to_fit());
                });
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("No picture yet");
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smaller_preview_halves_the_sequence_only() {
        let mut ed = Editor::new("t");
        let seq = ed
            .add_sequence(SequenceSettings {
                width: 640,
                height: 360,
                ..Default::default()
            })
            .unwrap();
        let half = scaled(ed.project(), 2);
        let s = half.sequence(seq).unwrap();
        assert_eq!((s.width, s.height), (320, 180));
        let quarter = scaled(ed.project(), 4);
        let s = quarter.sequence(seq).unwrap();
        assert_eq!((s.width, s.height), (160, 90));
        let full = scaled(ed.project(), 1);
        assert_eq!(full.sequence(seq).unwrap().width, 640);
    }
}
