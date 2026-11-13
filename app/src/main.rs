// SPDX-License-Identifier: GPL-3.0-or-later
//! OriginFX: the desktop app.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

slint::include_modules!();

mod media;
mod player;
mod session;
mod settings;
mod system;
mod tools;
mod util;
mod voice;
mod xchg;

use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model as _, ModelRc, SharedString, Timer, TimerMode, VecModel};

use plugin_engine::Host;
use timeline_engine::{ClipId, Edge, MarkerId, TrackId, TrackKind, TrackUpdate};

use player::{Audio, Clock, Worker};
use session::Session;
use settings::AppSettings;

pub struct State {
    pub ui: slint::Weak<App>,
    pub settings: AppSettings,
    pub session: Option<Session>,
    pub audio: Audio,
    pub worker: Option<Worker>,
    pub worker_rev: u64,
    pub clock: Clock,
    pub asked: i64,
    pub shown: i64,
    pub plugins: Arc<Host>,
    pub nav: Vec<Screen>,
    pub toast_until: Option<Instant>,
    pub jobs: media::Jobs,
    pub voice: voice::Voice,
    pub sys: system::SysState,
    pub tools: tools::ToolState,
    pub last_meter: Instant,
}

thread_local! {
    static ST: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Run `f` with the app state and the window. Returns `None` before start-up or after exit.
pub fn with<R>(f: impl FnOnce(&mut State, &App) -> R) -> Option<R> {
    ST.with(|cell| {
        let mut borrow = cell.try_borrow_mut().ok()?;
        let st = borrow.as_mut()?;
        let ui = st.ui.upgrade()?;
        Some(f(st, &ui))
    })
}

/// Queue `f` to run on the UI thread with the state (safe from any thread).
pub fn later(f: impl FnOnce(&mut State, &App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        with(f);
    });
}

impl State {
    pub fn toast(&mut self, ui: &App, text: impl Into<SharedString>) {
        ui.global::<Model>().set_toast(text.into());
        self.toast_until = Some(Instant::now() + Duration::from_millis(3500));
    }

    pub fn ts(&self) -> bool {
        self.settings.timecode_style == 1
    }

    /// Push the whole project to the UI.
    pub fn sync(&mut self, ui: &App) {
        let ts = self.ts();
        if let Some(s) = self.session.as_mut() {
            s.sync(ui, ts);
        } else {
            ui.global::<Model>().set_has_project(false);
        }
        self.want_frame(true);
    }

    pub fn want_frame(&mut self, force: bool) {
        if let Some(s) = &self.session {
            let idx = s.frame_index();
            if let Some(w) = &self.worker {
                if force || idx != self.asked {
                    w.want(idx);
                    self.asked = idx;
                }
            }
        }
    }

    /// Run an edit on the session; on error show it, on success resync.
    pub fn edit(&mut self, ui: &App, f: impl FnOnce(&mut Session) -> Result<(), String>) {
        let Some(s) = self.session.as_mut() else {
            self.toast(ui, "Open or create a project first.");
            return;
        };
        match f(s) {
            Ok(()) => self.sync(ui),
            Err(e) => self.toast(ui, e),
        }
    }

    pub fn go(&mut self, ui: &App, screen: Screen) {
        let m = ui.global::<Model>();
        let cur = m.get_screen();
        if cur == screen {
            return;
        }
        let needs_project = matches!(
            screen,
            Screen::Editor
                | Screen::ProjectSettings
                | Screen::Inspector
                | Screen::Export
                | Screen::Interchange
                | Screen::Voiceover
                | Screen::Trim
                | Screen::Multicam
                | Screen::Speed
                | Screen::Transitions
                | Screen::Mixer
                | Screen::AudioCleanup
                | Screen::Color
                | Screen::Grading
                | Screen::Scopes
                | Screen::Effects
                | Screen::Titles
                | Screen::Keyframes
                | Screen::Masks
                | Screen::Composition
        );
        if needs_project && self.session.is_none() {
            self.toast(ui, "Open or create a project first.");
            return;
        }
        if cur != Screen::Welcome && cur != Screen::Recovery {
            self.nav.push(cur);
            if self.nav.len() > 40 {
                self.nav.remove(0);
            }
        }
        if self.clock.playing {
            self.stop_playback(ui);
        }
        m.set_previous_screen(cur);
        m.set_screen(screen);
        self.on_enter(ui, screen);
        bump_focus(ui);
    }

