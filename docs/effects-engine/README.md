# Effects Engine

Engine 7 of 10. Picture effects on clips: colour, blur, sharpen, vignette and pixelate, stacked in any order,
with settings that can change over time.

Crate: `crates/effects-engine` (package `originfx-effects-engine`, library `effects_engine`). It is pure CPU code with
no dependencies: one function, `apply(kind, values, rgba, w, h)`, changes a picture in place. It knows nothing of
clips or files. The effect stack of a clip, its settings and its keyframes belong to the
[Timeline Engine](../timeline-engine/README.md); the [Render Engine](../render-engine/README.md) applies the stack
when it draws. Keyframes come from the [Keyframe Engine](../keyframe-engine/README.md). See [DECISIONS.md](DECISIONS.md) E1.

## The effects

Sizes are a **percent of the picture's height**, so an effect looks the same in a small preview and in the full-size
export.

| Effect | Settings (range, default) | What it does |
|---|---|---|
| `brightness` | amount (-100 to 100 %, 0) | Adds or removes light, as a share of full scale |
| `contrast` | amount (-100 to 100 %, 0) | Pushes colours away from mid grey (or towards it); -100 is flat grey |
| `exposure` | stops (-5 to 5, 0) | Doubles or halves the light per stop, in linear light, like a camera |
| `gamma` | gamma (0.1 to 5, 1) | Lifts (above 1) or deepens (below 1) the mid tones; black and white stay |
| `saturation` | amount (0 to 300 %, 100) | 0 is black and white, 100 is as shot, more is richer |
| `hue` | degrees (-180 to 180, 0) | Turns every colour round the colour wheel; greys do not move |
| `temperature` | amount (-100 to 100 %, 0) | Warmer (more red, less blue) or cooler |
| `invert` | amount (0 to 100 %, 100) | Photographic negative |
| `grayscale` | amount (0 to 100 %, 100) | Black and white, by how the eye weighs colours |
| `sepia` | amount (0 to 100 %, 100) | Old-photo brown |
| `blur` | radius (0 to 20 %, 1) | Gaussian blur |
| `sharpen` | amount (0 to 500 %, 100), radius (0 to 5 %, 0.3) | Unsharp mask: adds back the difference from a blur |
| `vignette` | amount (0 to 100 %, 50), softness (0 to 100 %, 50) | Darkens towards the corners |
| `pixelate` | size (0 to 30 %, 2) | Mosaic blocks, each the average of the picture beneath |

Every effect has a **neutral** setting that changes no bits at all (brightness 0, saturation 100, blur 0, and so on).
The ones that look like something by default (invert, grayscale, sepia, blur, sharpen, vignette, pixelate) start
visible when you add them; the others start neutral and wait for you to move them.

Colour effects never touch transparency. Blur, sharpen and pixelate work on colour and transparency together, so the
soft edge of a shape does not get a dark or bright halo.

## Using them

```rust
use timeline_engine::*;

let fx = ed.add_effect(seq, clip, EffectKind::Blur, None)?;          // at the end of the stack
ed.set_effect_value(seq, clip, fx, "radius", 2.5)?;                  // a fixed value
ed.key_effect(seq, clip, fx, "radius", Time::from_secs_f64(4.0), 0.0, Interp::Smooth)?;   // ... or keyframes
ed.move_effect(seq, clip, fx, 0)?;                                   // 0 is applied first
ed.set_effect_enabled(seq, clip, fx, false)?;
ed.remove_effect(seq, clip, fx)?;
```

* The stack is applied **in order**: brighten then invert is not the same as invert then brighten.
* Effects live on **picture clips**. A sound clip is refused. A clip can have up to 32.
* An effect's id (`fx1`, `fx2`...) is unique within its clip. When a clip is cut, both halves keep the same effects.
* A setting with keyframes is **animated**: the curve replaces the fixed value, and setting a fixed value is refused
  until you clear the keys. The first key on a setting starts from its fixed value at the start of the clip, so one
  key never changes what comes before it. Keys sit inside the clip and travel with it.
* Cutting a clip cuts the curves exactly, and trimming keeps them where they are, as for any other keyframes.
* Everything is an ordinary edit: undoable, validated, all-or-nothing, saved in the project.

The renderer draws each clip on its own transparent canvas, applies its effects, then puts the result over the
layers beneath. An effect on a top clip never touches the clips below it, and effects work inside transitions too
(each side of the transition gets its own effects).

## Command line

```bash
originfx timeline p.originfx effect kinds                       # every effect and its settings
originfx timeline p.originfx effect add clip8 saturation        # prints fx1
originfx timeline p.originfx effect set clip8 fx1 amount 150
originfx timeline p.originfx effect add clip8 blur --at 0       # first in the stack
originfx timeline p.originfx effect key clip8 fx2 radius 0 0    # blur 0 at 0 s ...
originfx timeline p.originfx effect key clip8 fx2 radius 4 6 --interp smooth   # ... to 6 at 4 s
originfx timeline p.originfx effect unkey clip8 fx2 radius 4
originfx timeline p.originfx effect clear-keys clip8 fx2 radius
originfx timeline p.originfx effect move clip8 fx2 1
originfx timeline p.originfx effect on clip8 fx2 --off
originfx timeline p.originfx effect remove clip8 fx2
originfx timeline p.originfx show                               # lists each clip's effects
originfx render-frame p.originfx --at 2 -o look.png             # check a look on one frame
```

## Project files

Format version 5. Older files load unchanged. An effect is written with its kind, whether it is on, every setting by
name, and any keyframes by setting name. A project with no effects writes none of it. Unknown effects or settings,
values out of range and keys outside their clip are refused on load.

## Speed

Colour effects are table lookups or one small matrix per pixel. Blur uses three box passes, so its cost does not
depend on the radius. A 1080p frame with a saturation, a vignette and a heavy blur (65 px) rendered in about 0.65 s on
the 2-core test machine; the effects are single threaded, and a GPU path is on the [roadmap](ROADMAP.md).

## How it is tested

* Effects crate: every effect at its neutral setting changes no bits; colour effects never change alpha; exact values for
  brightness, contrast, exposure, gamma, saturation, grayscale, sepia, invert, hue and temperature; blur keeps flat
  pictures and spreads an impulse symmetrically without losing its light, and does not darken transparent edges;
  sharpen leaves flat areas alone and steepens edges; vignette leaves the middle and darkens the corners; pixelate
  makes uniform blocks with the right average; values are clamped; same input gives same output.
* Timeline: adding in order with unique ids, refusing sound clips and a 33rd effect, setting checks, animation locking
  the fixed value, cutting and trimming keep curves exact, disabled and neutral effects are skipped, files round-trip
  and damaged files are refused, version 4 files load. The random-edit test includes effect edits.
* Render: no effects means no change; neutral and disabled effects change no bits; stack order; animated settings
  frame by frame; a cut clip renders the same frames; vignette; refused edits change nothing.
* End to end: a project with saturation, vignette and an animated blur rendered a 1080p frame of the test pattern that
  shows all three.
