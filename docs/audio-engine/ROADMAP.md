# Audio Engine roadmap

## Done

* Gain, pan, fades on clips; gain and pan on tracks; master gain
* Three fade curves, equal-power crossfades for transitions (from the Video Editing engine)
* Hard clip, soft clip and no-limit modes
* EBU R128 integrated loudness, loudest block, peak and RMS, streaming
* Waveform peaks
* Normalise the mix to a loudness target
* Command line: `gain`, `pan`, `fade`, `track`, `master`, `loudness`

## Next, in order

1. **A stateful render path.** Equaliser, compressor, noise gate and look-ahead limiter all remember what they heard a
   moment ago, so they cannot be computed for a range in isolation. The plan: a sequential mixer that renders from a
   start point with a warm-up run before it, keeping random access for everything else. This is the main piece of work
   behind items 2 to 4.
2. **Equaliser** (parametric, high and low pass), per clip and per track.
3. **Compressor, gate, de-esser, and a look-ahead true-peak limiter** for the master.
4. **Noise reduction and hum removal.**
5. **Automatic ducking** (music under voice). Keyframed volume and pan (rubber-band lines) are done: see the [Keyframe Engine](../keyframe-engine/README.md).
6. **Short-term loudness, loudness range (LRA) and true peak** meters, and a live meter for playback.
7. **Surround and other channel layouts**, with correct loudness weighting and downmix.
8. **Pitch-preserving time stretch** for clips with a changed speed (today pitch follows speed like a tape).
9. **Anti-aliasing filter** when sound is sped up.
10. **Waveform cache.** Generate peaks once per file (the function exists) and store them beside the proxies.
11. **Sample-rate conversion quality.** The Media Engine decodes and resamples with FFmpeg's defaults.

## Belongs to other engines

| Item | Engine |
|---|---|
| Animated effect parameters | Keyframe (volume and pan are done) |
| Effects as plug-ins (VST/CLAP hosting, custom DSP) | Plugin |
| Sound for generated graphics and titles | Motion Graphics |
| Playing sound live, with sync to picture | the player, once it exists |
