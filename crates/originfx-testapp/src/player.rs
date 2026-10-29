// SPDX-License-Identifier: GPL-3.0-or-later
//! The preview: a background thread that renders frames of a project snapshot on request.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use media_engine::{DecodeOptions, HwPolicy, VideoFrame};
use plugin_engine::Host;
use render_engine::{FileSource, MissingMedia, RenderSettings, Renderer};
use timeline_engine::{Project, SequenceId};

pub struct Rendered {
    pub index: i64,
    pub frame: Result<VideoFrame, String>,
    pub millis: f64,
}

/// Renders frames of one project snapshot. Asking for a frame while another is being drawn
/// replaces the older request, so scrubbing and playback skip ahead instead of falling behind.
pub struct Worker {
    tx: Sender<i64>,
    pub rx: Receiver<Rendered>,
}

impl Worker {
    pub fn new(project: Project, seq: SequenceId, plugins: Arc<Host>, gpu: bool) -> Worker {
        let (tx, req) = channel::<i64>();
        let (out, rx) = channel::<Rendered>();
        thread::spawn(move || {
            let decode = DecodeOptions {
                hw: if gpu { HwPolicy::Auto } else { HwPolicy::Off },
            };
            let settings = RenderSettings {
                // Show missing files as magenta rather than stopping the preview.
                missing_media: MissingMedia::Offline,
                ..Default::default()
            };
            let renderer = Renderer::new(
                &project,
                seq,
                FileSource::new().with_decode(decode),
                settings,
            )
            .map(|r| r.with_plugins(plugins));
            let mut renderer = match renderer {
                Ok(r) => r,
                Err(e) => {
                    let _ = out.send(Rendered {
                        index: 0,
                        frame: Err(e.to_string()),
                        millis: 0.0,
                    });
                    return;
                }
            };
            while let Ok(mut index) = req.recv() {
                while let Ok(newer) = req.try_recv() {
                    index = newer;
                }
                let t = Instant::now();
                let frame = renderer.frame(index).map_err(|e| e.to_string());
                let millis = t.elapsed().as_secs_f64() * 1000.0;
                if out
                    .send(Rendered {
                        index,
                        frame,
                        millis,
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
        Worker { tx, rx }
    }

    pub fn want(&self, index: i64) {
        let _ = self.tx.send(index);
    }
}
