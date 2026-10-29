// SPDX-License-Identifier: GPL-3.0-or-later
//! Sound: a thread renders the mix ahead of the speakers into a queue, and the sound card
//! drains the queue. Video follows the wall clock, so a little drift over a long play is
//! possible; this is a test app.

use std::collections::VecDeque;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use plugin_engine::Host;
use render_engine::{FileSource, MissingMedia, RenderSettings, Renderer};
use timeline_engine::{Project, SequenceId};

enum Cmd {
    Load(Box<Project>, SequenceId, Arc<Host>),
    Play(f64),
    Stop,
}

pub struct Audio {
    tx: Sender<Cmd>,
    queue: Arc<Mutex<VecDeque<f32>>>,
    _stream: Option<cpal::Stream>,
    pub device: String,
}

impl Audio {
    pub fn new() -> Audio {
        let queue: Arc<Mutex<VecDeque<f32>>> = Arc::default();
        let (tx, rx) = channel::<Cmd>();
        let (stream, device, rate) = match open_stream(Arc::clone(&queue)) {
            Ok((s, name, rate)) => (Some(s), name, rate),
            Err(e) => (None, format!("no sound: {e}"), 48_000),
        };
        let has_stream = stream.is_some();
        let q = Arc::clone(&queue);
        thread::spawn(move || producer(rx, q, rate, has_stream));
        Audio {
            tx,
            queue,
            _stream: stream,
            device,
        }
    }

    pub fn load(&self, p: Project, seq: SequenceId, plugins: Arc<Host>) {
        self.stop();
        let _ = self.tx.send(Cmd::Load(Box::new(p), seq, plugins));
    }

    pub fn play(&self, from_secs: f64) {
        self.queue.lock().unwrap().clear();
        let _ = self.tx.send(Cmd::Play(from_secs));
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
        self.queue.lock().unwrap().clear();
    }
}

fn open_stream(queue: Arc<Mutex<VecDeque<f32>>>) -> Result<(cpal::Stream, String, u32), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no output device found")?;
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
            move |data: &mut [f32], _| fill(data, channels, &queue, |v| v),
            err,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            config,
            move |data: &mut [i16], _| fill(data, channels, &queue, |v| (v * 32767.0) as i16),
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
    conv: impl Fn(f32) -> T,
) {
    let mut q = queue.lock().unwrap();
    for frame in out.chunks_mut(channels.max(1)) {
        let (l, r) = (q.pop_front().unwrap_or(0.0), q.pop_front().unwrap_or(0.0));
        for (i, s) in frame.iter_mut().enumerate() {
            *s = match (channels, i) {
                (1, _) => conv((l + r) * 0.5),
                (_, 0) => conv(l),
                (_, 1) => conv(r),
                _ => T::default(),
            };
        }
    }
}

fn producer(
    rx: std::sync::mpsc::Receiver<Cmd>,
    queue: Arc<Mutex<VecDeque<f32>>>,
    dev_rate: u32,
    live: bool,
) {
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
                playing = false;
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
        // Keep about half a second queued.
        let want = (dev_rate / 2) as usize * 2;
        while queue.lock().unwrap().len() < want {
            let n = 2048;
            match r.audio_samples(next, n) {
                Ok(src) => {
                    next += n as i64;
                    // Linear resample from the sequence rate to the device rate.
                    let step = seq_rate / f64::from(dev_rate);
                    let mut out = Vec::with_capacity(src.len());
                    let frames = src.len() / 2;
                    while frac < frames as f64 {
                        let i = frac.floor() as usize;
                        let t = (frac - i as f64) as f32;
                        let a = if i == 0 {
                            prev
                        } else {
                            (src[(i - 1) * 2], src[(i - 1) * 2 + 1])
                        };
                        let b = (src[i * 2], src[i * 2 + 1]);
                        out.push(a.0 + (b.0 - a.0) * t);
                        out.push(a.1 + (b.1 - a.1) * t);
                        frac += step;
                    }
                    frac -= frames as f64;
                    prev = (src[(frames - 1) * 2], src[(frames - 1) * 2 + 1]);
                    queue.lock().unwrap().extend(out);
                }
                Err(_) => {
                    playing = false;
                    break;
                }
            }
        }
    }
}
