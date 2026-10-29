# Plugin Engine roadmap

## Done

* Sandboxed WebAssembly plugins: no imports, work and memory caps, fresh instance per call, picture untouched on failure
* A manifest (id, name, settings with ranges) and a five-function interface, versioned
* Plugin effects in the clip effect stack: add, remove, move, enable, set, keyframe, cut, undo, save
* `plugin list|info|example`; `effect add plugin:<id>`; `--plugins DIR` on `render` and `render-frame`
* Project file format version 8

## Next, in order

1. **Build a real Rust plugin in CI** (a `wasm32` target), publish a small plugin SDK crate and a template repository, and
   test the Rust sample in the docs.
2. **Speed.** Offer the `wasmtime` runtime (compiled code) behind a feature, keep `wasmi` as the portable default; give plugins
   SIMD; process rows in parallel with several instances.
3. **More plugin kinds.** Generators (graphics from numbers, for the Motion Graphics engine), transitions, audio effects
   (for the Audio engine), blend modes and importers or exporters.
4. **Frame context.** Pass time, frame number, frame rate and the previous frame so a plugin can do temporal effects (echo,
   motion blur, denoise); pass a second picture for transitions and keying.
5. **Higher precision.** 16-bit and float pictures in and out.
6. **Plugin folders per project and per user**, plugin versions and updates, and a "plugin changed" migration that keeps
   settings by name instead of asking to re-add the effect.
7. **Signing and a registry.** Signed plugins, a community index, and a permission model if capabilities (such as reading a
   LUT file the user picks) are ever added: always opt in per plugin, never ambient.
8. **OFX.** Host standard OFX plugins (native) in a separate, clearly marked, out-of-process mode; this cannot be sandboxed
   the same way.
9. **GPU plugins** (shader modules) that must match the CPU result.
10. **Settings of other types** (colours, choices, on/off, text, curves) so a settings panel can be built from the manifest.

## Belongs to other engines

| Item | Engine |
|---|---|
| Plugin manager, settings panel generated from the manifest, plugin browser | the interface, once it exists |
| Plugin-provided codecs and formats | Media |
