# originfx-media-engine

Engine #1 of OriginFX. Everything that touches a media *file*: probing,
decoding, seeking, caching, encoding, proxies. A plain Rust library with no UI,
built on FFmpeg 6.1. Its public API contains no FFmpeg types.

```rust
use media_engine::{export, probe, ExportPreset, ExportSettings, FrameReader, Quality};

let info = probe("clip.mp4")?;                       // codecs, size, fps, tracks
let mut reader = FrameReader::open("clip.mp4", 512 << 20)?;
let frame = reader.frame_at(12.5)?;                  // RGBA8, upright, frame-accurate
export("clip.mp4", "clip-720p.mp4",
       &ExportSettings::new(ExportPreset::Mp4H264).quality(Quality::Standard).height(720))?;
```

## What works

| Area | Status |
|------|--------|
| Probe (container, video/audio/subtitle streams, metadata, rotation) | done, tested |
| Software video decode to RGBA, any codec FFmpeg has | done, tested (8-bit and 10-bit) |
| Audio decode to interleaved `f32`, resample and remix | done, tested |
| Frame-accurate seeking (MP4, MKV, WebM, MOV, MPEG-TS) | done, tested against frames that encode their own number |
| Byte-budgeted LRU frame cache + random-access reader | done, tested |
| Phone portrait rotation (display matrix) | done, tested |
| Software export, 19 presets, scale, fps conform, progress, cancel | done, all 19 tested here |
| Proxy workflow (content-addressed, atomic, prune) | done, tested |
| GPU decode (CUDA/NVDEC, VA-API) | done, tested on real hardware; off by default (`DecodeOptions`) |
| GPU encode (NVENC, H.264/H.265 presets) | done, tested on real hardware; off by default (`ExportSettings::hw_encode`) |
| Zero-copy hand-off to wgpu | **not done**: needs the Render Engine and a GPU |
| Cinema RAW | **out of scope for the engine**: future plugin, see `docs/ROADMAP.md` |
| Project interchange (EDL/OTIO/FCPXML/AAF) | **not started** (needs the Timeline Engine) |

## Layout

| Module | Purpose |
|--------|---------|
| `probe` | metadata without decoding |
| `decode` | `VideoDecoder`, `AudioDecoder` |
| `cache` | `FrameCache`, `FrameReader` |
| `preset`, `transcode` | `ExportPreset`, `ExportSettings`, `export` |
| `proxy` | `ProxyManager` |
| `hwaccel` | GPU API discovery, GPU decode setup (`HwApi`, `HwPolicy`) |
| `types`, `error` | plain data types and the error enum |

## Tests

```bash
sudo apt install ffmpeg libavcodec-dev libavformat-dev libavutil-dev libswscale-dev \
  libswresample-dev libavfilter-dev libavdevice-dev libx264-dev libx265-dev \
  clang libclang-dev pkg-config
cargo test -p originfx-media-engine
```

Tests build their own clips with the `ffmpeg` command line tool (no binary
fixtures in the repo) and skip themselves if it is missing. The key fixture is a
clip whose frame *N* is a solid colour that encodes *N*, so a test can look at
a decoded frame and know exactly which frame it got. That is how seek accuracy
is verified.
