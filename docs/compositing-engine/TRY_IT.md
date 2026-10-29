# Try the Compositing Engine on your own machine

Nothing here is urgent. Full guide: [README.md](README.md).

```bash
git clone https://github.com/FX-Foundation/originfx && cd originfx
cargo build --release     # needs the FFmpeg libraries, see ../media-engine/TRY_IT.md
O=./target/release/originfx
P=/tmp/demo.originfx
$O timeline $P new --name demo --fps 24
$O timeline $P import ~/Videos/interview.mp4                  # prints asset6
$O timeline $P add asset6 --length 6                          # clip8 (picture) and clip9 (sound) on track 1
$O timeline $P add asset6 --from 10 --length 6 --video-track 2 --no-audio    # clip10 on top
$O timeline $P look clip10 scale-x 40
$O timeline $P look clip10 scale-y 40
$O timeline $P mask clip10 ellipse
$O timeline $P look clip10 mask-width 90
$O timeline $P look clip10 mask-height 90
$O timeline $P look clip10 mask-feather 3
$O timeline $P key clip10 x 0 -40
$O timeline $P key clip10 x 4 40 --interp smooth
$O timeline $P key clip10 opacity 0 0
$O timeline $P key clip10 opacity 1 100
$O timeline $P blend clip10 screen
$O render-frame $P --at 2 -o /tmp/a.png
$O render $P /tmp/demo.mp4
rm $P /tmp/a.png /tmp/demo.mp4
```

## Worth checking by hand

* Picture in picture: do the position, scale and corner feel right? Is the scale about the centre what you expect?
* Each blend mode against a few backgrounds: does it match what you get in other editors?
* Mask feather: usable range? Do inverted masks behave as you expect?
* Shrink a sharp clip to 20 %: is the aliasing bad enough to move mip-mapping up the list?
* A clip with a look cut in half: does the cut change anything you can see?
* How slow is a 4K frame with several layers, masks and rotations? It is single threaded for now.
