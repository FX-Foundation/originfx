# Decisions made while building the Media Engine

Defaults chosen while building this engine, each with the way to reverse it. Where the owner made the call, the row says so.
Decisions that apply to the whole project are in [../DECISIONS.md](../DECISIONS.md).

| # | Decision | Why | Reverse it by |
|---|----------|-----|---------------|
| M1 | FFmpeg 6.1 via the `ffmpeg-next 6.1` crate | Matches Ubuntu 24.04's FFmpeg; the newest crate versions target FFmpeg 7+ | Bump both together |
| M2 | Decode output is RGBA8 in system memory | Simplest correct contract for now; GPU/YUV paths come with the Render Engine | See [ROADMAP.md](ROADMAP.md) |
| M3 | Export is always constant frame rate | What editors and delivery formats expect | Remove the `fps` filter |
| M4 | Cinema RAW removed from engine scope | The owner's call: future plugin | See [ROADMAP.md](ROADMAP.md) |
| M5 | The 19 export presets (list in `preset.rs`) | Started as my own list; **confirmed by the owner on 26 Oct 2026**. Covers common web, editing and audio formats; every preset is tested | Edit `preset.rs` (one table) |
| M6 | Hardware acceleration is probe-only (superseded: GPU decode added once a GPU was available, see M7 to M10) | No GPU in the build environment; untested GPU code is worse than none | See [ROADMAP.md](ROADMAP.md) |
| M7 | GPU decode API is `DecodeOptions { hw: HwPolicy::{Off, Auto, Require(HwApi)} }` | `hwaccel::HwBackend` already names the probe result, so the API enum is `HwApi { Cuda, Vaapi }` | Rename in `hwaccel.rs` |
| M8 | GPU frames are repacked from NV12/P010 to planar `yuv420p`/`yuv420p10le` before RGBA conversion | swscale converts NV12 along a different path, so sharp colour edges differed from software by up to ~190 levels; repacking makes GPU and software frames byte-identical | `hwaccel::download` |
| M9 | Only 8/10-bit 4:2:0 streams go to the GPU | Intel VA-API accepted RGB VP9 (profile 1) and returned wrong pixels; 4:2:0 covers almost all camera and web video | `open_video_decoder` in `decode.rs` |
| M10 | GPU decoders run with one CPU thread | Frame threads only wait on the GPU; on the test laptop they cost more CPU than software decoding (4K HEVC: 16 fps with them, 27 without) | `hardware_decoder` in `decode.rs` |
| M11 | After a seek only the returned frame is copied back from the GPU | Copying every skipped frame made GPU seeking slower than software (H.264: 177 ms vs 42 ms per jump) | `bring_to_memory` in `decode.rs` |
| M12 | NVENC is opt-in per export (`ExportSettings::hw_encode`, default off) and covers only the H.264/H.265 presets | Software output stays exactly as before; NVENC is what this GPU can test (no AV1 encode on RTX 30) | `nvenc_encoder` in `transcode.rs` |
| M13 | If NVENC fails part-way with `Auto`, the whole export restarts in software | Packets already muxed cannot be re-encoded mid-file; a restart is simple and always correct. Progress starts again from 0 | `export_with_progress` |
| M14 | NVENC quality: VBR with constant quality (`cq`), presets p2/p5/p7, `tune=hq`, High profile for H.264, `maxrate` 400M | Mirrors CRF in x264/x265. Values (H.264 31/26/21, H.265 32/27/22) put Standard at about software's quality. Without a `maxrate` NVENC caps itself near 21 Mbit/s, which held 4K quality down | `video_options` in `transcode.rs` |
| M15 | Subtitles and chapters are opt-in on export (`ExportSettings::subtitles` / `chapters`, default off) | Existing exports must not change; a pro tool should not silently add tracks | `ExportSettings::new` |
| M16 | Subtitles are carried as plain text cues, written as SRT in MKV and `mov_text` in MP4/MOV; WebM is skipped with a note | Text round-trips through every format without a subtitle encoder, and matches what editors need first. Styling is lost; WebM needs WebVTT, which I could not verify against real players | `prepare_subtitles` in `transcode.rs` |
| M17 | While seeking, software decoding skips unreferenced frames until 1 s before the target, decided per packet | The decoder reads ahead of the frames it outputs, so a per-frame check skipped frames near the target (found by a test with real B-frames). 1 s covers reordering with room to spare | `start_catching_up` in `decode.rs` |
| M18 | Backward scrubbing prefetches up to 16 frames, never more than a third of the cache | Backwards is the slow direction; more than a third of the cache would evict the frames about to be used | `scrub_prefetch` in `cache.rs` |
| M19 | If a seek to time zero still lands late or finds nothing, the file is reopened from its first packet | MPEG-TS with B-frames lists no keyframe at or before the start, so the seek jumped a whole GOP ahead or to the end | `rewind` in `decode.rs` |
| M20 | The Media Engine can receive frames and sound from outside (`WorkingFileWriter`): to a lossless NUT file (Ut Video or FFV1 picture, 32-bit float PCM sound) or, on Unix, to an in-memory pipe of uncompressed frames | A renderer makes frames that exist in no file, and the exporter only read files. Feeding the exporter keeps one encoder for every preset | `working.rs` |
| M21 | The working file is NUT, not Matroska | Matroska stores times in whole milliseconds, which moved sound by up to 24 samples when seeking (found by a test); NUT keeps exact time bases | `format::output_as` in `working.rs` |
| M22 | `export_pipe_with_progress` cannot trim, and a GPU encode that fails part-way is an error, not a restart in software | A pipe can be read only once | `export_inner` in `transcode.rs` |
| M23 | A pipe is told the frame rate explicitly | NUT over a pipe does not reliably carry it; without this the exporter guessed 30 fps and duplicated frames (found by a test) | `export_pipe_with_progress` in `transcode.rs` |
| M24 | After an audio seek the first block is trimmed by rounding, not truncating, the number of samples | 17.9999999 samples was truncated to 17, so every seek could land one sample early (found by a test) | `AudioDecoder::next_buffer` in `decode.rs` |
