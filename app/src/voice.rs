// SPDX-License-Identifier: GPL-3.0-or-later
//! Voiceover: record narration from a microphone straight onto the timeline.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use slint::ComponentHandle;

use timeline_engine as tl;

use crate::{strings, util, vec_model, with, App, Backend, Model, Screen, State, TakeItem};

#[derive(Default)]
pub struct Voice {
    stream: Option<cpal::Stream>,
    level: Arc<AtomicU64>,
    writer: Arc<Mutex<Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>>>,
    recording: Arc<AtomicBool>,
    started: Option<Instant>,
    countdown_until: Option<Instant>,
    takes: Vec<(String, PathBuf, f64)>,
    selected: usize,
    current: Option<PathBuf>,
    device: Option<String>,
}

fn open_input(st: &mut State, ui: &App) {
    st.voice.stream = None;
    let host = cpal::default_host();
    let want = ui.global::<Model>().get_input_devices();
    let idx = ui.global::<Model>().get_input_device().max(0) as usize;
    let want_name = slint::Model::row_data(&want, idx).map(|s| s.to_string());
    let device = host
        .input_devices()
        .ok()
        .and_then(|mut it| it.find(|d| d.description().ok().map(|x| x.name().to_string()) == want_name))
        .or_else(|| host.default_input_device());
    let Some(device) = device else {
        st.toast(ui, "No microphone found.");
        return;
    };
    let Ok(cfg) = device.default_input_config() else {
        st.toast(ui, "That microphone cannot be opened.");
        return;
    };
    let rate = cfg.sample_rate();
    let channels = cfg.channels() as usize;
    let fmt = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.into();
    let level = Arc::clone(&st.voice.level);
    let writer = Arc::clone(&st.voice.writer);
    let rec = Arc::clone(&st.voice.recording);
    let handle = move |data: &[f32]| {
        let mut peak = 0.0f32;
        let mut w = if rec.load(Ordering::Relaxed) { writer.lock().ok() } else { None };
        for frame in data.chunks(channels.max(1)) {
            let v = frame.iter().sum::<f32>() / frame.len().max(1) as f32;
            peak = peak.max(v.abs());
            if let Some(Some(w)) = w.as_deref_mut() {
                let _ = w.write_sample((v.clamp(-1.0, 1.0) * 32767.0) as i16);
            }
        }
        let prev = level.load(Ordering::Relaxed) as f32 / 1000.0;
        let next = if peak > prev { peak } else { prev * 0.9 };
        level.store((next.min(1.0) * 1000.0) as u64, Ordering::Relaxed);
    };
    let err = |e: cpal::Error| eprintln!("input error: {e}");
    let stream = match fmt {
        cpal::SampleFormat::F32 => device.build_input_stream(config, move |d: &[f32], _| handle(d), err, None),
        cpal::SampleFormat::I16 => {
            let mut h = handle;
            device.build_input_stream(
                config,
                move |d: &[i16], _| {
                    let f: Vec<f32> = d.iter().map(|s| *s as f32 / 32768.0).collect();
                    h(&f)
                },
                err,
                None,
            )
        }
        _ => {
            st.toast(ui, "That microphone's sample format is not supported.");
            return;
        }
    };
    match stream {
        Ok(s) => {
            if s.play().is_ok() {
                st.voice.stream = Some(s);
                st.voice.device = want_name;
                st.sys_rate(rate);
            }
        }
        Err(e) => st.toast(ui, format!("Could not open the microphone: {e}")),
    }
}

trait Rate {
    fn sys_rate(&mut self, r: u32);
}
impl Rate for State {
    fn sys_rate(&mut self, r: u32) {
        RATE.store(r as u64, Ordering::Relaxed);
    }
}
static RATE: AtomicU64 = AtomicU64::new(48_000);

pub fn enter(st: &mut State, ui: &App) {
    let m = ui.global::<Model>();
    m.set_input_devices(strings(crate::player::input_devices()));
    let tracks: Vec<String> = st.session.as_ref().map(|s| s.sequence().audio_tracks().iter().map(|t| t.name.clone()).collect()).unwrap_or_default();
    m.set_vo_track(tracks.len().saturating_sub(1) as i32);
    m.set_clip_names(strings(tracks));
    open_input(st, ui);
    refresh_takes(st, ui);
}

fn refresh_takes(st: &mut State, ui: &App) {
    let items: Vec<TakeItem> = st
        .voice
        .takes
        .iter()
        .enumerate()
        .map(|(i, (n, p, d))| TakeItem { name: n.clone().into(), duration: util::short_duration(*d).into(), path: p.to_string_lossy().to_string().into(), selected: i == st.voice.selected })
        .collect();
    ui.global::<Model>().set_takes(vec_model(items));
}

