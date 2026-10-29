# Keyframe Engine roadmap

## Done

* Curves with hold, linear and smooth keys; exact values, integrals and cuts
* Animated clip gain, pan and speed; animated track gain and pan; animated master gain
* Speed ramps for picture and sound, forwards and reversed
* Edits: set a key, remove a key, clear a value; undoable and validated
* Command line: `key`, `unkey`, `clear-keys`; `show` lists animated values
* Project file format version 4

## Next, in order

1. **More edits on keys.** Move a key in time, change only its value or shape, copy keys between clips, scale or offset
   a selection. Today you set a key again at the same time, or remove and set.
2. **Handles.** Edit the slopes of smooth keys (the data is already stored).
3. **More shapes.** Ease in, ease out and bezier segments, as new `Interp` names.
4. **Ripple-aware track and master keys.** Let them follow a ripple delete or insert, as an option.
5. **Frame blending and optical flow for speed ramps.** Slow ramps repeat frames today. That belongs with the Effects
   engine.
6. **Pitch-preserving sound for ramps.** Pitch follows speed today, like a tape. It needs the Audio engine's stretching.
7. **Automatic ducking** (music under voice), made from the same curves, once the Audio engine has an analysis stage.
8. **Faster evaluation.** Cache the segment last used while mixing, or evaluate every few samples, if profiling asks for it.
9. **Expressions and links between values** (one value driving another), if Motion Graphics wants them.

## Belongs to other engines

| Item | Engine |
|---|---|
| Animated position, scale, rotation and opacity | Done in the [Compositing Engine](../compositing-engine/README.md) |
| Animated effect settings | Done: [Effects Engine](../effects-engine/README.md) |
| Animated text and shape properties | Motion Graphics (position, scale, rotation, opacity work today; the roadmap lists the rest) |
| Curve editor, timeline lanes for keys | the interface, once it exists |
| Keys on plug-in settings | Plugin |
