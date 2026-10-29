# Render Engine roadmap

## Done

* Frame compositing: layers, alpha, fit modes, high-quality resizing, stills
* Audio mixing with mute, solo, clipping and sample-exact joins
* Export of whole sequences and in/out ranges to all presets, streaming without temp files
* Progress, cancel, missing media handling
* `originfx render` and `originfx render-frame`
* A pluggable media source

## Next, in order

1. **Speed.** On a 2-core test machine a 1080p H.264 export runs at about 14 frames per second. Known costs, in
   order: scaling a layer (about 18 ms for 640x360 to 1080p), filling the background, copying the frame into the pipe,
   and the Media Engine's RGBA to YUV conversion. Ideas: skip the background fill when a layer covers it,
   reuse buffers between frames, SIMD blending and resizing, and hand over YUV so no conversion is needed.
2. **Frame cache and background render.** Remember rendered frames (keyed by the clips and time that made them) so scrubbing
   and re-exporting after a small edit only re-render what changed.
3. **GPU renderer (wgpu).** Same `Renderer` interface, checked against this CPU renderer. Needs GPU frames from the Media Engine
   (zero-copy hand-off) to be worth it.
4. **Colour management.** Convert between colour spaces, HDR tone mapping, wide gamut. The Media Engine's decoder should hand over
   native YUV plus colour metadata first.
5. **Proxy rendering.** Render from proxies for preview and from originals for export, using the Media Engine's proxy manager.
6. **Windows streaming.** The in-memory pipe uses Unix descriptors; Windows currently uses the working file route.
7. **Pixel aspect ratio and field order.** Non-square pixels and interlaced footage are treated as square and progressive.

## Belongs to other engines

| Item | Engine |
|---|---|
| Opacity, blend modes, transforms, masks | Compositing |
| Sound effects (EQ, compression), stateful processing | Audio (see its [roadmap](../audio-engine/ROADMAP.md)); levels, pan, fades and limiting are done |
| Animated position, scale and opacity | Compositing, using the [Keyframe Engine](../keyframe-engine/README.md) curves (gain, pan and speed are done) |
| Blur and basic colour effects are done ([Effects Engine](../effects-engine/README.md)); full colour grading, LUTs, keying | Effects (roadmap) |
| Text, shapes, animation | Motion Graphics |
