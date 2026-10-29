// SPDX-License-Identifier: GPL-3.0-or-later
//! Where the renderer gets pictures and sound from.
//!
//! The renderer asks a [`MediaSource`] for "frame at this time of this asset" and "these
//! samples of that asset". [`FileSource`] answers from real files through the Media Engine.
//! Tests, and later proxies or a cache server, provide their own.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;

use media_engine::{AudioDecoder, AudioOptions, FrameReader, VideoFrame};
use timeline_engine::{Asset, AssetId};

use crate::error::{Error, Result};

/// Supplies decoded media to the renderer.
pub trait MediaSource {
    /// The picture of video `stream` of `asset` at `secs` of media time (RGBA8, upright).
    fn video_frame(&mut self, asset: &Asset, stream: u32, secs: f64) -> Result<Arc<VideoFrame>>;

    /// `frames` stereo sample frames (interleaved `f32`, so `frames * 2` values) at `rate`,
    /// starting at media time `secs`. Silence where the media has no sound (before its start
    /// or after its end). Always returns exactly `frames * 2` values.
    fn audio(
        &mut self,
        asset: &Asset,
        stream: u32,
        secs: f64,
        frames: usize,
        rate: u32,
    ) -> Result<Vec<f32>>;
}

/// Bytes of decoded frames each open video file may cache.
const DEFAULT_CACHE_BYTES: usize = 128 * 1024 * 1024;

/// Reads real files through the Media Engine, keeping each file open between requests.
pub struct FileSource {
    video: HashMap<AssetId, FrameReader>,
    audio: HashMap<AssetId, AudioCursor>,
    cache_bytes: usize,
}

impl Default for FileSource {
    fn default() -> Self {
        FileSource::new()
    }
}

impl FileSource {
    pub fn new() -> Self {
        FileSource::with_cache(DEFAULT_CACHE_BYTES)
    }

    /// Cache this many bytes of decoded frames per open video file.
    pub fn with_cache(cache_bytes: usize) -> Self {
        FileSource {
            video: HashMap::new(),
            audio: HashMap::new(),
            cache_bytes,
        }
    }
}

fn require_file(asset: &Asset) -> Result<()> {
    if asset.path.exists() {
        Ok(())
    } else {
        Err(Error::MissingMedia {
            asset: asset.id,
            name: asset.name.clone(),
            path: asset.path.clone(),
        })
    }
}

impl MediaSource for FileSource {
    fn video_frame(&mut self, asset: &Asset, stream: u32, secs: f64) -> Result<Arc<VideoFrame>> {
        if stream != 0 {
            return Err(Error::Unsupported(
                "only the first video stream of a file can be rendered so far".into(),
            ));
        }
        require_file(asset)?;
        if let Entry::Vacant(slot) = self.video.entry(asset.id) {
            slot.insert(FrameReader::open(&asset.path, self.cache_bytes)?);
        }
        let reader = self.video.get_mut(&asset.id).expect("just inserted");
        // A still image is one frame held for as long as the edit needs.
        let secs = if asset.is_still() { 0.0 } else { secs.max(0.0) };
        Ok(reader.frame_at(secs)?)
    }

    fn audio(
        &mut self,
        asset: &Asset,
        stream: u32,
        secs: f64,
        frames: usize,
        rate: u32,
    ) -> Result<Vec<f32>> {
        if stream != 0 {
            return Err(Error::Unsupported(
                "only the first audio stream of a file can be rendered so far".into(),
            ));
        }
        require_file(asset)?;
        if let Entry::Vacant(slot) = self.audio.entry(asset.id) {
            let dec = AudioDecoder::open(
                &asset.path,
                AudioOptions {
                    sample_rate: Some(rate),
                    channels: Some(2),
                },
            )?;
            slot.insert(AudioCursor::new(dec, rate));
        }
        let cursor = self.audio.get_mut(&asset.id).expect("just inserted");
        if cursor.rate != rate {
            // The sequence's rate changed since this file was opened.
            self.audio.remove(&asset.id);
            return self.audio(asset, stream, secs, frames, rate);
        }
        let start = (secs * f64::from(rate)).round() as i64;
        cursor.read(start, frames)
    }
}

/// A decoder that hands out consecutive stretches of sound without re-seeking each time.
struct AudioCursor {
    dec: AudioDecoder,
    rate: u32,
    /// Decoded stereo samples not yet handed out.
    buf: Vec<f32>,
    /// Media sample index of `buf[0]`.
    buf_start: i64,
    /// Whether the decoder is positioned right after `buf` (false until the first seek).
    positioned: bool,
    eof: bool,
}

impl AudioCursor {
    fn new(dec: AudioDecoder, rate: u32) -> Self {
        AudioCursor {
            dec,
            rate,
            buf: Vec::new(),
            buf_start: 0,
            positioned: false,
            eof: false,
        }
    }

    fn buf_end(&self) -> i64 {
        self.buf_start + (self.buf.len() / 2) as i64
    }

    /// `n` stereo sample frames starting at media sample `start`; zeros where there is no sound.
    fn read(&mut self, start: i64, n: usize) -> Result<Vec<f32>> {
        let end = start + n as i64;
        let far_ahead = start > self.buf_end() + i64::from(self.rate) / 2;
        let mut seeked = false;
        if !self.positioned || start < self.buf_start || far_ahead {
            self.dec
                .seek((start.max(0)) as f64 / f64::from(self.rate))?;
            self.buf.clear();
            self.buf_start = start;
            self.positioned = true;
            self.eof = false;
            seeked = true;
        }
        while self.buf_end() < end && !self.eof {
            match self.dec.next_buffer()? {
                None => self.eof = true,
                Some(b) => {
                    if seeked && self.buf.is_empty() {
                        // After a seek the decoder says where it really landed.
                        self.buf_start = (b.pts_secs * f64::from(self.rate)).round() as i64;
                    }
                    self.buf.extend_from_slice(&b.samples);
                }
            }
        }
        let mut out = vec![0f32; n * 2];
        let from = start.max(self.buf_start);
        let to = end.min(self.buf_end());
        if from < to {
            let src = ((from - self.buf_start) * 2) as usize;
            let dst = ((from - start) * 2) as usize;
            let len = ((to - from) * 2) as usize;
            out[dst..dst + len].copy_from_slice(&self.buf[src..src + len]);
        }
        // Everything before `end` has been used.
        if self.buf_end() > end {
            let drop = ((end - self.buf_start).max(0) * 2) as usize;
            self.buf.drain(..drop.min(self.buf.len()));
            self.buf_start = end.max(self.buf_start);
        } else {
            self.buf_start = self.buf_end();
            self.buf.clear();
        }
        Ok(out)
    }
}
