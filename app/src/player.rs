// SPDX-License-Identifier: GPL-3.0-or-later
//! Preview playback: a render thread for pictures and a cpal stream for sound.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use media_engine::VideoFrame;
use plugin_engine::Host;
use render_engine::{FileSource, MediaSource, MissingMedia, RenderSettings, Renderer};
use std::collections::HashMap;
use std::path::PathBuf;

/// Reads files like `FileSource`, but swaps in proxy files for the preview where they exist.
pub struct ProxySource {
    inner: FileSource,
    map: HashMap<PathBuf, PathBuf>,
}

impl ProxySource {
    pub fn new(map: HashMap<PathBuf, PathBuf>) -> ProxySource {
        ProxySource { inner: FileSource::new(), map }
    }
}

impl MediaSource for ProxySource {
    fn video_frame(
        &mut self,
        asset: &timeline_engine::Asset,
        stream: u32,
        secs: f64,
    ) -> render_engine::Result<Arc<VideoFrame>> {
        match self.map.get(&asset.path) {
            Some(p) => {
                let mut a = asset.clone();
                a.path = p.clone();
                self.inner.video_frame(&a, stream, secs)
            }
            None => self.inner.video_frame(asset, stream, secs),
        }
    }

    fn audio(
        &mut self,
        asset: &timeline_engine::Asset,
        stream: u32,
        secs: f64,
        frames: usize,
        rate: u32,
    ) -> render_engine::Result<Vec<f32>> {
        self.inner.audio(asset, stream, secs, frames, rate)
    }
}
use timeline_engine::{Project, SequenceId};

/// Renders frames of one project snapshot. A newer request replaces an older one that has not
/// started yet, so scrubbing never falls behind.
pub struct Worker {
    tx: Sender<i64>,
    pub rx: Receiver<(i64, Result<VideoFrame, String>)>,
    pub alive: Arc<AtomicBool>,
}

