# Your checklist (nothing here is urgent)

## Try it on your laptop (about 10 minutes)

```bash
git fetch origin claude/media-engine && git checkout claude/media-engine
sudo apt install ffmpeg libavcodec-dev libavformat-dev libavutil-dev libswscale-dev \
  libswresample-dev libavfilter-dev libavdevice-dev libx264-dev libx265-dev \
  clang libclang-dev pkg-config
cargo build --release
./target/release/originfx probe ~/Videos/some-clip.mp4
./target/release/originfx convert ~/Videos/some-clip.mp4 /tmp/out.mp4 --height 720
./target/release/originfx frame ~/Videos/some-clip.mp4 --at 3 -o /tmp/frame.png
cargo test
```

The release build is the slow part (a few minutes, and the laptop fan will
spin). Skip it if you want the machine cool; nothing depends on it.

## Send me these when you feel like it (this unlocks the GPU work)

1. Output of `./target/release/originfx hw` (tells me what your RTX 4060 and
   Intel graphics expose on Ubuntu).
2. Output of `./target/release/originfx bench ~/Videos/some-clip.mp4` for one
   phone clip and, if you have one, one big 4K or HEVC clip.
3. One or two real clips from your Samsung phone (portrait ones especially;
   rotation handling is tested only on generated files).

## Decisions only you can make

* Pick a **licence** (see the bottom of `ROADMAP.md`; FFmpeg's x264 matters).
* Delete the leftover `claude/push-test` branch on GitHub.
* Confirm or change the **19 export presets** list (`DECISIONS.md` #7).
* Review and merge `claude/media-engine` into `main`.
