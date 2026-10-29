# Motion Graphics Engine

Engine 9 of 10. Layers that are drawn rather than filmed: solid colours, gradients, shapes and text. They are media
like a file is, so everything else in OriginFX works on them: trim, move, transitions, opacity, blend modes, masks,
effects and keyframes.

Crate: `crates/motion-graphics-engine` (package `originfx-motion-graphics-engine`, library `motion_graphics_engine`).
One function, `render(generator, w, h, fonts)`, draws a graphic onto a transparent RGBA picture of the frame's size. It
knows nothing of clips or files. The project side (the graphic as an asset, its rules, saving) is in the
[Timeline Engine](../timeline-engine/README.md); the [Render Engine](../render-engine/README.md) draws it (once per
size, then reuses it for every frame) and the [Compositing Engine](../compositing-engine/README.md) places it.
Defaults chosen are in [DECISIONS.md](DECISIONS.md).

## The graphics

Sizes are a percent of the frame (widths of the width, heights and text size of the height), so a graphic looks the same
at preview and export size. Colours are `#rrggbb`, `#rrggbbaa`, `#rgb` or a name (white, black, red, green, blue, yellow,
cyan, magenta, orange, grey, transparent).

| Graphic | What it is |
|---|---|
| `solid` | The whole frame in one colour |
| `gradient` | The whole frame blending from one colour to another at an angle (0 left to right, 90 top to bottom) |
| `shape rect` | A rectangle, with `--corner` rounding (percent of the shorter side; 100 is a pill) |
| `shape ellipse` | An ellipse (or circle) |
| `shape polygon` | A regular polygon with `--sides` 3 to 32, one corner up |
| `shape star` | A star with `--sides` points and `--inner` radius (percent of the outer) |
| `text` | Text, with size, colour, alignment, line gap, letter spacing, outline and drop shadow |

Shapes take a `--fill` (or a gradient with `--fill-to`) and an optional `--stroke` outline centred on the edge. Edges are
smoothly anti-aliased and colours can be translucent.

Graphics are drawn **in the middle of the frame**. Put them where you want with `look` on the clip (`x`, `y`, `scale-x`,
`rotation`, `opacity` and so on), and animate that with `key`: a title that slides in and fades is a position key and an
opacity key.

## Text and fonts

Text needs a font file. Name one with `--font path/to/font.ttf` (.ttf or .otf). With no `--font` OriginFX uses the first
usual system font it finds (DejaVu Sans, Liberation Sans, FreeSans, Inter, Arial, Segoe UI), and says so clearly if there
is none. The project stores the font's *path*, like it stores the path of a media file, so a project moved to another
machine needs the same font there (or the text re-pointed with `graphic set`). A missing font stops the render with a
clear message; it never draws a wrong font silently.

Lines are split at newlines (`\n` on the command line). `--align` sets how the lines sit against each other.

## Command line

```bash
O=originfx; P=project.originfx
$O timeline $P graphic add bg gradient "#1a1a40" "#c0392b" --angle 45        # prints an asset id, say asset10
$O timeline $P graphic add star shape star --width 18 --height 32 --fill "#ffd700" --fill-to "#ff8c00" --stroke white
$O timeline $P graphic add bar shape rect --width 60 --height 12 --corner 100 --fill "#00000099"
$O timeline $P graphic add title text "ORIGIN\nFX" --size 14 --outline black --shadow "#000000aa" --tracking 8
$O timeline $P add asset10 --length 5 --video-track 2       # place it like any picture (graphics need a --length)
$O timeline $P look clip13 opacity 55                       # then everything else works on it
$O timeline $P graphic set asset12 text "NEW TITLE" --size 12   # change it; every clip using it changes
```

`new` takes `--video-tracks N` so there is room to stack layers. `show` lists graphics with a one-line description.

## In the project file

Format version 7. A graphic is saved as a `generator` on its asset (colours as hex text); a project with no graphics
writes nothing extra, and files from versions 1 to 6 open unchanged. A graphic in a file is checked with the same
rules as a new one.

## Known limits

See [ROADMAP.md](ROADMAP.md). The main ones: graphics are always centred (the rotation pivot is the frame centre), text
is one font and one style per graphic, there is no per-letter animation, and everything is CPU, 8-bit, one thread.
