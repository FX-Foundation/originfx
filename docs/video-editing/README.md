# Video Editing Engine

Engine 4 of 10. It changes *when* a clip's media plays and how one clip hands over to the next:

* **Speed.** Any speed from 1% to 10,000%, as an exact fraction (50%, 250%, 3/2 never drift).
* **Reverse.** Play a clip backwards.
* **Transitions.** Dissolve, dip to black, dip to white, wipes and pushes in four directions, and an equal-power
  audio crossfade.

It has no crate of its own. The data and the edits live in the [Timeline Engine](../timeline-engine/README.md)
(`crates/timeline-engine/src/retime.rs`, plus changes to the model and edit operations), and the pixels and
samples are made by the [Render Engine](../render-engine/README.md) (`crates/render-engine/src/transition.rs`
and `renderer.rs`). See [DECISIONS.md](DECISIONS.md) V1 for why.

## Speed

A clip's `duration` is always time on the timeline. At 2x, a 5 second clip shows 10 seconds of media
(`clip.source_len()`).

```rust
use timeline_engine::*;

// Same media, so the clip gets shorter. Clips after it close up (Ripple::Track or Sync) or stay put (Off).
ed.set_speed(seq, clip, Speed::new(2, 1).unwrap(), SpeedMode::KeepMedia, Ripple::Track, LinkMode::Follow)?;

// Same length on the timeline, so it shows more media (or less).
ed.set_speed(seq, clip, Speed::parse("50%").unwrap(), SpeedMode::KeepLength, Ripple::Off, LinkMode::Follow)?;
```

* `KeepMedia` (the usual "rate stretch") shortens or lengthens the clip. The new length is rounded **down** to whole
  frames, so the clip never needs more media than it had. Without ripple it fails with `Collision` rather than grow
  into the next clip, and nothing changes.
* `KeepLength` keeps the clip where it is and fails with `OutsideSource` if it would need media the file does not have.
* Linked clips (the sound that goes with a picture) follow, unless `LinkMode::Ignore`.
* Split, trim, roll, slide, overwrite and insert all understand speed: cut a 2x clip at 2 seconds and the right part
  starts 4 seconds further into the media.

**Picture.** The renderer reads the frame at the media time the clip maps to. Slow motion repeats frames and fast
motion skips them; there is no frame blending yet ([ROADMAP.md](ROADMAP.md)).

**Sound.** Sound is resampled, so speeding up raises the pitch and slowing down lowers it, like a tape. Between
samples the value is interpolated with a Catmull-Rom curve (a straight ramp stays a straight ramp). There is no
anti-aliasing filter when speeding up, so very fast clips can sound harsh. Pitch-preserving time stretch belongs to
the Audio engine.

## Reverse

```rust
ed.set_reverse(seq, clip, true, LinkMode::Follow)?;
```

The clip keeps its place, its length and the stretch of media it shows (`source_in` is always the *early* end of
that stretch); it just plays from its last moment to its first. Frame by frame it is an exact mirror: the first
frame shown is the last frame of the stretch. Reverse and speed combine. Trimming the head of a reversed clip removes
media from the end of its stretch, and trimming the tail removes it from the start. The code handles this, so edits
behave the way the picture on screen suggests.

Reversed video is slower to render than forward video because the decoder has to seek for every frame.

## Transitions

A transition belongs to the **incoming** clip and blends the clip before it into this one. The two clips must touch
on the same track.

```rust
ed.set_transition(seq, incoming_clip, Some(Transition {
    kind: TransitionKind::Dissolve,       // or DipToBlack, DipToWhite, Wipe(Direction), Push(Direction)
    duration: Time::from_secs_f64(1.0),   // moved to whole frames, at most 60 s
    alignment: Alignment::Center,         // or StartAtCut, EndAtCut
}), LinkMode::Follow)?;
ed.set_transition(seq, incoming_clip, None, LinkMode::Follow)?;   // remove it
```

