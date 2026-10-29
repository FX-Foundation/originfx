# Compositing Engine

Engine 8 of 10. How a picture clip is laid over the layers beneath it: opacity, blend mode, position, scale, rotation,
crop and masks. Every number can change over time.

Crate: `crates/compositing-engine` (package `originfx-compositing-engine`, library `compositing_engine`). Pure CPU code
with no dependencies; it works on RGBA pictures and knows nothing of clips or files. What a clip remembers (its look and
its keyframes) belongs to the [Timeline Engine](../timeline-engine/README.md); the
[Render Engine](../render-engine/README.md) applies it when it draws. Keyframes come from the
[Keyframe Engine](../keyframe-engine/README.md). Defaults chosen are in [DECISIONS.md](DECISIONS.md).

## What a picture clip can do

| Setting | Name | Range | Default |
|---|---|---|---|
| Opacity | `opacity` | 0 to 100 % | 100 |
| Move right / down | `x`, `y` | -300 to 300 % of the frame width / height | 0 |
| Size | `scale-x`, `scale-y` | 0 to 1000 % | 100 |
| Clockwise turn | `rotation` | -3600 to 3600 degrees | 0 |
| Crop from a side | `crop-left`, `crop-right`, `crop-top`, `crop-bottom` | 0 to 100 % | 0 |
| Mask centre | `mask-x`, `mask-y` | -100 to 200 % of the frame | 50 |
| Mask size | `mask-width`, `mask-height` | 0 to 300 % of the frame | 50 |
| Mask soft edge | `mask-feather` | 0 to 50 % of the frame height | 0 |

Scale and rotation turn about the **centre of the frame**. A mask is a `rect` or an `ellipse`, optionally `--invert`ed
(show the outside instead). Blend modes: `normal`, `multiply`, `screen`, `overlay`, `darken`, `lighten`, `hard-light`,
`soft-light`, `difference`, `exclusion`, `add`. (`blend <clip> list` prints the exact names.)

## Order of work

For each picture clip the renderer draws the picture on its own transparent canvas, then: **effects** (in stack order),
**crop**, **mask**, **move/scale/rotate**, and finally lays the result over the layers beneath with its **opacity and
blend mode**. So a blur follows the picture, a mask cuts the blurred picture, and a transform moves the cut picture.
Clips on higher video tracks go on top of lower ones.

A clip with no look settings and no effects takes the old direct path, so existing projects render bit for bit as before.

## Command line

```bash
O=originfx; P=project.originfx
$O timeline $P look clip8 opacity 60
$O timeline $P look clip8 scale-x 40          # and scale-y
$O timeline $P look clip8 rotation 10
$O timeline $P look clip8 crop-left 25
$O timeline $P blend clip8 screen
$O timeline $P mask clip8 ellipse             # then: look clip8 mask-width 90, mask-feather 3 ...
$O timeline $P mask clip8 rect --invert
$O timeline $P mask clip8 off
$O timeline $P key clip8 x 0 -40              # animate any of them: key, unkey, clear-keys
$O timeline $P key clip8 x 3 40 --interp smooth
```

`show` lists each clip's look, blend mode and mask. A fixed value cannot be set while it is animated (clear the keys
first); the first key on a value starts from its fixed value at the clip's start. Mask values need a mask.

## In the project file

Format version 6. A clip without look settings saves nothing extra, and files from versions 1 to 5 open unchanged.
Cutting a clip slices its look keyframes exactly, like every other animated value.

## Known limits

See [ROADMAP.md](ROADMAP.md). The main ones: scaling down a lot aliases (no mip filter), everything is 8-bit CPU on one
thread, and there is no track-level or sequence-level adjustment layer yet.
