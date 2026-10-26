# Decisions made while building the Media Engine overnight

You were away, so these were made with the sensible default. Every one can be
reversed; none touched your existing files.

| # | Decision | Why | Reverse it by |
|---|----------|-----|---------------|
| 1 | Work on branch `claude/media-engine`, not `main` | Your `main` stays untouched until you review | Merge the branch, or delete it |
| 2 | Cargo workspace at the repo root, crates under `crates/` | Room for the other 9 engines as sibling crates | Move folders |
| 3 | FFmpeg 6.1 via the `ffmpeg-next 6.1` crate | Matches Ubuntu 24.04's FFmpeg; the newest crate versions target FFmpeg 7+ | Bump both together |
| 4 | Decode output is RGBA8 in system memory | Simplest correct contract for now; GPU/YUV paths come with the Render Engine | See ROADMAP |
| 5 | Export is always constant frame rate | What editors and delivery formats expect | Remove the `fps` filter |
| 6 | Cinema RAW removed from engine scope | Your call: future plugin | See ROADMAP |
| 7 | The 19 export presets are my own list | Your notes say "19 export formats" but not which; this covers the common web, editing, and audio formats | Edit `preset.rs` (one table) |
| 8 | No licence declared (`publish = false`) | Choosing a licence is yours, and FFmpeg+x264 affects it | Add LICENSE, remove `publish = false` |
| 9 | Hardware acceleration is probe-only (superseded: GPU decode added once a GPU was available, see 12-15) | No GPU in the build environment; untested GPU code is worse than none | See ROADMAP |
| 10 | Did not touch the root `README.md`, `CONTRIBUTING.md` or `CODE_OF_CONDUCT.md` | They are your voice | n/a |
| 11 | A stray test branch `claude/push-test` exists on GitHub (one empty commit) | I could not delete it from here | Delete it in GitHub's Branches page |
| 12 | GPU decode API is `DecodeOptions { hw: HwPolicy::{Off, Auto, Require(HwApi)} }` | `hwaccel::HwBackend` already names the probe result, so the API enum is `HwApi { Cuda, Vaapi }` | Rename in `hwaccel.rs` |
| 13 | GPU frames are repacked from NV12/P010 to planar `yuv420p`/`yuv420p10le` before RGBA conversion | swscale converts NV12 along a different path, so sharp colour edges differed from software by up to ~190 levels; repacking makes GPU and software frames byte-identical | `hwaccel::download` |
| 14 | Only 8/10-bit 4:2:0 streams go to the GPU | Intel VA-API accepted RGB VP9 (profile 1) and returned wrong pixels; 4:2:0 covers almost all camera and web video | `open_video_decoder` in `decode.rs` |
| 15 | GPU decoders run with one CPU thread | Frame threads only wait on the GPU; on the test laptop they cost more CPU than software decoding (4K HEVC: 16 fps with them, 27 without) | `hardware_decoder` in `decode.rs` |
| 16 | After a seek only the returned frame is copied back from the GPU | Copying every skipped frame made GPU seeking slower than software (H.264: 177 ms vs 42 ms per jump) | `bring_to_memory` in `decode.rs` |
| 17 | NVENC is opt-in per export (`ExportSettings::hw_encode`, default off) and covers only the H.264/H.265 presets | Software output stays exactly as before; NVENC is what this GPU can test (no AV1 encode on RTX 30) | `nvenc_encoder` in `transcode.rs` |
| 18 | If NVENC fails part-way with `Auto`, the whole export restarts in software | Packets already muxed cannot be re-encoded mid-file; a restart is simple and always correct. Progress starts again from 0 | `export_with_progress` |
| 19 | NVENC quality: VBR with constant quality (`cq`), presets p2/p5/p7, `tune=hq`, High profile for H.264, `maxrate` 400M | Mirrors CRF in x264/x265. Values (H.264 31/26/21, H.265 32/27/22) put Standard at about software's quality. Without a `maxrate` NVENC caps itself near 21 Mbit/s, which held 4K quality down | `video_options` in `transcode.rs` |
