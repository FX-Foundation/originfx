# Try the Render Engine on your own machine

Nothing here is urgent. Full guide: [README.md](README.md).

```bash
git clone https://github.com/FX-Foundation/originfx && cd originfx
cargo build --release     # needs the FFmpeg libraries, see ../media-engine/TRY_IT.md
O=./target/release/originfx
P=/tmp/demo.originfx
$O timeline $P new --name demo --fps 24
$O timeline $P import ~/Videos/clip1.mp4        # prints asset6
$O timeline $P import ~/Videos/clip2.mp4        # prints asset7
$O timeline $P add asset6 --length 5
$O timeline $P add asset7 --at 5 --length 5
$O render $P /tmp/demo.mp4
$O render-frame $P --at 2.5 -o /tmp/look.png
rm $P /tmp/demo.mp4 /tmp/look.png
```

## Worth checking by hand

* Footage in a different shape from the sequence (vertical phone video on a 16:9 timeline): try `--fit contain`, `cover` and `stretch`
* A clip with sound cut in the middle: listen at the cut for clicks (there should be none)
* A long export: watch memory and CPU; tell me the frame rate you get and how many cores you have
* Footage from a phone with rotation, and a 4K clip on a 1080p timeline