    pub fn back(&mut self, ui: &App) {
        let target = loop {
            match self.nav.pop() {
                Some(s) if s == ui.global::<Model>().get_screen() => continue,
                Some(s) => break s,
                None => break if self.session.is_some() { Screen::Editor } else { Screen::Home },
            }
        };
        let m = ui.global::<Model>();
        if self.clock.playing {
            self.stop_playback(ui);
        }
        m.set_screen(target);
        self.on_enter(ui, target);
        bump_focus(ui);
    }

    fn on_enter(&mut self, ui: &App, screen: Screen) {
        match screen {
            Screen::Home => system::refresh_recent(self, ui),
            Screen::Import => media::enter_import(self, ui),
            Screen::Inspector => media::enter_inspector(self, ui),
            Screen::Proxy => media::enter_proxy(self, ui),
            Screen::Export => media::enter_export(self, ui),
            Screen::Voiceover => voice::enter(self, ui),
            Screen::Settings => system::enter_settings(self, ui),
            Screen::Library => system::refresh_library(self, ui),
            _ => tools::enter(self, ui, screen),
        }
        self.sync(ui);
    }

    pub fn play(&mut self, ui: &App) {
        let Some(s) = &self.session else { return };
        let dur = s.duration();
        if dur <= 0.0 {
            self.toast(ui, "The timeline is empty. Add some media first.");
            return;
        }
        let mut from = s.playhead;
        if from >= dur - 0.01 {
            from = 0.0;
        }
        self.clock.play(from);
        self.audio.play(from);
        ui.global::<Model>().set_playing(true);
    }

    pub fn stop_playback(&mut self, ui: &App) {
        let pos = self.clock.stop();
        self.audio.stop();
        if let Some(s) = self.session.as_mut() {
            s.playhead = s.snap_time(pos).as_secs_f64();
        }
        ui.global::<Model>().set_playing(false);
        self.sync(ui);
    }

    pub fn seek(&mut self, ui: &App, secs: f64) {
        let ts = self.ts();
        let Some(s) = self.session.as_mut() else { return };
        let max = s.duration().max(0.0);
        let t = s.snap_time(secs.clamp(0.0, max.max(secs.min(max + 600.0)))).as_secs_f64();
        s.playhead = t;
        if self.clock.playing {
            self.clock.seek(t);
            self.audio.play(t);
        }
        s.sync_time(ui, ts);
        self.want_frame(false);
        tools::on_seek(self, ui);
    }

    pub fn load_session(&mut self, ui: &App, s: Session) {
        if self.clock.playing {
            self.stop_playback(ui);
        }
        self.session = Some(s);
        self.worker = None;
        self.worker_rev = u64::MAX;
        self.asked = -1;
        self.shown = -1;
        ui.global::<Model>().set_has_preview(false);
        media::scan_session_media(self);
        let (name, path, aspect) = {
            let s = self.session.as_ref().unwrap();
            (s.name(), s.path.clone(), s.aspect_name())
        };
        if let Some(p) = path {
            self.settings.touch_recent(&name, &p.to_string_lossy(), &aspect);
            self.settings.save();
        }
        self.sync(ui);
    }

    pub fn selected_clip(&mut self, ui: &App) -> Option<ClipId> {
        let id = self.session.as_mut().and_then(|s| s.ensure_selection());
        if id.is_none() {
            self.toast(ui, "Select a clip first.");
        }
        id
    }
}

pub fn bump_focus(ui: &App) {
    let m = ui.global::<Model>();
    m.set_focus_tick(m.get_focus_tick() + 1);
}

