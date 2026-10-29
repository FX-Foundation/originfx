# Motion Graphics Engine roadmap

## Done

* Solid colours, gradients, rect (rounded), ellipse, polygon and star shapes with fill, gradient fill and stroke
* Text with a font file, size, colour, alignment, line gap, tracking, outline and drop shadow
* Graphics as project media: placeable, trimmable, keyframe-able through the clip's look, shared and editable
* Command line: `graphic add|set` (solid, gradient, shape, text); `show` lists graphics; `new --video-tracks`
* Project file format version 7

## Next, in order

1. **Animate a graphic's own values.** Colour, shape size, corner radius, stroke width, text size and a text *reveal*
   (typewriter, per word and per letter) as keyframe-able values. Needs per-clip overrides on top of the shared graphic.
2. **Anchor point and offsets.** Place and rotate a graphic about its own corner or edge, not the frame centre (shares
   work with the Compositing roadmap).
3. **Text boxes.** A box size with word wrap, vertical alignment, auto-fit, and rich text (bold and colours inside a line).
4. **Bundled fonts and a font picker** (the project owner's choice of fonts and licences), font fallback for missing
   characters, and complex scripts (shaping for Arabic, Indic and others).
5. **Paths.** Lines, arcs, bezier shapes and SVG import; trim paths (draw-on); rounded-polygon corners.
6. **Soft shadows and glows** for shapes and text, shadows that follow shape.
7. **Templates.** Lower thirds, title cards and callouts saved as a group of graphics and clips with named fields.
8. **Shape layers with repeaters, and particles.**
9. **Number and data graphics.** Counters, charts and bars driven by values or data files.
10. **Speed.** Rows in parallel, then a GPU path that must match the CPU result; 16-bit or float for smooth gradients
    (8-bit gradients can band).
11. **Subtitles as graphics.** Burn subtitle tracks in with a chosen style (from the Media Engine's subtitle support).

## Belongs to other engines

| Item | Engine |
|---|---|
| Effects on graphics (blur, glow of the finished layer) | Effects (already works, since a graphic is a picture clip) |
| Blend modes, masks, transforms | Compositing (already works) |
| Graphics from plug-ins (custom generators, OFX) | Plugin |
| Text editing on screen, font picker, shape handles | the interface, once it exists |
