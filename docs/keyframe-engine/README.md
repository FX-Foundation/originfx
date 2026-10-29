# Keyframe Engine

Engine 6 of 10. Values that change over time: a volume that fades under a voice, a pan that sweeps across the
picture, a clip that plays slowly and then speeds up.

Crate: `crates/keyframe-engine` (package `originfx-keyframe-engine`, library `keyframe_engine`). It is pure maths with
no dependencies: a `Curve` is a list of keys, and it says what the value is at any moment, what the area under it is,
and how to cut it in two without changing it. It knows nothing of clips or files. What can be animated, and the
edits that animate it, belong to the [Timeline Engine](../timeline-engine/README.md); the
[Render Engine](../render-engine/README.md) reads the curves when it mixes and draws. See
[DECISIONS.md](DECISIONS.md) K1.

## What can be animated

| Where | Value | Unit | Range |
|---|---|---|---|
| Clip | `gain` (audio clips) | dB | -120 to +24 |
| Clip | `pan` (audio clips) | percent | -100 (left) to 100 (right) |
| Clip | `speed` (picture and sound clips) | ratio, 1 is normal | 0.01 to 100 |
| Audio track | `gain`, `pan` | as above | as above |
| Sequence | master `gain` | dB | -120 to +24 |

Opacity, position, scale and effect settings arrive with the engines that own them (Compositing, Effects); they will
use the same curves and the same edits.

A value with a curve is **animated**: the curve replaces the fixed value. While a value is animated its fixed
field (`speed`, `gain`, `pan`) is ignored, and changing it is refused with a clear message until the curve is
cleared. Clearing brings the fixed value back.

## Keys and interpolation

A key is a time, a value and a way to travel to the next key.

| `interp` | Between this key and the next |
|---|---|
| `hold` | Keeps this value, then jumps at the next key |
| `linear` | A straight line |
| `smooth` | A smooth cubic: eases in and out at the ends and flows through the keys in the middle, never overshooting them |

Before the first key the curve holds the first value; after the last it holds the last.

Gain curves are drawn **in decibels**, so a straight line is a steady fade that sounds even, not a hump in the
linear level. A `smooth` curve through three or more keys uses automatic slopes (zero at the ends and at turning
points, a monotone average elsewhere); the slopes are stored with the key, so hand-set slopes are possible later
without a file change.

## Editing

```rust
use timeline_engine::*;

// Fade clip volume from 0 dB at 12 s down to -20 dB at 15 s (times are on the timeline, in seconds).
ed.set_keyframe(seq, Target::Clip(clip), Param::Gain, Time::from_secs_f64(12.0), 0.0,   Interp::Linear, LinkMode::Follow)?;
ed.set_keyframe(seq, Target::Clip(clip), Param::Gain, Time::from_secs_f64(15.0), -20.0, Interp::Linear, LinkMode::Follow)?;
ed.remove_keyframe(seq, Target::Clip(clip), Param::Gain, Time::from_secs_f64(15.0), LinkMode::Follow)?;
ed.clear_animation(seq, Target::Clip(clip), Param::Gain, LinkMode::Follow)?;
```

* A value that was not animated yet **starts from its fixed value** at the start of the clip (or of the sequence),
  so the first key you set does not change what came before it. Setting a gain key at 12 s on a clip at -6 dB
  holds -6 dB until 12 s.
* A key at a time that already has one replaces it. Removing the last key ends the animation.
* Clip keys must lie inside the clip (its end is allowed). Values outside the range are refused.
* A **speed** key follows links, so a picture and its sound stay in step; gain and pan apply to the one audio clip.
* Everything is an ordinary edit: undoable, validated, all-or-nothing, saved in the project.

### Cutting, trimming, moving

* **Clip keys move with their clip.** Their times are measured from the clip's start.
* **Cutting a clip cuts its curves exactly.** Both halves evaluate the same as the whole did, including smooth
  segments (a cubic cut in two is two cubics). That holds for speed ramps too: the two halves meet in the media to
  within a few ticks, and every frame shows the same moment as before the cut.
* **Trimming** shortens or lengthens a clip without moving its curves; a lengthened clip holds its end values.
* **Track and master keys stay where they are on the timeline.** Moving clips does not move them.

## Speed ramps

A speed key on a clip makes a ramp. The amount of media that has played at any moment is the **exact area under
the speed curve** (the curve's integral), so a ramp from 1x to 3x over 4 seconds plays 8 seconds of media, to the
tick. Both picture and sound follow it, forwards or reversed. A ramp is refused if the clip would then run past the
end of its media. `set_speed` (a fixed speed) is refused while the speed is animated.

## Command line

```bash
originfx timeline p.originfx key clip9 gain 0 0                 # target, value name, seconds, value
originfx timeline p.originfx key clip9 gain 8 -40 --interp smooth
originfx timeline p.originfx key clip8 speed 4 1                # a speed ramp: 1x at 4 s ...
originfx timeline p.originfx key clip8 speed 8 2 --interp smooth  # ... easing to 2x at 8 s
originfx timeline p.originfx key a1 pan 0 -100                  # audio track 1
originfx timeline p.originfx key master gain 0 -3
originfx timeline p.originfx unkey clip9 gain 8                 # remove the key at exactly 8 s
originfx timeline p.originfx clear-keys clip9 gain
originfx timeline p.originfx show                               # lists animated values and key counts
```

Targets are `clip<N>`, `a<N>` (audio track) or `master`. Values are dB (`-6`, `-inf`), percent (`-40`) or a speed
ratio (`2`, `0.5`).

## Project files

Format version 4. Older files load unchanged. A value with a curve is written as a list of keys (time in ticks, value,
interpolation, optional slope); a project with no animation writes none of it. Keys out of order, values out of
range, unknown names and keys outside their clip are refused on load. Numbers are written so that they read back
exactly.

## How it is tested

* Curve crate: values before, between and after keys; smooth curves pass through their keys, ease at the ends and
  never overshoot; integrals checked against numerical integration; slices evaluate exactly like the original and
  keep their areas; refused key lists; name round trips.
* Timeline: keys start from the fixed value, fixed values are locked while animated, bad keys are refused and change
  nothing, removing keys, cutting and trimming and moving keep automation exact, ramps change media length exactly,
  ramps cannot run past the media, reversed ramps, links, track and master curves, files round-trip and damaged
  files are refused, version 3 files still load. The random-edit test now includes keyframe edits.
* Render: gain automation sample by sample, pan sweeps, track and master curves in sequence time, speed ramps against
  the exact formula, reversed ramps, picture requests following a ramp, a cut ramped clip sounding identical to the
  whole, and no change at all without keyframes.
* End to end: a project with a fading, panning clip, a ramp and a master curve renders to a file of the right length
  whose measured level falls and whose channels cross over as set.