fn tick() {
    with(|st, ui| {
        let m = ui.global::<Model>();
        // keep the preview worker on the latest project
        if let Some(s) = &st.session {
            if st.worker.is_none() || st.worker_rev != s.revision {
                let map = media::proxy_map(st);
                let s = st.session.as_ref().unwrap();
                st.worker = Some(Worker::new(s.project().clone(), s.seq, Arc::clone(&st.plugins), map, || {}));
                st.audio.load(s.project().clone(), s.seq, Arc::clone(&st.plugins));
                st.worker_rev = s.revision;
                st.asked = -1;
                st.want_frame(true);
                if st.clock.playing {
                    st.audio.play(st.clock.position());
                }
            }
        }
        // playback clock
        if st.clock.playing {
            let ts = st.ts();
            let pos = st.clock.position();
            let mut stop = false;
            if let Some(s) = st.session.as_mut() {
                let end = match (s.in_point, s.out_point) {
                    (_, Some(o)) if s.playhead < o => o,
                    _ => s.duration(),
                };
                if pos >= end {
                    s.playhead = end;
                    stop = true;
                } else {
                    s.playhead = pos;
                }
                s.sync_time(ui, ts);
                // keep the playhead on screen
                let zoom = m.get_zoom() as f64;
                let scroll = m.get_scroll() as f64;
                let visible = (ui.window().size().width as f64 / ui.window().scale_factor() as f64 - 100.0) / zoom.max(1.0);
                if s.playhead > scroll + visible * 0.92 || s.playhead < scroll {
                    m.set_scroll((s.playhead - visible * 0.1).max(0.0) as f32);
                }
            }
            if stop {
                st.stop_playback(ui);
            } else {
                st.want_frame(false);
            }
        }
        // collect rendered frames
        if let Some(w) = &st.worker {
            let mut latest = None;
            while let Ok(r) = w.rx.try_recv() {
                latest = Some(r);
            }
            if let Some((idx, frame)) = latest {
                match frame {
                    Ok(f) => {
                        m.set_preview(util::rgba_image(f.width, f.height, &f.data, 1280));
                        m.set_has_preview(true);
                        st.shown = idx;
                        tools::on_frame(st, ui, &f);
                    }
                    Err(e) => {
                        m.set_status(format!("Preview: {e}").into());
                    }
                }
            }
        }
        // meters
        if st.last_meter.elapsed() > Duration::from_millis(50) {
            st.last_meter = Instant::now();
            let lvl = if st.clock.playing { st.audio.level() } else { 0.0 };
            m.set_master_level(lvl);
            voice::tick(st, ui);
        }
        // background jobs
        media::poll_jobs(st, ui);
        // toast timeout
        if let Some(t) = st.toast_until {
            if Instant::now() > t {
                m.set_toast("".into());
                st.toast_until = None;
            }
        }
        // autosave
        system::autosave_tick(st, ui);
    });
}

