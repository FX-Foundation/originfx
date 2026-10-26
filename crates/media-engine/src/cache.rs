// SPDX-License-Identifier: GPL-3.0-or-later
//! Byte-budgeted LRU cache of decoded frames and a cached random-access reader.

use std::path::Path;
use std::sync::Arc;

use lru::LruCache;

use crate::decode::{DecodeOptions, VideoDecoder};
use crate::error::{Error, Result};
use crate::index::FrameIndex;
use crate::types::VideoFrame;

/// Hit/miss counters, handy for tuning and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
    pub bytes: usize,
}

/// Least-recently-used frame cache that stays under a memory budget.
///
/// Frames are shared as `Arc<VideoFrame>`, so handing one to the renderer is
/// free and eviction never invalidates a frame that is still in use.
pub struct FrameCache {
    lru: LruCache<u64, Arc<VideoFrame>>,
    budget: usize,
    bytes: usize,
    hits: u64,
    misses: u64,
}

impl FrameCache {
    /// Create a cache that holds at most `budget_bytes` of pixel data
    /// (always keeps at least one frame, even if it alone exceeds the budget).
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            lru: LruCache::unbounded(),
            budget: budget_bytes,
            bytes: 0,
            hits: 0,
            misses: 0,
        }
    }

    /// Look a frame up by index, marking it most recently used.
    pub fn get(&mut self, index: u64) -> Option<Arc<VideoFrame>> {
        match self.lru.get(&index) {
            Some(f) => {
                self.hits += 1;
                Some(Arc::clone(f))
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    /// Check for a frame without touching recency or counters.
    pub fn contains(&self, index: u64) -> bool {
        self.lru.contains(&index)
    }

    /// Insert a frame, evicting the least recently used ones if needed.
    pub fn insert(&mut self, index: u64, frame: Arc<VideoFrame>) {
        self.bytes += frame.byte_len();
        if let Some(old) = self.lru.put(index, frame) {
            self.bytes -= old.byte_len();
        }
        while self.bytes > self.budget && self.lru.len() > 1 {
            if let Some((_, old)) = self.lru.pop_lru() {
                self.bytes -= old.byte_len();
            }
        }
    }

    /// Drop everything.
    pub fn clear(&mut self) {
        self.lru.clear();
        self.bytes = 0;
    }

    /// Change the budget; evicts immediately if the cache is now too big.
    pub fn set_budget(&mut self, budget_bytes: usize) {
        self.budget = budget_bytes;
        while self.bytes > self.budget && self.lru.len() > 1 {
            if let Some((_, old)) = self.lru.pop_lru() {
                self.bytes -= old.byte_len();
            }
        }
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits,
            misses: self.misses,
            entries: self.lru.len(),
            bytes: self.bytes,
        }
    }
}

/// How far ahead (in frames) we decode forward instead of seeking.
/// Seeking restarts from a keyframe, which is usually costlier than this.
const MAX_FORWARD_DECODE: u64 = 30;

/// A request this many frames (or fewer) before the previous one counts as
/// scrubbing backwards.
const SCRUB_WINDOW: u64 = 60;

/// When scrubbing backwards, how many extra frames before the requested one are
/// decoded and cached, so the next steps back cost nothing. Backwards is the
/// expensive direction: the decoder can only run forward from a keyframe.
const SCRUB_PREFETCH_MAX: u64 = 16;

/// Random-access frame reader: a [`VideoDecoder`] fronted by a [`FrameCache`].
///
/// Frames are addressed by index. For constant-frame-rate footage the index is
/// `time * fps`. When the container reports a variable frame rate, the reader
/// reads every frame's timestamp up front ([`FrameIndex`]) so lookups stay
/// exact; use [`FrameReader::open_exact`] to force that for any file.
pub struct FrameReader {
    decoder: VideoDecoder,
    cache: FrameCache,
    fps: f64,
    /// Exact frame times; only built for variable-frame-rate footage.
    index: Option<FrameIndex>,
    /// Index of the frame the decoder will produce next, if known.
    next_index: Option<u64>,
    /// Index of the final frame once end of stream has been seen.
    last_index: Option<u64>,
    /// The frame requested last, to notice backward scrubbing.
    last_request: Option<u64>,
    /// Cache budget and the size of one decoded frame, to size the backward prefetch.
    cache_bytes: usize,
    frame_bytes: usize,
}

impl FrameReader {
    /// Open `path` with a cache of `cache_bytes`.
    pub fn open(path: impl AsRef<Path>, cache_bytes: usize) -> Result<Self> {
        Self::open_inner(path.as_ref(), cache_bytes, false, DecodeOptions::default())
    }

    /// Like [`open`](Self::open), with decode options (for example GPU decoding).
    pub fn open_with(
        path: impl AsRef<Path>,
        cache_bytes: usize,
        options: DecodeOptions,
    ) -> Result<Self> {
        Self::open_inner(path.as_ref(), cache_bytes, false, options)
    }

    /// Like [`open`](Self::open) but always reads the exact frame times first.
    /// Costs one pass over the file; worth it when frame accuracy matters.
    pub fn open_exact(path: impl AsRef<Path>, cache_bytes: usize) -> Result<Self> {
        Self::open_inner(path.as_ref(), cache_bytes, true, DecodeOptions::default())
    }

    /// Like [`open_exact`](Self::open_exact), with decode options.
    pub fn open_exact_with(
        path: impl AsRef<Path>,
        cache_bytes: usize,
        options: DecodeOptions,
    ) -> Result<Self> {
        Self::open_inner(path.as_ref(), cache_bytes, true, options)
    }

    fn open_inner(
        path: &Path,
        cache_bytes: usize,
        force_index: bool,
        options: DecodeOptions,
    ) -> Result<Self> {
        let decoder = VideoDecoder::open_with(path, options)?;
        let fps = decoder.fps();
        let stream = decoder.info().index;
        let index = if force_index || FrameIndex::looks_variable(path, stream) {
            Some(FrameIndex::build(path, stream)?)
        } else {
            None
        };
        Ok(Self {
            decoder,
            cache: FrameCache::new(cache_bytes),
            fps,
            index,
            next_index: Some(0),
            last_index: None,
            last_request: None,
            cache_bytes,
            frame_bytes: 0,
        })
    }

    /// The exact frame timeline, when one was built.
    pub fn frame_index(&self) -> Option<&FrameIndex> {
        self.index.as_ref()
    }

    /// Total number of frames, when known exactly (needs the frame index).
    pub fn frame_count(&self) -> Option<u64> {
        self.index.as_ref().map(FrameIndex::len)
    }

    /// Display time of frame `index` in seconds.
    pub fn time_of(&self, index: u64) -> f64 {
        match &self.index {
            Some(ix) => ix.time_of(index),
            None => index as f64 / self.fps,
        }
    }

    pub fn decoder(&self) -> &VideoDecoder {
        &self.decoder
    }

    pub fn fps(&self) -> f64 {
        self.fps
    }

    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// Index of the frame on screen at `secs`.
    pub fn index_at(&self, secs: f64) -> u64 {
        match &self.index {
            Some(ix) => ix.index_at(secs),
            None => (secs.max(0.0) * self.fps + 1e-6).floor() as u64,
        }
    }

    /// Frame displayed at time `secs`.
    pub fn frame_at(&mut self, secs: f64) -> Result<Arc<VideoFrame>> {
        let index = self.index_at(secs);
        self.frame(index)
    }

    /// How many frames to read ahead of a backward step: at most a third of the
    /// cache, so they are not evicted before they are used.
    fn scrub_prefetch(&self) -> u64 {
        if self.frame_bytes == 0 {
            return 0;
        }
        ((self.cache_bytes / 3 / self.frame_bytes) as u64).min(SCRUB_PREFETCH_MAX)
    }

    /// Frame number `index` (0-based). Past the end it returns the last frame.
    pub fn frame(&mut self, index: u64) -> Result<Arc<VideoFrame>> {
        let index = self.last_index.map_or(index, |last| index.min(last));
        let index = self
            .index
            .as_ref()
            .map_or(index, |ix| index.min(ix.len() - 1));
        let previous_request = self.last_request.replace(index);
        if let Some(f) = self.cache.get(index) {
            return Ok(f);
        }

        let forward_ok = matches!(self.next_index, Some(next) if index >= next && index - next <= MAX_FORWARD_DECODE);
        if !forward_ok {
            // Scrubbing backwards: start a little earlier and keep the frames on the
            // way, so the next steps back are cache hits instead of new seeks.
            let scrubbing_back =
                matches!(previous_request, Some(p) if index < p && p - index <= SCRUB_WINDOW);
            let aim = if scrubbing_back {
                index.saturating_sub(self.scrub_prefetch())
            } else {
                index
            };
            // Aim a quarter frame past the frame start so rounding never lands on the previous frame.
            let target = match &self.index {
                Some(ix) => ix.time_of(aim) + 0.25 * ix.duration_of(aim),
                None => (aim as f64 + 0.25) / self.fps,
            };
            self.decoder.seek(target)?;
            self.next_index = None;
        }

        let mut last_seen: Option<Arc<VideoFrame>> = None;
        loop {
            let Some(frame) = self.decoder.next_frame()? else {
                // End of stream: the last frame we produced is the final frame.
                let Some(next) = self.next_index.filter(|&n| n > 0) else {
                    return Err(Error::InvalidArgument(format!(
                        "frame {index} is past the end of the stream"
                    )));
                };
                let last = next - 1;
                self.last_index = Some(last);
                return match last_seen {
                    Some(f) => Ok(f),
                    // We were already sitting at the end; fetch the final frame (from cache or by seeking).
                    None => self.frame(last),
                };
            };
            let idx = match self.next_index {
                // Sequential decode: trust the running counter (robust to odd timestamps).
                Some(n) => n,
                // First frame after a seek: derive from its timestamp.
                None => match &self.index {
                    Some(ix) => ix.nearest(frame.pts_secs),
                    None => (frame.pts_secs.max(0.0) * self.fps + 0.5).floor() as u64,
                },
            };
            self.frame_bytes = frame.byte_len();
            let frame = Arc::new(frame);
            self.cache.insert(idx, Arc::clone(&frame));
            self.next_index = Some(idx + 1);
            last_seen = Some(Arc::clone(&frame));
            if idx >= index {
                return Ok(frame);
            }
        }
    }
}
