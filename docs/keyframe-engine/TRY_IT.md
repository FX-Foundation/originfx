# Try the Keyframe Engine on your own machine

Nothing here is urgent. Full guide: [README.md](README.md).

```bash
git clone https://github.com/FX-Foundation/originfx && cd originfx
cargo build --release     # needs the FFmpeg libraries, see ../media-engine/TRY_IT.md
O=./target/release/originfx
P=/tmp/demo.originfx
$O timeline $P new --name demo --fps 24
$O timeline $P import ~/Videos/interview.mp4            # prints asset6
$O timeline $P add asset6 --length 10                   # a picture clip and a sound clip, say clip8 and clip9
$O timeline $P key clip9 gain 0 0                       # volume 0 dB at the start ...
$O timeline $P key clip9 gain 10 -30 --interp smooth    # ... easing down to -30 dB at 10 s
$O timeline $P key clip9 pan 0 -100                     # sound sweeps from left ...
$O timeline $P key clip9 pan 10 100                     # ... to right
$O timeline $P key clip8 speed 4 1                      # picture and sound play normally until 4 s ...
$O timeline $P key clip8 speed 8 2 --interp smooth      # ... then ease up to double speed
$O timeline $P show
$O render $P /tmp/demo.mp4
rm $P /tmp/demo.mp4
```

## Worth checking by hand

* Does the volume fade sound even, or does it drop too early or too late? (It is a straight line in dB.)
* Does the speed ramp look smooth on moving footage? Slow parts repeat frames; is that acceptable until frame blending exists?
* Pan sweep on mono voice and on stereo music
* Cut a clip with a ramp and a fade in the middle: does the cut change anything you can see or hear?
* Does the sound stay in step with the picture through the whole ramp, and when reversed?
* `key` on a value, then try `gain` or `speed` to set a fixed value: is the refusal message clear?
