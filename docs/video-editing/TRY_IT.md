# Try the Video Editing Engine on your own machine

Nothing here is urgent. Full guide: [README.md](README.md).

```bash
git clone https://github.com/FX-Foundation/originfx && cd originfx
cargo build --release     # needs the FFmpeg libraries, see ../media-engine/TRY_IT.md
O=./target/release/originfx
P=/tmp/demo.originfx
$O timeline $P new --name demo --fps 24
$O timeline $P import ~/Videos/clip.mp4        # prints asset6
$O timeline $P add asset6 --length 5 --from 2   # prints the picture clip and sound clip ids, say clip8 and clip9
$O timeline $P add asset6 --at 5 --length 5 --from 10   # clip11 and clip12
$O timeline $P transition clip11 dissolve --length 1
$O timeline $P speed clip8 2 --ripple
$O timeline $P reverse clip11
$O timeline $P show
$O render $P /tmp/demo.mp4
$O render-frame $P --at 2.0 -o /tmp/look.png
rm $P /tmp/demo.mp4 /tmp/look.png
```

## Worth checking by hand

* Every transition kind at 0.5 s, 1 s and 3 s, on footage with a lot of motion
* A dissolve between clips of different shapes (a vertical phone clip next to a landscape one)
* Speed at 25%, 50%, 200% and 800% with sound: does the sound line up with the picture, and is the pitch change acceptable?
* A reversed clip with sound, then a long reversed clip: how slow is rendering?
* Edit after adding a transition (move a clip, trim, delete): a note should say when a transition was removed, and
  undo should bring it back
* Slow motion at 10%: tell me how it looks; frame blending is the next step if it stutters too much