fn wire_core(ui: &App) {
    let b = ui.global::<Backend>();

    b.on_go(|s| {
        with(|st, ui| st.go(ui, s));
    });
    b.on_back(|| {
        with(|st, ui| st.back(ui));
    });
    b.on_open_tool(|s| {
        with(|st, ui| st.go(ui, s));
    });
    b.on_dismiss_toast(|| {
        with(|st, ui| {
            ui.global::<Model>().set_toast("".into());
            st.toast_until = None;
        });
    });

    // ----- transport -----
    b.on_play_pause(|| {
        with(|st, ui| {
            if st.clock.playing {
                st.stop_playback(ui)
            } else {
                st.play(ui)
            }
        });
    });
    b.on_stop(|| {
        with(|st, ui| st.stop_playback(ui));
    });
    b.on_seek(|t| {
        with(|st, ui| {
            st.seek(ui, t as f64);
            bump_focus(ui);
        });
    });
    b.on_step(|n| {
        with(|st, ui| {
            if st.clock.playing {
                st.stop_playback(ui);
            }
            let (p, f) = match &st.session {
                Some(s) => (s.playhead, s.frame_secs()),
                None => return,
            };
            st.seek(ui, p + n as f64 * f);
        });
    });
    b.on_go_start(|| {
        with(|st, ui| {
            st.seek(ui, 0.0);
            ui.global::<Model>().set_scroll(0.0);
        });
    });
    b.on_go_end(|| {
        with(|st, ui| {
            let d = st.session.as_ref().map(|s| s.duration()).unwrap_or(0.0);
            st.seek(ui, d);
        });
    });
    b.on_set_in(|| {
        with(|st, ui| {
            if let Some(s) = st.session.as_mut() {
                s.in_point = Some(s.playhead);
                if s.out_point.is_some_and(|o| o <= s.playhead) {
                    s.out_point = None;
                }
            }
            st.sync(ui);
        });
    });
    b.on_set_out(|| {
        with(|st, ui| {
            if let Some(s) = st.session.as_mut() {
                s.out_point = Some(s.playhead);
                if s.in_point.is_some_and(|i| i >= s.playhead) {
                    s.in_point = None;
                }
            }
            st.sync(ui);
        });
    });
    b.on_clear_in_out(|| {
        with(|st, ui| {
            if let Some(s) = st.session.as_mut() {
                s.in_point = None;
                s.out_point = None;
            }
            st.sync(ui);
        });
    });

    // ----- timeline -----
    b.on_select_clip(|track, clip| {
        with(|st, ui| {
            if let Some(s) = st.session.as_mut() {
                s.selected = Some(ClipId(clip as u64));
                s.selected_track = Some(TrackId(track as u64));
            }
            ui.global::<Model>().set_effect_selected(-1);
            st.sync(ui);
            tools::on_selection(st, ui);
            bump_focus(ui);
        });
    });
    b.on_clear_selection(|| {
        with(|st, ui| {
            if let Some(s) = st.session.as_mut() {
                s.selected = None;
            }
            st.sync(ui);
        });
    });
    b.on_split_at_playhead(|| {
        with(|st, ui| st.edit(ui, |s| s.split_at_playhead()));
    });
    b.on_delete_selected(|| {
        with(|st, ui| st.edit(ui, |s| s.delete_selected(false)));
    });
    b.on_ripple_delete_selected(|| {
        with(|st, ui| st.edit(ui, |s| s.delete_selected(true)));
    });
    b.on_move_clip(|clip, delta, tracks| {
        with(|st, ui| {
            let snap = ui.global::<Model>().get_snapping();
            st.edit(ui, |s| s.move_clip(ClipId(clip as u64), delta as f64, tracks, snap))
        });
    });
    b.on_trim_clip(|clip, edge, delta, ripple| {
        with(|st, ui| {
            let e = if edge == 0 { Edge::Head } else { Edge::Tail };
            st.edit(ui, |s| s.trim(ClipId(clip as u64), e, delta as f64, ripple))
        });
    });
    b.on_roll_edit(|clip, d| {
        with(|st, ui| st.edit(ui, |s| s.roll(ClipId(clip as u64), d as f64)));
    });
    b.on_slip_clip(|clip, d| {
        with(|st, ui| st.edit(ui, |s| s.slip(ClipId(clip as u64), d as f64)));
    });
    b.on_slide_clip(|clip, d| {
        with(|st, ui| st.edit(ui, |s| s.slide(ClipId(clip as u64), d as f64)));
    });
    b.on_undo(|| {
        with(|st, ui| {
            let r = st.session.as_mut().map(|s| s.undo());
            match r {
                Some(Ok(label)) => {
                    st.sync(ui);
                    st.toast(ui, format!("Undid {label}"));
                }
                Some(Err(_)) => st.toast(ui, "Nothing to undo."),
                None => {}
            }
        });
    });
    b.on_redo(|| {
        with(|st, ui| {
            let r = st.session.as_mut().map(|s| s.redo());
            match r {
                Some(Ok(label)) => {
                    st.sync(ui);
                    st.toast(ui, format!("Redid {label}"));
                }
                Some(Err(_)) => st.toast(ui, "Nothing to redo."),
                None => {}
            }
        });
    });
    b.on_add_marker(|| {
        with(|st, ui| st.edit(ui, |s| s.add_marker()));
    });
    b.on_remove_marker(|id| {
        with(|st, ui| st.edit(ui, |s| s.remove_marker(MarkerId(id as u64))));
    });
    b.on_toggle_track_enabled(|id| {
        with(|st, ui| {
            st.edit(ui, |s| {
                let on = s.sequence().track(TrackId(id as u64)).map(|t| t.enabled).unwrap_or(true);
                s.update_track(TrackId(id as u64), TrackUpdate { enabled: Some(!on), ..Default::default() })
            })
        });
    });
    b.on_toggle_track_locked(|id| {
        with(|st, ui| {
            st.edit(ui, |s| {
                let on = s.sequence().track(TrackId(id as u64)).map(|t| t.locked).unwrap_or(false);
                s.update_track(TrackId(id as u64), TrackUpdate { locked: Some(!on), ..Default::default() })
            })
        });
    });
    b.on_toggle_track_solo(|id| {
        with(|st, ui| {
            st.edit(ui, |s| {
                let on = s.sequence().track(TrackId(id as u64)).map(|t| t.solo).unwrap_or(false);
                s.update_track(TrackId(id as u64), TrackUpdate { solo: Some(!on), ..Default::default() })
            })
        });
    });
    b.on_set_track_gain(|id, db| {
        with(|st, ui| {
            st.edit(ui, |s| {
                let g = session::gain_db(db as f64).ok_or("Bad level")?;
                s.update_track(TrackId(id as u64), TrackUpdate { gain: Some(g), ..Default::default() })
            })
        });
    });
    b.on_set_track_pan(|id, p| {
        with(|st, ui| {
            st.edit(ui, |s| {
                let pan = session::pan_pct(p as f64).ok_or("Bad pan")?;
                s.update_track(TrackId(id as u64), TrackUpdate { pan: Some(pan), ..Default::default() })
            })
        });
    });
    b.on_set_master_gain(|db| {
        with(|st, ui| st.edit(ui, |s| s.set_master_gain(db as f64)));
    });
    b.on_set_clip_volume(|db| {
        with(|st, ui| {
            if let Some(id) = st.selected_clip(ui) {
                st.edit(ui, |s| {
                    let g = session::gain_db(db as f64).ok_or("Bad level")?;
                    s.set_clip_audio(id, timeline_engine::ClipAudio { gain: Some(g), ..Default::default() })
                });
            }
        });
    });
    b.on_set_clip_pan(|p| {
        with(|st, ui| {
            if let Some(id) = st.selected_clip(ui) {
                st.edit(ui, |s| {
                    let pan = session::pan_pct(p as f64).ok_or("Bad pan")?;
                    s.set_clip_audio(id, timeline_engine::ClipAudio { pan: Some(pan), ..Default::default() })
                });
            }
        });
    });
    b.on_set_clip_fades(|fi, fo| {
        with(|st, ui| {
            if let Some(id) = st.selected_clip(ui) {
                st.edit(ui, |s| {
                    s.set_clip_audio(
                        id,
                        timeline_engine::ClipAudio {
                            fade_in: Some(session::fade(fi as f64)),
                            fade_out: Some(session::fade(fo as f64)),
                            ..Default::default()
                        },
                    )
                });
            }
        });
    });
    b.on_set_clip_enabled(|on| {
        with(|st, ui| {
            if let Some(id) = st.selected_clip(ui) {
                st.edit(ui, |s| s.set_enabled(id, on));
            }
        });
    });
    b.on_rename_clip(|name| {
        with(|st, ui| {
            if let Some(id) = st.selected_clip(ui) {
                st.edit(ui, |s| s.rename_clip(id, name.as_str()));
            }
        });
    });
    b.on_add_track(|kind| {
        with(|st, ui| st.edit(ui, |s| s.add_track(if kind == 1 { TrackKind::Audio } else { TrackKind::Video })));
    });
    b.on_set_tool(|t| {
        with(|_, ui| ui.global::<Model>().set_tool(t));
    });
    b.on_zoom_changed(|z| {
        with(|st, ui| {
            let m = ui.global::<Model>();
            let old = m.get_zoom() as f64;
            let z = (z as f64).clamp(2.0, 400.0);
            // keep the playhead where it is on screen
            let ph = st.session.as_ref().map(|s| s.playhead).unwrap_or(0.0);
            let scroll = m.get_scroll() as f64;
            let px = (ph - scroll) * old;
            m.set_zoom(z as f32);
            m.set_scroll((ph - px / z).max(0.0) as f32);
        });
    });
    b.on_drop_asset(|asset, track, start| {
        with(|st, ui| {
            st.edit(ui, |s| {
                let vt = s.track_at_index(TrackKind::Video, track.max(0) as usize);
                s.insert_asset(timeline_engine::AssetId(asset as u64), start as f64, vt, None)
            })
        });
    });
    b.on_insert_asset_at_playhead(|asset| {
        with(|st, ui| {
            if asset < 0 {
                st.toast(ui, "Pick a clip in the media list first.");
                return;
            }
            st.edit(ui, |s| {
                let at = s.playhead;
                let vt = s.selected_track.filter(|t| s.sequence().track(*t).is_some_and(|t| t.kind == TrackKind::Video));
                s.insert_asset(timeline_engine::AssetId(asset as u64), at, vt, None)
            })
        });
    });
    b.on_add_asset_to_timeline(|asset| {
        with(|st, ui| {
            st.edit(ui, |s| {
                let at = s.duration();
                s.insert_asset(timeline_engine::AssetId(asset as u64), at, None, None)
            });
            st.go(ui, Screen::Editor);
        });
    });
    b.on_key(|text, ctrl, shift, alt| with(|st, ui| system::handle_key(st, ui, text.as_str(), ctrl, shift, alt)).unwrap_or(false));
}

