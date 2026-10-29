# Decisions made while building the Plugin Engine

Defaults chosen while building this engine, each with the way to reverse it. Earlier engines:
[Timeline T](../timeline-engine/DECISIONS.md), [Render R](../render-engine/DECISIONS.md),
[Video Editing V](../video-editing/DECISIONS.md), [Audio A](../audio-engine/DECISIONS.md),
[Keyframe K](../keyframe-engine/DECISIONS.md), [Effects E](../effects-engine/DECISIONS.md),
[Compositing C](../compositing-engine/DECISIONS.md), [Motion Graphics G](../motion-graphics-engine/DECISIONS.md).

| # | Decision | Why | Reverse it by |
|---|----------|-----|---------------|
| P1 | Plugins are WebAssembly modules run by `wasmi`, a pure Rust interpreter | Sandboxed by design, identical on every OS and CPU, one file to share, no native-code loading (no `dlopen`, no `unsafe` plugin ABI), any language | Swap the runtime in `plugin.rs` (for example `wasmtime`, which compiles to machine code and is faster but a much larger dependency) |
| P2 | A plugin may import nothing; modules with imports are refused | The sandbox gives no files, network, clock or randomness; there is nothing to leak and nothing to break repeatability | `Plugin::load_with` |
| P3 | Every call runs in a new instance, with a fuel (step) allowance of 50 million plus 600 per pixel and a 512 MiB memory cap | A runaway plugin ends in milliseconds to seconds; no state leaks between frames; renders are repeatable. Costs a little time per call (instantiation) | `Limits` |
| P4 | The plugin interface is five exports and a JSON manifest, versioned (`abi: 1`) | Small enough to write by hand in any language; the version lets later changes be refused cleanly rather than misread | `manifest.rs`, `ABI_VERSION` |
| P5 | The picture is passed as straight-alpha RGBA8 at the sequence size, settings as `f64` in manifest order, clamped by the host | The same picture format the other engines use; the plugin never sees an out-of-range number | `Plugin::apply` |
| P6 | If a plugin fails, the picture is left unchanged and the render stops with the plugin's error | Never silently draw a wrong frame | `Plugin::apply`, `draw_clip` in `renderer.rs` |
| P7 | A plugin effect lives in the same stack as built-in effects, with the same edits, keyframes, cutting and undo | One set of tools; the Timeline Engine needs no second concept | `Effect::plugin` in `effects.rs` |
| P8 | The project stores a copy of the plugin's id and settings (names, ranges, defaults), not the plugin | The project opens, edits, validates and saves without the plugin installed; only rendering needs it | `PluginSpec` |
| P9 | A plugin is found by its id in a folder (`--plugins`, `ORIGINFX_PLUGINS` or `./plugins`); the first file by name wins on a duplicate id | Nothing is installed anywhere on the machine unless you put it there; deterministic | `Host::scan`, `plugin.rs` in the CLI |
| P10 | A missing plugin stops the render, unless `--allow-offline`, which skips it | Same rule as missing media (R-decisions) | `renderer.rs` |
| P11 | If the installed plugin has a different number of settings than the project's copy, the render stops and says so | Values would otherwise be matched to the wrong settings | `renderer.rs` |
| P12 | Up to 16 settings per plugin, 64 KiB manifest, 8192 x 4608 pixels at most | Bounds that keep a bad plugin from making a bad project file or a huge call | `manifest.rs`, `Limits` |
| P13 | Plugin effects are always treated as live (never skipped as neutral) | A plugin cannot say which settings change nothing; skipping would be a guess | `Effect::is_live` |
| P14 | A Rust plugin sample is in the docs but not built here | No `wasm32` target was available offline in the cloud workspace; the tested reference is hand-written WebAssembly text | See [ROADMAP.md](ROADMAP.md) |
| P15 | Project file format version 8 | Old projects keep working | `FORMAT_VERSION` |
