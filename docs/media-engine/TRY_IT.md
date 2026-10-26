# Try the Media Engine on your own machine

Nothing here is urgent. Full guide: [README.md](README.md).

## Try it on your own machine (about 10 minutes)

```bash
git clone https://github.com/FX-Foundation/originfx && cd originfx
sudo apt install ffmpeg libavcodec-dev libavformat-dev libavutil-dev libswscale-dev \
  libswresample-dev libavfilter-dev libavdevice-dev libx264-dev libx265-dev \
  clang libclang-dev pkg-config
cargo build --release
./target/release/originfx probe ~/Videos/some-clip.mp4
./target/release/originfx convert ~/Videos/some-clip.mp4 /tmp/out.mp4 --height 720
./target/release/originfx frame ~/Videos/some-clip.mp4 --at 3 -o /tmp/frame.png
cargo test
```

The release build is the slow part (a few minutes, and the fan will spin). Skip it if you
want the machine cool; nothing depends on it.

## Worth trying with real footage

Rotation handling is tested only on generated clips, and subtitle and chapter handling on
generated MKV and MP4 files. A few real files would be a good check:

1. A portrait clip from a phone: `originfx probe` should report a rotation, and `frame`
   should save an upright picture.
2. A film or episode with subtitles and chapters:
   `originfx convert in.mkv out.mkv --subtitles --chapters`, then open `out.mkv` in a player.
3. If you have an NVIDIA GPU: `originfx convert in.mov out.mp4 --hw auto`, which prints the
   encoder used.

If anything misbehaves, `originfx hw` and `originfx bench <file>` print what to paste into
a bug report.
