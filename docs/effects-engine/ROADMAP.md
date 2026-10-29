# Effects Engine roadmap

## Done

* 14 effects: brightness, contrast, exposure, gamma, saturation, hue, temperature, invert, grayscale, sepia, blur, sharpen,
  vignette, pixelate
* An effect stack on every picture clip, ordered, switchable, with keyframe-able settings
* Edits: add, remove, move, switch on and off, set, key, unkey, clear; undoable and validated
* Command line: `effect kinds|add|remove|move|on|set|key|unkey|clear-keys`; `show` lists effects
* Project file format version 5

## Next, in order

1. **Colour grading proper.** Lift, gamma and gain (shadows, mid tones, highlights) with a colour each, curves,
   white balance by picking a grey, and a working colour space with a defined display transform.
2. **LUTs.** Load `.cube` 3D look-up tables as an effect.
3. **More looks.** Tint, posterise, threshold, noise and film grain, chromatic aberration, glow, halftone, mosaic by
   shape, edge detect, emboss, motion and radial blur.
4. **Keying.** Chroma key (green screen) and luma key. Pairs with the Compositing engine.
5. **Speed.** Use all the cores (rows in parallel), then a GPU path (wgpu) that must match the CPU result.
6. **Effects on a track or on the whole sequence** (adjustment layers), applied to everything beneath.
7. **Presets.** Save a stack and apply it to other clips; copy and paste effects between clips.
8. **Frame blending and optical flow** for slow motion and speed ramps (from the Video Editing roadmap).
9. **Stabilisation and denoise.**
10. **More precision.** Process in 16-bit or float, not 8-bit, to avoid banding in heavy grades (visible on smooth blurs).
11. **Sound effects** (EQ, compressor) are the Audio engine's job and need its stateful path.

## Belongs to other engines

| Item | Engine |
|---|---|
| Keying composites | Compositing (opacity, blend modes, transforms and masks are done: [Compositing Engine](../compositing-engine/README.md)) |
| Text, shapes, animated graphics | Done in the [Motion Graphics Engine](../motion-graphics-engine/README.md) |
| Effects from plug-ins | Done in the [Plugin Engine](../plugin-engine/README.md) (WebAssembly; OFX and shaders are on its roadmap) |
| Effect settings panel, keyframe lanes | the interface, once it exists |
