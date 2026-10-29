# Try the Audio Engine on your own machine

Nothing here is urgent. Full guide: [README.md](README.md).

```bash
git clone https://github.com/FX-Foundation/originfx && cd originfx
cargo build --release     # needs the FFmpeg libraries, see ../media-engine/TRY_IT.md
O=./target/release/originfx
P=/tmp/demo.originfx
$O loudness ~/Videos/interview.mp4                      # how loud is it as it is?
$O timeline $P new --name demo --fps 24
$O timeline $P import ~/Videos/interview.mp4            # prints asset6
$O timeline $P add asset6 --length 20                   # prints a picture clip and a sound clip, say clip8 and clip9
$O timeline $P fade clip9 --in 0.5 --out 1.5
$O timeline $P master --normalize -16                   # podcast / streaming level
$O loudness $P
$O render $P /tmp/demo.mp4 --limiter soft
$O loudness /tmp/demo.mp4                               # the exported file should read -16
rm $P /tmp/demo.mp4
```

## Worth checking by hand

* Your own recordings: does `loudness` agree with a meter you trust (Audacity's Loudness Normalization, YouTube's "stats for nerds")?
* Fade curves on music and on speech: are `smooth` and `equal-power` the right defaults?
* Pan on mono voice and on stereo music
* A hot mix (two loud clips on top of each other): `--limiter hard` against `--limiter soft`; which sounds better?
* Normalising something very quiet: the +24 dB limit should stop you with a clear message
