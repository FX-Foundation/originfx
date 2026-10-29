# Timeline Engine

Engine 2 of 10. It holds the timeline: which clips sit on which tracks, where, and every way to
change them. It never touches pixels or sound. It says *which part of which file* is on screen or
heard at a given moment; the [Media Engine](../media-engine/README.md) supplies the frames.

Crate: `crates/timeline-engine` (package `originfx-timeline-engine`, library `timeline_engine`).
Pure Rust: it builds without FFmpeg unless you turn on the `media` feature.

## The model

| Thing | What it is |
|---|---|
| `Project` | Assets plus sequences. Saved as one `.originfx` file |
| `Asset` | A media file and the facts needed to edit it: length, picture sizes and rates, audio rates. No pixels |
| `Sequence` | One timeline: frame rate, size, sample rate, timecode start, video tracks, audio tracks, markers |
| `Track` | A row of clips that never overlap. Can be disabled (hidden / muted), locked, soloed (audio) |
| `Clip` | A slice of an asset (`source_in`, `duration`) placed at `start`. Can be disabled and can belong to a link group |
| `Marker` | A coloured note at a time, optionally with a length |

Video tracks are listed bottom first: V1 is drawn underneath V2, and so on.

## Time

`Time` counts **ticks of 1/254,016,000,000 s** in an `i64`. That number divides evenly into every
standard frame rate (23.976, 24, 25, 29.97, 30, 48, 50, 59.94, 60) and audio rate (44.1k, 48k, 96k
and others), so frames and samples land exactly and nothing drifts over a long edit. There is a test
for every rate. `FrameRate` is an exact fraction (23.976 is 24000/1001, not 23.976).

Everything an edit touches snaps to the sequence's frame grid, so clips always start and end on frames.

`Timecode` converts between frames and `HH:MM:SS:FF`, including **drop-frame** at 29.97 and 59.94
(`;` before the frames). Known values are tested (frame 17,982 at 29.97 is `00:10:00;00`), and
every frame of an hour round-trips.

## Editing

All changes go through an `Editor`. It works on a private copy of the project, checks the result
against every rule (`Project::validate`), and only then commits. If anything fails, the project is
exactly as it was. A property test applies thousands of random edits and checks this every time.

| Edit | What it does |
|---|---|
| `place_clip`, `place_asset` | Put a clip on a track. **Overwrite** replaces what is under it. **Insert** makes room: later clips move, a clip under the point is split. `place_asset` makes a linked picture and sound pair |
| `split_clip`, `split_all` | Cut at a time (one clip with its links, or everything under the playhead) |
| `delete_clips` | Delete, with ripple **Off**, **Track** or **Sync** (all unlocked tracks stay in step; refuses if another track has something in the gap) |
| `ripple_delete_range` | Cut a span of time out of the tracks and close the gap |
| `move_clips` | Move in time and across tracks. Whatever they land on is overwritten |
| `trim` | Move a clip's head or tail, normal or ripple |
| `roll` | Move the cut between two touching clips |
| `slip` | Show a different part of the media; clip stays put |
| `slide` | Move a clip between its neighbours, which stretch and shrink |
| `link`, `unlink`, `set_enabled`, `rename_clip` | Clip properties. With `LinkMode::Follow`, edits to one clip also apply to the clips linked to it |
| tracks, markers, assets, sequences | Add, remove, update. Removing an asset that a clip uses is refused |

Locked tracks refuse every edit. Clips cannot start before zero, be shorter than a frame, or run
past the end of their media.

### Undo, redo, groups

Projects are *persistent* data: a copy shares everything that did not change. So undo is just
"go back to the old project". There is no inverse of each edit to get wrong, and keeping thousands
of steps costs little (limit: 10,000). `Editor::group` makes several edits one undo step, and takes
them all back if one fails. `is_dirty` and `mark_saved` track unsaved changes, and undoing back to
the saved state clears the flag.

A snapshot (`editor.project().clone()`) never changes afterwards, so a player or exporter thread can
hold one while you keep editing.

## Asking questions

* `Sequence::video_layers_at(t)`: the picture layers at a moment, bottom first, with the exact time to read in each file
* `Sequence::audio_at(t)`: the sounds, respecting mute and solo
* `Sequence::edit_points()`, `duration()`, `timecode_at(t)`
* `Project::used_assets()`, `Project::missing_assets()`

## Project files

`Project::save` / `load` write plain, human-readable JSON (`"format": "originfx-project"`,
`"version": 5`; versions 1 to 4 files still load). Saving goes through a temporary file and a rename, so a crash cannot leave half a
project. Loading runs the full rule check, so a damaged or hand-edited file is refused with a clear
error. A file from a newer version is refused with `UnsupportedVersion` instead of being guessed at.

Clips also carry a speed, a reverse flag and an optional transition from the clip before them; see the
[Video Editing Engine guide](../video-editing/README.md). Audio clips, audio tracks and sequences also carry gain, pan,
fades and a master level; see the [Audio Engine guide](../audio-engine/README.md).

## From the command line

```bash
originfx timeline demo.originfx new --name demo --fps 24
originfx timeline demo.originfx import clip.mp4          # prints asset6
originfx timeline demo.originfx add asset6 --length 10   # prints the new clip ids
originfx timeline demo.originfx split clip8 4
originfx timeline demo.originfx delete clip8 --ripple
originfx timeline demo.originfx show
originfx timeline demo.originfx check
```

Also `trim`, `move` and `marker`. Times are in seconds, track numbers start at 1.

## As a library

```rust
use timeline_engine::*;

let mut ed = Editor::new("demo");
let seq = ed.add_sequence(SequenceSettings::default())?;
// ... add assets, place clips (see the crate docs for a full example)
ed.undo()?;
```

With the `media` feature, `timeline_engine::media::probe_asset(path)` reads a real file through the
Media Engine and returns a `NewAsset`.

## Tests

35 tests plus a randomised stress test (200 random edit sequences per run) that checks three things:
a failed edit changes nothing, every successful edit leaves a valid project, and undoing everything
returns to the start. The file format round-trips exactly after every random run.

```bash
cargo test -p originfx-timeline-engine
```
