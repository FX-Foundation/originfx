//! Proxy workflow: small, fast-to-scrub copies of heavy source files.
//!
//! Editing 4K H.265 directly is slow. A proxy is a low-resolution H.264 copy
//! with frequent keyframes that the editor can decode and seek quickly, while
//! the final export still reads the original.
//!
//! Proxies live in one directory and are named from the source's path, size
//! and modification time, so editing or replacing the source automatically
//! invalidates its proxy.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::error::{Error, Result};
use crate::preset::{ExportPreset, Quality};
use crate::probe::probe;
use crate::transcode::{export_with_progress, ExportSettings, Progress};

/// How proxies are generated.
#[derive(Debug, Clone, Copy)]
pub struct ProxySettings {
    /// Proxy height in pixels (never larger than the source).
    pub height: u32,
    /// Keyframe interval in frames. Small values make scrubbing snappy.
    pub keyint: u32,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            height: 540,
            keyint: 12,
        }
    }
}

/// Creates and finds proxy files in one directory.
#[derive(Debug, Clone)]
pub struct ProxyManager {
    dir: PathBuf,
    settings: ProxySettings,
}

/// 64-bit FNV-1a. Stable across Rust versions, unlike `DefaultHasher`,
/// which matters because the hash ends up in file names on disk.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

impl ProxyManager {
    /// Proxies are stored in `dir` (created on first use).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            settings: ProxySettings::default(),
        }
    }

    pub fn with_settings(mut self, settings: ProxySettings) -> Self {
        self.settings = settings;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn settings(&self) -> ProxySettings {
        self.settings
    }

    /// Where the proxy for `source` lives (whether or not it exists yet).
    pub fn proxy_path(&self, source: impl AsRef<Path>) -> Result<PathBuf> {
        let source = source.as_ref();
        let meta = std::fs::metadata(source).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound(source.to_path_buf()),
            _ => Error::Io(e),
        })?;
        let canonical = std::fs::canonicalize(source)?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());

        let mut key = canonical.to_string_lossy().into_owned().into_bytes();
        key.extend_from_slice(&meta.len().to_le_bytes());
        key.extend_from_slice(&mtime.to_le_bytes());
        key.extend_from_slice(&self.settings.height.to_le_bytes());
        key.extend_from_slice(&self.settings.keyint.to_le_bytes());

        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "media".into());
        let stem: String = stem
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .take(48)
            .collect();
        Ok(self
            .dir
            .join(format!("{stem}-{:016x}.proxy.mp4", fnv1a(&key))))
    }

    /// The proxy for `source` if it has already been generated.
    pub fn existing(&self, source: impl AsRef<Path>) -> Option<PathBuf> {
        let path = self.proxy_path(source).ok()?;
        path.is_file().then_some(path)
    }

    /// Return the proxy for `source`, generating it if needed.
    pub fn ensure(&self, source: impl AsRef<Path>) -> Result<PathBuf> {
        self.ensure_with_progress(source, |_| true)
    }

    /// Like [`ensure`](Self::ensure) with progress and cancellation.
    pub fn ensure_with_progress(
        &self,
        source: impl AsRef<Path>,
        on_progress: impl FnMut(&Progress) -> bool,
    ) -> Result<PathBuf> {
        let source = source.as_ref();
        let target = self.proxy_path(source)?;
        if target.is_file() {
            return Ok(target);
        }
        std::fs::create_dir_all(&self.dir)?;

        let info = probe(source)?;
        let src_height = info
            .primary_video()
            .map(|v| v.height)
            .ok_or(Error::NoStream("video"))?;
        let settings = ExportSettings::new(ExportPreset::Mp4H264)
            .quality(Quality::Draft)
            .height(self.settings.height.min(src_height))
            .keyint(self.settings.keyint)
            .audio_bitrate(128_000);

        // Write next to the final name and rename, so a crash never leaves a
        // half-written file that looks like a finished proxy.
        let tmp = target.with_extension("partial.mp4");
        let _ = std::fs::remove_file(&tmp);
        export_with_progress(source, &tmp, &settings, on_progress)?;
        std::fs::rename(&tmp, &target)?;
        Ok(target)
    }

    /// Pick what the editor should open: the proxy when `use_proxy` is set and
    /// one exists, otherwise the original.
    pub fn resolve(&self, source: impl AsRef<Path>, use_proxy: bool) -> PathBuf {
        let source = source.as_ref();
        if use_proxy {
            if let Some(p) = self.existing(source) {
                return p;
            }
        }
        source.to_path_buf()
    }

    /// Delete proxies whose source files are gone or changed. Returns how many were removed.
    ///
    /// A proxy is kept only if some file in `live_sources` maps to it.
    pub fn prune<P: AsRef<Path>>(&self, live_sources: &[P]) -> Result<usize> {
        let keep: std::collections::HashSet<PathBuf> = live_sources
            .iter()
            .filter_map(|s| self.proxy_path(s).ok())
            .collect();
        let mut removed = 0;
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Ok(0);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if (name.ends_with(".proxy.mp4") || name.ends_with(".partial.mp4"))
                && !keep.contains(&path)
            {
                std::fs::remove_file(&path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}
