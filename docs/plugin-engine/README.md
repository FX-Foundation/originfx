# Plugin Engine

Engine 10 of 10. Plugins add new picture effects to OriginFX without changing OriginFX. A plugin is one
WebAssembly file (`.wasm`) in a folder; its effect appears in a clip's effect stack next to the built-in ones, with its own
settings that can be keyframed like any other.

Crate: `crates/plugin-engine` (package `originfx-plugin-engine`, library `plugin_engine`). It loads and checks plugins and
runs them on RGBA pictures. The effect stack and what a project remembers about a plugin effect are in the
[Timeline Engine](../timeline-engine/README.md); the [Render Engine](../render-engine/README.md) runs the plugin when it draws.
Defaults chosen are in [DECISIONS.md](DECISIONS.md).

## Why WebAssembly

A plugin is somebody else's code running inside your editor, so it has to be safe by construction, work the same on every
operating system, and be shareable as a single file.

* **Sandboxed.** A plugin cannot read or write files, use the network, read the clock, or see anything else on the machine.
  It is not allowed to import anything at all; a module that asks for an import is refused when loaded.
* **Capped.** Each call has a work allowance (50 million steps plus 600 per pixel) and a memory cap (512 MiB). A plugin
  that loops forever or asks for too much is stopped with a clear message; it cannot hang the editor or exhaust the machine.
* **Clean.** Every call runs in a fresh instance. Nothing carries from one frame to the next, so renders are repeatable.
  If a plugin fails, the picture is left exactly as it was.
* **Portable.** The same `.wasm` file runs on Windows, macOS and Linux, on any CPU.
* **Language-free.** Anything that compiles to WebAssembly works: Rust, C, C++, Zig, AssemblyScript and others.

## Using plugins

Put `.wasm` files in a folder and tell OriginFX where: `--plugins DIR` on a command, or set `ORIGINFX_PLUGINS`, or use a
folder called `plugins` in the current folder.

```bash
O=originfx
$O plugin example ./plugins                  # writes the small example plugin ("Invert Mix")
$O plugin list --plugins ./plugins           # what is installed, with each plugin's settings
$O plugin info ./plugins/invert-mix.wasm     # check one file
$O timeline P.originfx effect add clip8 plugin:originfx.example.invert-mix --plugins ./plugins   # prints fx1
$O timeline P.originfx effect set clip8 fx1 amount 60
$O timeline P.originfx effect key clip8 fx1 amount 0 0       # keyframes work like any effect
$O render-frame P.originfx --at 2 -o a.png --plugins ./plugins
$O render P.originfx out.mp4 --plugins ./plugins
```

The project remembers the plugin's id and its list of settings (names, ranges, defaults), so a project opens, edits and
validates without the plugin installed. Only rendering needs it. If it is missing the render stops with
`plugin 'id' is not installed` (or skips the effect with `--allow-offline`). If the installed plugin's settings no longer
match the project's, the render says so and asks you to add the effect again. Plugin files that fail to load are reported
and skipped; they do not stop the others.

## Writing a plugin (interface version 1)

A plugin module exports these, and imports nothing:

| Export | Type | What it does |
|---|---|---|
| `memory` | memory | The plugin's linear memory |
| `originfx_manifest_ptr` | `() -> i32` | Address of the manifest in memory |
| `originfx_manifest_len` | `() -> i32` | Its length in bytes (1 to 65536) |
| `originfx_alloc` | `(len: i32) -> i32` | Give the host `len` bytes of memory; return the address, or 0 if there is none |
| `originfx_effect` | `(pixels: i32, width: i32, height: i32, params: i32, nparams: i32) -> i32` | Process the picture in place; return 0 for success, anything else for failure |

The **manifest** is JSON:

```json
{"abi": 1, "id": "com.example.glow", "name": "Glow", "version": "1.0.0", "author": "You",
 "description": "Soft glow around bright areas.",
 "params": [{"name": "radius", "unit": "px", "default": 10, "min": 0, "max": 50},
            {"name": "mix", "unit": "%", "default": 50, "min": 0, "max": 100}]}
```

`abi` must be 1. `id` and setting names use letters, digits, `.`, `-`, `_`. Up to 16 settings; each needs finite
`min <= default <= max`.

When rendering, the host calls `originfx_alloc` for the picture (`width * height * 4` bytes, straight-alpha RGBA, 8 bits
a channel, rows top to bottom) and for the settings (`nparams` little-endian `f64`s, in manifest order, already clamped to
their ranges), copies them in, calls `originfx_effect`, and reads the picture back. A plugin may allocate as it likes
(a bump allocator is enough: the memory is thrown away after the call).

The tested reference plugin is the WebAssembly text in `crates/plugin-engine/src/example.rs`. A plugin in Rust looks like
this (a sketch of the same interface; **not yet built or tested in this repository**, because the tooling for building
`wasm32` targets was not available in the cloud workspace; treat it as a starting point):

```rust
// cargo build --release --target wasm32-unknown-unknown   (crate-type = ["cdylib"])
static MANIFEST: &str = r#"{"abi":1,"id":"com.example.dim","name":"Dim","params":[{"name":"amount","unit":"%","default":50,"min":0,"max":100}]}"#;

#[no_mangle] pub extern "C" fn originfx_manifest_ptr() -> *const u8 { MANIFEST.as_ptr() }
#[no_mangle] pub extern "C" fn originfx_manifest_len() -> usize { MANIFEST.len() }

#[no_mangle]
pub extern "C" fn originfx_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

#[no_mangle]
pub unsafe extern "C" fn originfx_effect(px: *mut u8, w: i32, h: i32, params: *const f64, n: i32) -> i32 {
    if n < 1 { return 1; }
    let keep = 1.0 - *params / 100.0;
    for i in 0..(w as usize * h as usize) {
        for c in 0..3 { let p = px.add(i * 4 + c); *p = (*p as f64 * keep) as u8; }
    }
    0
}
```

## Errors you may see

| Message | Meaning |
|---|---|
| `plugin interface: plugins may not import anything` | The module asks for something from outside the sandbox |
| `plugin manifest: ...` | The manifest is missing, not valid JSON, or breaks a rule (the message says which) |
| `plugin used too much processing time and was stopped` | It ran past its work allowance |
| `plugin used too much memory and was stopped` | It asked for more than the memory cap |
| `plugin crashed: ...` | A trap (out-of-bounds access, `unreachable`, and so on) |
| `plugin could not process the picture (code N)` | `originfx_effect` returned N |

## In the project file

Format version 8. A plugin effect is saved with `kind: "plugin"` and a `plugin` block (id, name, version and the settings'
ranges); a project with only built-in effects writes nothing extra, and files from versions 1 to 7 open unchanged.

## Known limits

See [ROADMAP.md](ROADMAP.md). The main ones: only picture effects (no sound effects, generators, transitions or
importers yet), CPU only (the plugin gets pixels, not a GPU), an interpreter (about 50 ms to run a simple effect over a
720p frame in a release build, slower for heavy effects), and one picture in, one picture out.
