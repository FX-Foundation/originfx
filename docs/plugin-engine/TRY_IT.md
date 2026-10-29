# Try the Plugin Engine on your own machine

Nothing here is urgent. Full guide: [README.md](README.md).

```bash
git clone https://github.com/FX-Foundation/originfx && cd originfx
cargo build --release     # needs the FFmpeg libraries, see ../media-engine/TRY_IT.md
O=./target/release/originfx
P=/tmp/demo.originfx
D=/tmp/demo-plugins
$O plugin example $D                                   # writes $D/invert-mix.wasm
$O plugin list --plugins $D
$O timeline $P new --name demo --fps 24
$O timeline $P import ~/Videos/interview.mp4           # prints asset6
$O timeline $P add asset6 --length 6                   # clip8 (picture) and clip9 (sound)
$O timeline $P effect add clip8 plugin:originfx.example.invert-mix --plugins $D     # prints fx1
$O timeline $P effect key clip8 fx1 amount 0 0
$O timeline $P effect key clip8 fx1 amount 4 100
$O render-frame $P --at 2 -o /tmp/a.png --plugins $D
$O render-frame $P --at 2 -o /tmp/b.png                # without --plugins: a clear "not installed" error
$O render $P /tmp/demo.mp4 --plugins $D
rm -r $P $D /tmp/a.png /tmp/demo.mp4
```

## Worth checking by hand

* How slow is a plugin on a 4K frame? Is the interpreter fast enough for the plugins you would write?
* Try to break it: write a plugin that loops forever, allocates too much, or reads outside its memory. Each should stop with a
  clear message and leave the picture alone.
* Write a small real plugin in Rust (or C) from the sample in the guide and tell us what was awkward in the interface.
* What do you want plugins to be able to do that they cannot (time and neighbouring frames, a second picture, sound)?
