# Audio Engine

Engine 5 of 10. The sound side of OriginFX: how loud, where in the stereo picture, how it fades, how the final
mix is kept clean, and how loud the result is.

Crate: `crates/audio-engine` (package `originfx-audio-engine`, library `audio_engine`). It is pure sound maths
with no dependencies: it takes interleaved `f32` samples and gives back samples or numbers, and knows nothing of
timelines or files. The sound settings that belong to clips, tracks and sequences are stored by the
[Timeline Engine](../timeline-engine/README.md); the [Render Engine](../render-engine/README.md) applies them when
it mixes. See [DECISIONS.md](DECISIONS.md) A1.

## What you can set

| Where | Setting | Range |
|---|---|---|
| Audio clip | gain | -120 dB (silence) to +24 dB, in steps of 0.01 dB |
| Audio clip | pan | -100 (hard left) to 100 (hard right) |
| Audio clip | fade in, fade out | any length that fits in the clip; curve `linear`, `equal-power` or `smooth` |
| Audio track | gain, pan | same as a clip; applied to the sum of the track's clips |
| Sequence | master gain | same; applied to the whole mix |
| Render | limiter | `HardClip` (default), `SoftClip`, or `Off` for measuring |

```rust
use timeline_engine::*;

ed.set_clip_audio(seq, &[clip], ClipAudio {
    gain: Gain::from_db(-6.0),
    pan: Pan::new(-25),
    fade_in:  Some(Some(Fade { duration: Time::from_secs_f64(1.0), curve: FadeCurve::Smooth })),
    fade_out: Some(None),            // Some(None) removes a fade; None leaves it alone
    ..Default::default()
}, LinkMode::Follow)?;
ed.update_track(seq, track, TrackUpdate { gain: Gain::from_db(-3.0), ..Default::default() })?;
ed.set_master_gain(seq, Gain::from_db(-1.0).unwrap())?;
```

Everything is an ordinary edit: undoable, saved in the project, refused cleanly if it makes no sense (a fade longer
than its clip, sound settings on a picture clip). With `LinkMode::Follow` a linked picture is simply skipped.

## How the mix is built

For every sample, in this order:

1. The clip's media, at its speed and direction ([Video Editing Engine](../video-editing/README.md)).
2. The clip's own **fades**, then any **transition** crossfade.
3. The clip's **gain** and **pan**.
4. All the clips of a track add up on that track's **bus**.
5. The bus gets the **track's** gain and pan.
6. Tracks add up (mute and solo decide which tracks take part).
7. The sum gets the **master** gain.
8. The **limiter** keeps it inside full scale.

A clip with neutral settings changes no bits. And the mix of any range is the same whatever blocks it is cut into,
so players and exporters can read it in any size of pieces.

### Pan

Pan is a **balance** control for stereo material. In the centre both channels are untouched (exactly 1.0, no loss of
level). Moving away lowers the far side along a constant-power curve while the near side stays at full level; at
hard left the right channel is silent. Mono sources arrive already spread to both channels, so panning them
works the same way.

### Fades

| Curve | Shape |
|---|---|
| `linear` | Level rises in a straight line |
| `equal-power` | Power rises in a straight line (a quarter sine). Two crossing fades keep a steady level; this is what transitions use |
| `smooth` | A raised cosine: gentle at both ends. A good default for music |

Fades are measured on the timeline from the clip's edges. When a clip is cut, the new edges get no fade (a fade in the
middle of a sentence is never what you wanted). When a clip is trimmed shorter than its fades, the fade out gives way first, then the fade in.

### Limiters

* `HardClip` (default): exact below full scale, flat above. This is what earlier versions did.
* `SoftClip`: untouched below 0.8, then bent smoothly towards 1.0 and never above it. Rounds loud peaks instead of
  flat-topping them.
* `Off`: no limiting. For measurement.

Both limiters judge one sample at a time, which is what keeps block-by-block mixing exact. They are safety nets; a
real look-ahead limiter needs state across blocks and is on the [roadmap](ROADMAP.md).

## Loudness

`audio_engine::measure` and `LoudnessMeter` implement **ITU-R BS.1770-4 / EBU R128**: K-weighting, 400 ms blocks every
100 ms, the -70 LUFS absolute gate and the -10 LU relative gate. You get the integrated loudness in LUFS, the loudest
400 ms block, the sample peak and the RMS. The meter accepts audio in pieces of any size, uses a few numbers per
second of programme, and gives the same answer however the audio is cut up. It is checked against the standard's
filter coefficients and its reference case (a stereo sine at -23 dBFS reads -23.0 LUFS), at 44.1, 48 and 96 kHz.

Channels are weighted equally, right for mono and stereo; surround weighting is not done yet.

`audio_engine::peaks` turns samples into min/max buckets for drawing a waveform.

## Command line

```bash
originfx timeline p.originfx gain clip9 -6            # also: +3, -inf
originfx timeline p.originfx pan clip9 -40
originfx timeline p.originfx fade clip9 --in 1 --out 2 --curve smooth    # 0 removes a fade
originfx timeline p.originfx track 1 --gain -3 --pan 10                   # audio track 1
originfx timeline p.originfx master -1.5
originfx timeline p.originfx master --normalize -23   # measure the mix, set the master gain to hit -23 LUFS
originfx loudness p.originfx                           # loudness of a project's mix ...
originfx loudness interview.wav                        # ... or of any media file
originfx render p.originfx out.mp4 --limiter soft
```

`master --normalize` measures the mix without limiting, then sets the master gain so the integrated loudness hits the
target. It warns when the peaks would then go above full scale (use `--limiter soft` or a lower target). Common
targets: -23 LUFS (EBU broadcast), -16 LUFS (podcasts and most streaming), -14 LUFS (music streaming).

## Project files

Format version 3 (now 4, which adds [keyframes](../keyframe-engine/README.md)). Versions 1 and 2 load unchanged (every level at 0 dB, centre, no fades). Out-of-range gains or pans,
or unknown fade curves, are refused with a clear error on load.

## How it is tested

* Audio crate: dB conversion, the pan law, every fade curve (endpoints, never falling, equal-power sums), limiters
  (exact, smooth, bounded), peaks, and the loudness meter against the standard's coefficients and reference level,
  at three sample rates, with gating of silence and of much quieter passages, in pieces versus whole, mono and stereo.
* Timeline: setting, undoing and refusing sound settings, cuts and trims and their effect on fades, track and master
  levels, files round-trip and are refused when damaged. The random-edit test now includes sound settings.
* Render: gain, pan, fades (at half way, in the middle, at the end, in pieces versus whole), track and master levels
  multiplying, track pan, the limiters, and that neutral settings change no bits.
* End to end: a project normalised to -23 LUFS measures -23.0 LUFS, and the exported WAV measures the same.
