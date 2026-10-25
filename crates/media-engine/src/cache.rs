//! Byte-budgeted LRU cache of decoded frames and a cached random-access reader.

use std::path::Path;
use std::sync::Arc;

use lru::LruCache;

use crate::decode::VideoDecoder;
use crate::error::{Error, Result};
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

/// Random-access frame reader: a [`VideoDecoder`] fronted by a [`FrameCache`].
///
/// Frames are addressed by index (`time * fps`). Variable-frame-rate files are
/// mapped through their nominal frame rate, which is exact for constant rate
/// footage and consistent (but approximate) for VFR.
pub struct FrameReader {
    decoder: VideoDecoder,
    cache: FrameCache,
    fps: f64,
    /// Index of the frame the decoder will produce next, if known.
    next_index: Option<u64>,
    /// Index of the final frame once end of stream has been seen.
    last_index: Option<u64>,
}

impl FrameReader {
    /// Open `path` with a cache of `cache_bytes`.
    pub fn open(path: impl AsRef<Path>, cache_bytes: usize) -> Result<Self> {
        let decoder = VideoDecoder::open(path)?;
        let fps = decoder.fps();
        Ok(Self {
            decoder,
            cache: FrameCache::new(cache_bytes),
            fps,
            next_index: Some(0),
            last_index: None,
        })
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
        (secs.max(0.0) * self.fps + 1e-6).floor() as u64
    }

    /// Frame displayed at time `secs`.
    pub fn frame_at(&mut self, secs: f64) -> Result<Arc<VideoFrame>> {
        let index = self.index_at(secs);
        self.frame(index)
    }

    /// Frame number `index` (0-based). Past the end it returns the last frame.
    pub fn frame(&mut self, index: u64) -> Result<Arc<VideoFrame>> {
        let index = self.last_index.map_or(index, |last| index.min(last));
        if let Some(f) = self.cache.get(index) {
            return Ok(f);
        }

        let forward_ok = matches!(self.next_index, Some(next) if index >= next && index - next <= MAX_FORWARD_DECODE);
        if !forward_ok {
            // Aim a quarter frame past the frame start so rounding never lands on the previous frame.
            self.decoder.seek((index as f64 + 0.25) / self.fps)?;
            self.next_index = None;
        }

        let mut last_seen: Option<Arc<VideoFrame>> = None;
        loop {
            let Some(frame) = self.decoder.next_frame()? else {
                // End of stream: whatever we saw last is the final frame.
                return match last_seen {
                    Some(f) => {
                        let last = self.next_index.map_or(index, |n| n.saturating_sub(1));
                        self.last_index = Some(last);
                        Ok(f)
                    }
                    None => Err(Error::InvalidArgument(format!("frame {index} is past the end of the stream"))),
                };
            };
            let idx = match self.next_index {
                // Sequential decode: trust the running counter (robust to odd timestamps).
                Some(n) => n,
                // First frame after a seek: derive from its timestamp.
                None => (frame.pts_secs.max(0.0) * self.fps + 0.5).floor() as u64,
            };
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

