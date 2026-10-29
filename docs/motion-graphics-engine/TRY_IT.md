# Try the Motion Graphics Engine on your own machine

Nothing here is urgent. Full guide: [README.md](README.md).

```bash
git clone https://github.com/FX-Foundation/originfx && cd originfx
cargo build --release     # needs the FFmpeg libraries, see ../media-engine/TRY_IT.md
O=./target/release/originfx
P=/tmp/demo.originfx
$O timeline $P new --name demo --fps 24 --video-tracks 5
$O timeline $P import ~/Videos/interview.mp4                     # prints asset6
$O timeline $P add asset6 --length 6                             # clip8 (picture) and clip9 (sound) on V1
$O timeline $P graphic add bar shape rect --width 60 --height 12 --corner 100 --fill "#000000aa"   # asset10
$O timeline $P graphic add title text "Your Name\nYour Title" --size 6 --outline black   # asset11 (add --font FILE if you like)
$O timeline $P add asset10 --length 6 --video-track 2            # prints clip12
$O timeline $P add asset11 --length 6 --video-track 3            # prints clip13
$O timeline $P look clip12 y 35
$O timeline $P look clip13 y 35
$O timeline $P key clip12 opacity 0 0
$O timeline $P key clip12 opacity 1 100
$O timeline $P key clip13 opacity 0.5 0
$O timeline $P key clip13 opacity 1.5 100
$O render-frame $P --at 3 -o /tmp/a.png
$O render $P /tmp/demo.mp4
rm $P /tmp/a.png /tmp/demo.mp4
```

## Worth checking by hand

* Text: does it look right at small and large sizes? Kerning, outline thickness, shadow offset, tracking.
* With your own fonts (`--font`): do `.otf` and `.ttf` files both work? Any that fail to load?
* Shapes: are the edges clean at 4K? Do stars and polygons look right at different `--sides` and `--inner`?
* Gradients on smooth backgrounds: any banding? (8-bit only, listed on the roadmap.)
* A title with opacity and position keys, a dissolve between two titles, a shape on top of a video with a blend mode.
* What is missing for the titles you actually make: text boxes with wrapping? per-letter animation? soft shadows?
