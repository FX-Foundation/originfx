# Try the Timeline Engine on your own machine

Nothing here is urgent. Full guide: [README.md](README.md).

```bash
git clone https://github.com/FX-Foundation/originfx && cd originfx
cargo build            # needs the same FFmpeg libraries as the Media Engine, see its TRY_IT.md
P=/tmp/demo.originfx
./target/debug/originfx timeline $P new --name demo
./target/debug/originfx timeline $P import ~/Videos/some-clip.mp4     # prints asset6
./target/debug/originfx timeline $P add asset6 --length 10            # prints clip ids
./target/debug/originfx timeline $P split clip8 4
./target/debug/originfx timeline $P delete clip8 --ripple
./target/debug/originfx timeline $P show
rm $P                  # clean up
```

The timeline engine's own tests need no FFmpeg:

```bash
cargo test -p originfx-timeline-engine
```

## Worth checking by hand

* Drop-frame timecode against a real NLE: make a 29.97 sequence with `--drop-frame`, put a clip at 60 s and compare the timecode with the editor you trust
* A clip with video and audio: split it and confirm both halves split at the same frame