While a transition is under way **both clips play at once**: the outgoing clip carries on past its end and the
incoming clip plays from before its start. That needs spare media ("handles"):

| Alignment | Outgoing clip needs | Incoming clip needs |
|---|---|---|
| `Center` (half each side of the cut) | half the length after its end | half the length before its start |
| `StartAtCut` (runs from the cut onwards) | the whole length after its end | nothing |
| `EndAtCut` (finishes at the cut) | nothing | the whole length before its start |

Without handles, `set_transition` fails with `BadTransition` and says which clip is short. Stills can be held for
any length, so they never run out.

**If a later edit makes a transition impossible, it is removed, and the edit still happens.** Moving a clip away
(they no longer touch), trimming away the spare media, reversing a clip whose handle was on the wrong side: the
transition goes, and undo brings it back with the clip. The command line prints a note when this happens.
This is [DECISIONS.md](DECISIONS.md) V8.

With `LinkMode::Follow`, the sound clips linked to the clip get a crossfade of the same length and alignment.
Audio only takes the crossfade: asking for any other kind on an audio clip gives a dissolve.

### The looks

| Kind | What you see |
|---|---|
| `Dissolve` | Cross fade. Transparent areas contribute no colour, so letterbox bars do not tint the picture |
| `DipToBlack`, `DipToWhite` | The outgoing picture fades to the colour, then the incoming one fades up from it |
| `Wipe(Left/Right/Up/Down)` | The incoming picture is revealed by an edge sweeping in from the named side, with a soft edge about 1% wide |
| `Push(Left/Right/Up/Down)` | The incoming picture arrives from the named side and pushes the outgoing one out the opposite side |

Progress is measured to the middle of each frame, so the first frame of a transition is never a bare outgoing
picture and the last is never a bare incoming one. Everything before and after the transition is the plain clip,
exactly.

On a track above another, a transition covers what is underneath like any other layer.

## Command line

```bash
originfx timeline p.originfx speed clip8 2                 # 2x, clip gets shorter
originfx timeline p.originfx speed clip8 50% --keep-length # half speed, same length on the timeline
originfx timeline p.originfx speed clip8 3/2 --ripple      # clips after it follow
originfx timeline p.originfx reverse clip8                 # --off to play forwards again
originfx timeline p.originfx transition clip11 dissolve --length 1
originfx timeline p.originfx transition clip11 wipe-left --length 0.5 --align start
originfx timeline p.originfx transition clip11 --remove
originfx timeline p.originfx show                          # shows speed, reverse and transitions
originfx render p.originfx out.mp4                         # renders all of it
```

Transition names: `dissolve`, `dip-black`, `dip-white`, `wipe-left`, `wipe-right`, `wipe-up`, `wipe-down`,
`push-left`, `push-right`, `push-up`, `push-down`.

## Project files

The file format is now version 2. Version 1 files load unchanged (normal speed, forwards, no transitions). Files are
validated on load: a speed of zero, an unknown transition, or a transition without its handles is refused with a
clear error. A version 2 file is refused by older builds, which is intended: they would silently play it wrong.

## How it is tested

* Timeline: exact edits on retimed clips (split, trim, overwrite, speed change with and without ripple, linked
  sound), transitions accepted and refused, transitions dropped by later edits and restored by undo, file round
  trips, version 1 files, damaged files. The random-edit test now includes speed, reverse and transitions and
  checks every rule after every edit, and that undo returns to the start.
* Render: a test source whose pictures and sound say which media time they came from, so mappings are checked
  exactly: forward, 2x, reversed, half speed, the transition windows for all three alignments, the media both
  clips read during a transition, the blends, equal-power crossfade levels, and that sound read in pieces equals
  sound read at once. A real-file test checks a 2x reversed clip frame by frame and its exported length.
* The blends themselves have unit tests (endpoints exact, alpha handled, wipe and push geometry).
