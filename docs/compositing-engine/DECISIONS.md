# Decisions made while building the Compositing Engine

Defaults chosen while building this engine, each with the way to reverse it. Earlier engines:
[Timeline T](../timeline-engine/DECISIONS.md), [Render R](../render-engine/DECISIONS.md),
[Video Editing V](../video-editing/DECISIONS.md), [Audio A](../audio-engine/DECISIONS.md),
[Keyframe K](../keyframe-engine/DECISIONS.md), [Effects E](../effects-engine/DECISIONS.md).

| # | Decision | Why | Reverse it by |
|---|----------|-----|---------------|
| C1 | The pixel maths is a separate crate with no dependencies; the timeline uses it only for the blend and mask names | Testable without a timeline, and movable to the GPU later | Move the enums into the timeline crate |
| C2 | `Normal` at full opacity uses the same integer "over" as the renderer's plain drawing; the other modes use the W3C compositing formulas in floats | Old projects stay bit-exact; the other modes match what other editors and browsers give | `blend.rs` |
| C3 | Eleven blend modes: normal, multiply, screen, overlay, darken, lighten, hard-light, soft-light, difference, exclusion, add | The ones people use daily; the rest (colour, hue, burn, dodge) need more colour maths | `BlendMode::ALL` |
| C4 | Opacity multiplies the layer's own alpha | Same as every editor; a half-transparent clip at 50 % is a quarter solid | `composite` |
| C5 | Position is a percent of the frame size, scale a percent, rotation clockwise degrees, all about the frame centre | Resolution-independent: the same project at preview and export size looks the same | `Affine` in `geometry.rs` |
| C6 | The transform is an inverse-mapped bilinear sample on premultiplied pixels; identity returns an exact copy | No holes, no dark fringes on soft edges, and no change when nothing is set | `transform` |
| C7 | Crop is a percent of the picture, hard edged | Simple, predictable; use a mask when you want a soft edge | `crop` |
| C8 | Masks are a rect or an ellipse with an optional invert and a feather (smoothstep, centred on the edge) | Covers most uses; freehand and bezier masks are a roadmap item | `MaskShape` |
| C9 | Order: effects, crop, mask, transform, composite | A blur or grade follows the picture; a mask cuts the finished picture; the transform moves the result | `draw_clip` in `renderer.rs` |
| C10 | Look settings are on picture clips only, and a sound clip with any is invalid | Keeps the rules simple and files honest | `Sequence::validate` |
| C11 | Look keys do not follow links (a speed key does) | The linked sound has no look | `place_of` in `ops.rs` |
| C12 | A mask's keys are dropped when the mask is removed, and mask values refuse to be set or keyed without a mask | No hidden animation left behind | `remove_mask` |
| C13 | A look value is either fixed or animated, never both | Same rule as the other animated values (K2, K10, E11) | `ops.rs` |
| C14 | Project file format version 6; look fields are written only when not default | Old projects keep working, new files stay small | `FORMAT_VERSION` |
| C15 | Compositing runs on the CPU, one thread | Correct and identical everywhere first | See [ROADMAP.md](ROADMAP.md) |
