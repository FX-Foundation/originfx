# Try the Effects Engine on your own machine

Nothing here is urgent. Full guide: [README.md](README.md).

```bash
git clone https://github.com/FX-Foundation/originfx && cd originfx
cargo build --release     # needs the FFmpeg libraries, see ../media-engine/TRY_IT.md
O=./target/release/originfx
P=/tmp/demo.originfx
$O timeline $P new --name demo --fps 24
$O timeline $P import ~/Videos/interview.mp4            # prints asset6
$O timeline $P add asset6 --length 6                    # a picture clip and a sound clip, say clip8 and clip9
$O timeline $P effect kinds
$O timeline $P effect add clip8 saturation              # prints fx1
$O timeline $P effect set clip8 fx1 amount 140
$O timeline $P effect add clip8 vignette                # fx2
$O timeline $P effect add clip8 blur                    # fx3
$O timeline $P effect key clip8 fx3 radius 0 0          # sharp at the start ...
$O timeline $P effect key clip8 fx3 radius 4 4 --interp smooth   # ... soft by 4 s
$O render-frame $P --at 0.5 -o /tmp/a.png
$O render-frame $P --at 4.5 -o /tmp/b.png
$O render $P /tmp/demo.mp4
rm $P /tmp/a.png /tmp/b.png /tmp/demo.mp4
```

## Worth checking by hand

* On your own footage: does each effect look like what other editors give you at the same numbers? Which defaults feel wrong?
* Skin tones under saturation, temperature and hue: any ugly shifts?
* Heavy blur on a smooth sky: do you see banding? (8-bit only, listed on the roadmap.)
* Sharpen on faces and on text: halos?
* Vignette softness and amount: usable range?
* Order of the stack: invert then brighten against brighten then invert
* How slow is a 4K frame with blur and sharpen? It is single threaded for now.
* A clip with effects cut in half: does the cut change anything you can see?
