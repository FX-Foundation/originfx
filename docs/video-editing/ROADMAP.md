# Video Editing Engine roadmap

## Done

* Speed from 1% to 10,000% as an exact fraction, with "keep the media" and "keep the length" modes, ripple, and linked sound
* Reverse, combined with speed, understood by every edit (split, trim, roll, slide, overwrite)
* Transitions: dissolve, dip to black and white, wipes and pushes in four directions, with three alignments around the cut
* Audio crossfade (equal power) on linked sound
* Spare-media ("handle") checks, with transitions dropped by edits that make them impossible
* Project file version 2 (version 1 still loads)
* Command line: `speed`, `reverse`, `transition`

## Next, in order

1. **Freeze frame and still-hold.** Hold one frame for any length (speed zero, effectively). Needs a clip mode, not just a
   speed, because speed zero cannot be a fraction.
2. **Frame blending and optical flow for slow motion.** Today slow motion repeats frames, which stutters at 25% and below.
   Blending is cheap; optical-flow interpolation is an Effects engine job.
3. ~~**Speed ramps.**~~ Done in the [Keyframe Engine](../keyframe-engine/README.md): the clip model maps time through the
   integral of a speed curve. Frame blending (item 2) matters more now, since ramps slow down smoothly.
4. **Fade in and fade out at the head and tail of a clip.** A transition with only one clip (from or to black or nothing).
   Today a dip needs a neighbour.
5. **Transitions between clips on different tracks** (an overlap, as in a cross dissolve done with layers) and across a gap.
6. **More looks**: iris, clock wipe, slide, zoom, blur dissolve, and custom transitions from the Plugin engine. The blend
   function in `crates/render-engine/src/transition.rs` is the single place to add them.
7. **Apply a transition to many cuts at once** (a "dissolve at every cut" command), and default transitions.
8. **Pitch-preserving time stretch** for sound. This belongs to the Audio engine; the hook is `Renderer::clip_audio`.
9. **Anti-aliasing when sound is sped up**, same owner.
10. **Faster reverse.** Decode a stretch forwards once, keep the frames, and play them backwards, instead of seeking per frame.
    Belongs with the Render Engine's frame cache.

## Belongs to other engines

| Item | Engine |
|---|---|
| Animated transition curves | Keyframe (speed ramps are done) |
| Optical-flow slow motion, blurs, glows | Effects |
| Opacity, blend modes, transforms | Done in the [Compositing Engine](../compositing-engine/README.md) |
| Pitch shifting, sound effects | Audio (see its [roadmap](../audio-engine/ROADMAP.md)) |
| Custom transitions | Plugin |