fn main() -> Result<(), slint::PlatformError> {
    media_engine::init();
    let settings = AppSettings::load();
    let ui = App::new()?;

    let mut host = Host::new();
    if let Ok(p) = plugin_engine::Plugin::load(&plugin_engine::example_invert_mix_wasm()) {
        host.add("invert-mix.wasm", p);
    }
    let plugins = Arc::new(host);

    let state = State {
        ui: ui.as_weak(),
        settings,
        session: None,
        audio: Audio::new(),
        worker: None,
        worker_rev: u64::MAX,
        clock: Clock::new(),
        asked: -1,
        shown: -1,
        plugins,
        nav: Vec::new(),
        toast_until: None,
        jobs: media::Jobs::default(),
        voice: voice::Voice::default(),
        sys: system::SysState::default(),
        tools: tools::ToolState::default(),
        last_meter: Instant::now(),
    };
    ST.with(|c| *c.borrow_mut() = Some(state));

    system::detect_gpu(&ui);
    wire_core(&ui);
    media::wire(&ui);
    tools::wire(&ui);
    system::wire(&ui);
    voice::wire(&ui);

    with(|st, ui| system::startup(st, ui));

    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(15), tick);

    let r = ui.run();
    with(|st, ui| system::shutdown(st, ui));
    ST.with(|c| *c.borrow_mut() = None);
    r
}

/// Helpers used by several modules.
pub fn vec_model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

pub fn strings(v: impl IntoIterator<Item = impl Into<SharedString>>) -> ModelRc<SharedString> {
    vec_model(v.into_iter().map(Into::into).collect())
}

pub fn model_len<T: Clone + 'static>(m: &ModelRc<T>) -> usize {
    m.row_count()
}
