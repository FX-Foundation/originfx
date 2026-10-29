# Decisions made while building the Render Engine

Defaults chosen while building this engine, each with the way to reverse it. Decisions that apply to the whole project are in
[../DECISIONS.md](../DECISIONS.md). Changes made to the Media Engine for this engine are in
[../media-engine/DECISIONS.md](../media-engine/DECISIONS.md) (M20 to M24).

| # | Decision | Why | Reverse it by |
|---|----------|-----|---------------|
| R1 | CPU renderer first; GPU later behind the same interface | The CPU version can be tested exactly in the cloud and becomes the reference the GPU one is checked against | Add a second `Renderer` backend |
| R2 | Pictures are RGBA8 with straight alpha | It is what the Media Engine decodes to; straight alpha is what files carry | `blend_over` and `resize` in `composite.rs` and `scale.rs` |
| R3 | Resizing uses a triangle filter that widens when shrinking, weighted by alpha | Downscaling that skips pixels shimmers; unweighted colour makes dark fringes around transparency | `resize` in `scale.rs` |
| R4 | Default fit is `Contain` (all of the picture, bars if the shape differs) | Never crops or distorts a clip without being asked | `RenderSettings::fit` |
| R5 | Pixel aspect ratio is ignored | Needs per-clip metadata the model does not have yet | See [ROADMAP.md](ROADMAP.md) item 7 |
| R6 | Background is opaque black | What every player shows for nothing | `RenderSettings::background` |
| R7 | Missing media stops an export; previews can show magenta ("media offline") and silence | A final file must never silently contain placeholders | `MissingMedia` in `renderer.rs` |
| R8 | Sound is mixed as a plain sum, then clipped at full scale | Predictable, and the Audio Engine owns real mixing (gain, pan, limiting) | `Renderer::audio_samples` |
| R9 | Audio is rendered per frame from absolute sample positions (`round(frame_time x rate)`) | Rounding never accumulates, so picture and sound end together at any frame rate, including 29.97 | `feed` in `export.rs` |
| R10 | Clip edges for sound are rounded to the nearest sample | Adjacent clips then join exactly with no gap or overlap | `audio_samples` in `renderer.rs` |
| R11 | Export reuses the Media Engine's exporter by handing it frames, not a second encoder | All 19 presets, GPU encode, colour tagging and progress work for free and cannot drift apart | `export.rs` |
| R12 | On Unix, frames stream to the exporter through an in-memory pipe; elsewhere through a lossless working file | Streaming doubled speed on the test machine and needs no disk; a lossless file of a long 1080p project would be tens of GB | `ExportOptions::keep_working_file` forces the file route |
| R13 | Sound is always written for presets that have it, silent if the timeline has none | Deliverables expect an audio track | `Plan::want_audio` in `export.rs` |
| R14 | Audio-only presets render no pictures (a 16 x 16 black frame stands in) | Compositing frames nobody will see wastes most of the time | `Plan` in `export.rs` |
| R15 | A render works on a snapshot of the project | Editing while exporting cannot change the result halfway | `Renderer::new` clones the project (cheap, shared data) |
| R16 | Still images get one resized copy, reused every frame | Resizing a 4K photo 1,440 times for a minute of video is pure waste | `Renderer::stills` |
| R17 | Only the first video and audio stream of a file can be used so far | The timeline model allows more; the decoder API opens the best stream | `FileSource` in `source.rs` |
| R18 | When the encoder stops early the writer fails with the encoder's error, and when rendering fails the encoder is stopped and the half-written output removed | Neither side can hang the other, and the real cause is reported | `export_streaming` in `export.rs` |