pub fn tick(st: &mut State, ui: &App) {
    if ui.global::<Model>().get_screen() != Screen::Voiceover {
        if st.voice.stream.is_some() && !st.voice.recording.load(Ordering::Relaxed) {
            st.voice.stream = None;
        }
        return;
    }
    let m = ui.global::<Model>();
    m.set_vo_level(st.voice.level.load(Ordering::Relaxed) as f32 / 1000.0);
    if let Some(until) = st.voice.countdown_until {
        let left = until.saturating_duration_since(Instant::now()).as_secs_f32();
        m.set_vo_countdown_left(left.ceil() as i32);
        if left <= 0.0 {
            st.voice.countdown_until = None;
            m.set_vo_countdown_left(0);
            begin(st, ui);
        }
    }
    if let Some(t) = st.voice.started {
        m.set_vo_time(util::timecode(t.elapsed().as_secs_f64(), 100.0, true)[..8].to_string().into());
    }
}

fn begin(st: &mut State, ui: &App) {
    let dir = st
        .session
        .as_ref()
        .and_then(|s| s.path.as_ref())
        .and_then(|p| p.parent().map(|d| d.join("Voiceover")))
        .unwrap_or_else(|| st.settings.storage_dir().join("Voiceover"));
    let _ = std::fs::create_dir_all(&dir);
    let n = st.voice.takes.len() + 1;
    let path = dir.join(format!("Take {n} {}.wav", chrono::Local::now().format("%H%M%S")));
    let spec = hound::WavSpec { channels: 1, sample_rate: RATE.load(Ordering::Relaxed) as u32, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    match hound::WavWriter::create(&path, spec) {
        Ok(w) => {
            *st.voice.writer.lock().unwrap() = Some(w);
            st.voice.recording.store(true, Ordering::Relaxed);
            st.voice.started = Some(Instant::now());
            st.voice.current = Some(path);
            ui.global::<Model>().set_vo_recording(true);
            if st.session.is_some() && !st.clock.playing {
                st.play(ui);
            }
        }
        Err(e) => st.toast(ui, format!("Could not start recording: {e}")),
    }
}

fn finish(st: &mut State, ui: &App) {
    st.voice.recording.store(false, Ordering::Relaxed);
    if let Some(w) = st.voice.writer.lock().unwrap().take() {
        let _ = w.finalize();
    }
    if st.clock.playing {
        st.stop_playback(ui);
    }
    let dur = st.voice.started.take().map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    ui.global::<Model>().set_vo_recording(false);
    if let Some(p) = st.voice.current.take() {
        let n = st.voice.takes.len() + 1;
        st.voice.takes.push((format!("Take {n}"), p, dur));
        st.voice.selected = st.voice.takes.len() - 1;
        refresh_takes(st, ui);
    }
}

pub fn wire(ui: &App) {
    let b = ui.global::<Backend>();
    b.on_vo_select_device(|i| {
        with(|st, ui| {
            ui.global::<Model>().set_input_device(i);
            open_input(st, ui);
        });
    });
    b.on_vo_record(|| {
        with(|st, ui| {
            if st.voice.recording.load(Ordering::Relaxed) {
                finish(st, ui);
                return;
            }
            if st.voice.stream.is_none() {
                open_input(st, ui);
                if st.voice.stream.is_none() {
                    return;
                }
            }
            let secs = match ui.global::<Model>().get_vo_countdown() {
                1 => 3,
                2 => 5,
                _ => 0,
            };
            if ui.global::<Model>().get_vo_insert() == 1 {
                st.seek(ui, 0.0);
            }
            if secs == 0 {
                begin(st, ui);
            } else {
                st.voice.countdown_until = Some(Instant::now() + std::time::Duration::from_secs(secs));
            }
        });
    });
    b.on_vo_stop(|| {
        with(|st, ui| {
            st.voice.countdown_until = None;
            if st.voice.recording.load(Ordering::Relaxed) {
                finish(st, ui);
            }
        });
    });
    b.on_vo_select_take(|i| {
        with(|st, ui| {
            st.voice.selected = i.max(0) as usize;
            refresh_takes(st, ui);
        });
    });
    b.on_vo_keep(|| {
        with(|st, ui| {
            let Some((_, path, dur)) = st.voice.takes.get(st.voice.selected).cloned() else {
                st.toast(ui, "Record a take first.");
                return;
            };
            let m = ui.global::<Model>();
            let track_i = m.get_vo_track().max(0) as usize;
            st.edit(ui, |s| {
                let a = tl::media::probe_asset(&path).map_err(|e| e.to_string())?;
                let id = s.import_asset(a)?;
                let track = s.track_at_index(tl::TrackKind::Audio, track_i).or_else(|| s.first_audio_track());
                let at = (s.playhead - dur).max(0.0);
                s.insert_asset(id, at, None, track)
            });
            let a = st.session.as_ref().and_then(|s| s.project().assets().values().last().cloned());
            if let Some(a) = a {
                crate::media::request_asset_media(st, &a);
            }
            st.toast(ui, "Take added to the timeline.");
        });
    });
    b.on_vo_discard(|| {
        with(|st, ui| {
            if st.voice.selected < st.voice.takes.len() {
                let (_, p, _) = st.voice.takes.remove(st.voice.selected);
                let _ = std::fs::remove_file(p);
                st.voice.selected = st.voice.takes.len().saturating_sub(1);
                refresh_takes(st, ui);
            }
        });
    });
}
