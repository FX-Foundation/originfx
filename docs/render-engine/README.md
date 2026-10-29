# Render Engine

Engine 3 of 10. It turns a timeline into pictures, sound and finished files. It reads a project from
the [Timeline Engine](../timeline-engine/README.md), gets decoded media from the
[Media Engine](../media-engine/README.md), and answers two questions:

* what does frame 120 of this sequence look like?
* what is heard between these two moments?

On top of those it exports a whole sequence (or an in/out range) with any of the Media Engine's 19
presets.

Crate: `crates/render-engine` (package `originfx-render-engine`, library `render_engine`).
This is the **CPU renderer**: the same code on every machine gives the same pixels. It is the reference
that a future GPU renderer is checked against.

## What it does to a frame

1. Start with the background (opaque black unless told otherwise).
2. For each video track from the bottom up, if it is enabled and has an enabled clip at that moment, ask the
   media source for the picture at the right time of the file (`source_in` plus how far into the clip we are).
3. Fit the picture to the sequence's frame (`Fit::Contain` is the default: all of it, with bars if the shape differs;
   `Cover` fills and crops; `Stretch` distorts).
4. Blend it over what is below, with straight alpha.

Resizing uses a triangle filter that widens when shrinking, so downscaling averages instead of skipping
pixels, and colour is weighted by alpha so transparent edges do not get dark fringes. Still images are
held for any length and are resized once, not once per frame. Rotation tags are handled by the decoder.

## What it does to sound

Every audio track that is not muted (or, if any track is soloed, only the soloed ones) contributes its
enabled clips. Clip edges are rounded to whole samples, and the mix of a range is the same however you cut it
into pieces, so consecutive requests join without gaps, overlaps or clicks. Mono sources are
spread to stereo; the mix is stereo `f32` at the sequence's sample rate and is clipped at full scale.

## Exporting

```rust
use render_engine::*;
use media_engine::{ExportPreset, ExportSettings};

let report = export_sequence(
    &project, sequence_id, "out.mp4",
    &ExportSettings::new(ExportPreset::Mp4H264),
    &ExportOptions::default(),
    |p| { println!("{:?} {:?}", p.phase, p.fraction); true }, // return false to cancel
)?;
```

* **Streaming.** On Linux and macOS, rendering and encoding run at the same time through an in-memory pipe:
  frames go from the renderer to the encoder uncompressed, nothing touches the disk, and no time is spent on an
  intermediate format. On other systems, or with `keep_working_file`, frames go to a lossless working file first
  (large; `temp_dir` can point it at a roomy drive) which is deleted afterwards.
* **Range.** `ExportOptions::range` renders only an in/out span. Times are moved to whole frames.
* **Presets.** Everything the Media Engine offers works, including quality, height, bitrate, GPU encoding
  (NVENC) and audio-only presets (no pictures are composited for those). The GIF preset has no sound.
* **Missing media.** The export checks every used file first and stops with the file's name if one is gone.
  `MissingMedia::Offline` shows magenta and silence instead, for previews.
* **Cancel.** Return `false` from the progress callback. No output file is left behind, and neither is a working file.
* **Errors in the middle** of an export (a failed encoder, a missing file) stop both sides cleanly; neither can hang the other.
* **Progress** has two phases: `Rendering` (frames handed over, with encoding running alongside) and `Encoding` (the encoder
  finishing the file).

## Command line

```bash
originfx render demo.originfx out.mp4                      # preset guessed from .mp4
originfx render demo.originfx out.mov -p mov-h264 -q high
originfx render demo.originfx out.wav                       # sound only
originfx render demo.originfx part.mp4 --from 10 --to 25    # in/out range
originfx render demo.originfx out.mp4 --fit cover --height 720
originfx render-frame demo.originfx --at 12.5 -o look.png   # one picture
```

Also `--hw auto` (NVENC), `--allow-offline`, `--sequence <id>`, `--temp-dir`, `--keep-working-file`.

## As a library

* `Renderer::new(&project, sequence, FileSource::new(), RenderSettings::default())`, then `frame(n)`,
  `frame_at(time)`, `audio(range)` and `audio_samples(first_sample, count)`.
* `MediaSource` is the trait that supplies pictures and sound. `FileSource` reads real files; tests, proxies or a
  future cache server plug in their own.
* A `Renderer` works on a snapshot of the project taken when it is created, so editing while a render runs is safe.

## Tests

31 tests: exact blending and fitting maths, a fake media source that checks layer order, source-time
mapping, disabled and muted material, solo, clipping and sample-exact joins; and end-to-end tests that write
real files, build a timeline, export MP4/WAV/GIF/MP3, decode the results and check colours, lengths, frame rates,
cancellation, missing media, NTSC sample counts and failure behaviour of the pipe.

```bash
cargo test -p originfx-render-engine
```

## What it is not (yet)

Opacity, transforms, blend modes, effects and keyframes come with the engines that own them
(see [ROADMAP.md](ROADMAP.md)). Speed, reverse and transitions are rendered here and described in the
[Video Editing Engine guide](../video-editing/README.md).