impl Worker {
    pub fn new(
        project: Project,
        seq: SequenceId,
        plugins: Arc<Host>,
        proxies: HashMap<PathBuf, PathBuf>,
        notify: impl Fn() + Send + 'static,
    ) -> Worker {
        let (tx, req) = channel::<i64>();
        let (out, rx) = channel::<(i64, Result<VideoFrame, String>)>();
        let alive = Arc::new(AtomicBool::new(true));
        let alive2 = Arc::clone(&alive);
        thread::Builder::new()
            .name("preview".into())
            .spawn(move || {
                let settings = RenderSettings {
                    missing_media: MissingMedia::Offline,
                    ..Default::default()
                };
                let renderer = Renderer::new(&project, seq, ProxySource::new(proxies), settings).map(|r| r.with_plugins(plugins));
                let mut renderer = match renderer {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = out.send((0, Err(e.to_string())));
                        notify();
                        return;
                    }
                };
                while let Ok(mut index) = req.recv() {
                    while let Ok(newer) = req.try_recv() {
                        index = newer;
                    }
                    if !alive2.load(Ordering::Relaxed) {
                        return;
                    }
                    let frame = renderer.frame(index).map_err(|e| e.to_string());
                    if out.send((index, frame)).is_err() {
                        return;
                    }
                    notify();
                }
            })
            .expect("preview thread");
        Worker { tx, rx, alive }
    }

    pub fn want(&self, index: i64) {
        let _ = self.tx.send(index);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

enum Cmd {
    Load(Box<Project>, SequenceId, Arc<Host>),
    Play(f64),
    Stop,
}

/// Sound output. `level` holds the latest peak (0..1000) for meters.
pub struct Audio {
    tx: Sender<Cmd>,
    queue: Arc<Mutex<VecDeque<f32>>>,
    _stream: Option<cpal::Stream>,
    pub device: String,
    pub level: Arc<AtomicU64>,
    pub ok: bool,
}

impl Audio {
    pub fn new() -> Audio {
        let queue: Arc<Mutex<VecDeque<f32>>> = Arc::default();
        let level = Arc::new(AtomicU64::new(0));
        let (tx, rx) = channel::<Cmd>();
        let (stream, device, rate) = match open_stream(Arc::clone(&queue), Arc::clone(&level)) {
            Ok((s, name, rate)) => (Some(s), name, rate),
            Err(e) => (None, format!("no sound: {e}"), 48_000),
        };
        let ok = stream.is_some();
        let q = Arc::clone(&queue);
        thread::Builder::new()
            .name("audio-mix".into())
            .spawn(move || producer(rx, q, rate, ok))
            .expect("audio thread");
        Audio { tx, queue, _stream: stream, device, level, ok }
    }

    pub fn load(&self, p: Project, seq: SequenceId, plugins: Arc<Host>) {
        let _ = self.tx.send(Cmd::Load(Box::new(p), seq, plugins));
    }

    pub fn play(&self, from_secs: f64) {
        self.queue.lock().unwrap().clear();
        let _ = self.tx.send(Cmd::Play(from_secs));
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
        self.queue.lock().unwrap().clear();
        self.level.store(0, Ordering::Relaxed);
    }

    pub fn level(&self) -> f32 {
        self.level.load(Ordering::Relaxed) as f32 / 1000.0
    }
}

fn open_stream(queue: Arc<Mutex<VecDeque<f32>>>, level: Arc<AtomicU64>) -> Result<(cpal::Stream, String, u32), String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no output device found")?;
    let name = device
        .description()
        .map_or_else(|_| "default output".into(), |d| d.name().to_string());
    let cfg = device.default_output_config().map_err(|e| e.to_string())?;
    let rate = cfg.sample_rate();
    let channels = usize::from(cfg.channels());
    let fmt = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.into();
    let err = |e: cpal::Error| eprintln!("audio stream error: {e}");
    let stream = match fmt {
        cpal::SampleFormat::F32 => device.build_output_stream(
            config,
            move |data: &mut [f32], _| fill(data, channels, &queue, &level, |v| v),
            err,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            config,
            move |data: &mut [i16], _| fill(data, channels, &queue, &level, |v| (v * 32767.0) as i16),
            err,
            None,
        ),
        other => return Err(format!("unsupported sample format {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, name, rate))
}

fn fill<T: Copy + Default>(
    out: &mut [T],
    channels: usize,
    queue: &Mutex<VecDeque<f32>>,
    level: &AtomicU64,
    conv: impl Fn(f32) -> T,
) {
    let mut q = queue.lock().unwrap();
    let mut peak = 0.0f32;
    for frame in out.chunks_mut(channels.max(1)) {
        let (l, r) = (q.pop_front().unwrap_or(0.0), q.pop_front().unwrap_or(0.0));
        peak = peak.max(l.abs()).max(r.abs());
        for (i, s) in frame.iter_mut().enumerate() {
            *s = match (channels, i) {
                (1, _) => conv((l + r) * 0.5),
                (_, 0) => conv(l),
                (_, 1) => conv(r),
                _ => T::default(),
            };
        }
    }
    // Smooth decay so meters do not flicker.
    let prev = level.load(Ordering::Relaxed) as f32 / 1000.0;
    let next = if peak > prev { peak } else { prev * 0.85 };
    level.store((next.min(1.0) * 1000.0) as u64, Ordering::Relaxed);
}

fn producer(rx: Receiver<Cmd>, queue: Arc<Mutex<VecDeque<f32>>>, dev_rate: u32, live: bool) {
    let mut renderer: Option<Renderer<FileSource>> = None;
    let mut playing = false;
    let mut next: i64 = 0;
    let mut seq_rate = 48_000.0;
    let mut frac = 0.0f64;
    let mut prev = (0.0f32, 0.0f32);
    loop {
        let cmd = if playing && live {
            rx.recv_timeout(Duration::from_millis(20)).ok()
        } else {
            match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            }
        };
        match cmd {
            Some(Cmd::Load(p, seq, plugins)) => {
                let settings = RenderSettings {
                    missing_media: MissingMedia::Offline,
                    ..Default::default()
                };
                renderer = Renderer::new(&p, seq, FileSource::new(), settings)
                    .ok()
                    .map(|r| r.with_plugins(plugins));
                if let Some(r) = &renderer {
                    seq_rate = f64::from(r.sequence().sample_rate);
                }
                // keep playing after a live edit
                if playing {
                    queue.lock().unwrap().clear();
                }
            }
            Some(Cmd::Play(from)) => {
                next = (from * seq_rate) as i64;
                frac = 0.0;
                playing = true;
            }
            Some(Cmd::Stop) => playing = false,
            None => {}
        }
        if !(playing && live) {
            continue;
        }
        let Some(r) = renderer.as_mut() else { continue };
        let want = (dev_rate / 4) as usize * 2;
        while queue.lock().unwrap().len() < want {
            let n = 2048;
            match r.audio_samples(next, n) {
                Ok(src) if src.len() >= 2 => {
                    next += n as i64;
                    let step = seq_rate / f64::from(dev_rate);
                    let mut out = Vec::with_capacity(src.len());
                    let frames = src.len() / 2;
                    while frac < frames as f64 {
                        let i = frac.floor() as usize;
                        let t = (frac - i as f64) as f32;
                        let a = if i == 0 { prev } else { (src[(i - 1) * 2], src[(i - 1) * 2 + 1]) };
                        let b = (src[i * 2], src[i * 2 + 1]);
                        out.push(a.0 + (b.0 - a.0) * t);
                        out.push(a.1 + (b.1 - a.1) * t);
                        frac += step;
                    }
                    frac -= frames as f64;
                    prev = (src[(frames - 1) * 2], src[(frames - 1) * 2 + 1]);
                    queue.lock().unwrap().extend(out);
                }
                _ => {
                    playing = false;
                    break;
                }
            }
        }
    }
}

/// Wall-clock playback position.
pub struct Clock {
    pub playing: bool,
    pub started: Instant,
    pub from: f64,
}

impl Clock {
    pub fn new() -> Clock {
        Clock { playing: false, started: Instant::now(), from: 0.0 }
    }
    pub fn position(&self) -> f64 {
        if self.playing {
            self.from + self.started.elapsed().as_secs_f64()
        } else {
            self.from
        }
    }
    pub fn play(&mut self, from: f64) {
        self.from = from;
        self.started = Instant::now();
        self.playing = true;
    }
    pub fn stop(&mut self) -> f64 {
        let p = self.position();
        self.from = p;
        self.playing = false;
        p
    }
    pub fn seek(&mut self, to: f64) {
        self.from = to.max(0.0);
        self.started = Instant::now();
    }
}

/// Lists audio input devices for the voiceover screen.
pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let mut names = Vec::new();
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            if let Ok(desc) = d.description() {
                names.push(desc.name().to_string());
            }
        }
    }
    if names.is_empty() {
        names.push("Built-in Microphone".into());
    }
    names
}
