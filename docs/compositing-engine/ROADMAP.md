# Compositing Engine roadmap

## Done

* Opacity, 11 blend modes, position, scale, rotation, crop, rect and ellipse masks with feather and invert
* All of them keyframe-able (`key`, `unkey`, `clear-keys`) and saved in project file version 6
* Edits: `look`, `blend`, `mask`; undoable and validated; cutting a clip slices the keys
* Renderer path with a bit-exact fast route for clips with nothing set

## Next, in order

1. **Anchor point and per-clip pivot.** Rotate and scale about a chosen point, not only the frame centre.
2. **Mip-mapped scaling.** Big downscales alias today; add a prefilter.
3. **More blend modes.** Colour burn, colour dodge, hue, saturation, colour, luminosity, and so on.
4. **More masks.** Bezier and freehand paths, several masks per clip, mask modes (add, subtract, intersect), mask expansion.
5. **Track mattes.** Use another clip's alpha or luma as the mask.
6. **Adjustment layers.** Effects and looks applied to everything beneath a clip.
7. **Keying composites.** Chroma and luma key (pairs with the Effects roadmap).
8. **Crop with feather, and corner pin / perspective.**
9. **Speed.** Rows in parallel, then a GPU path that must match the CPU result.
10. **Precision.** 16-bit or float layers to avoid banding.
11. **Track and master look keys that follow clip moves** (same open item as audio automation).

## Belongs to other engines

| Item | Engine |
|---|---|
| Text, shapes and animated graphics as layers | Motion Graphics |
| Blend modes or masks from plug-ins | Plugin |
| Transform handles, mask drawing, keyframe lanes | the interface, once it exists |
