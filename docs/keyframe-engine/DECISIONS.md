# Decisions made while building the Keyframe Engine

Defaults chosen while building this engine, each with the way to reverse it. Decisions that apply to the whole project are in
[../DECISIONS.md](../DECISIONS.md). Earlier engines: [Timeline T1 to T18](../timeline-engine/DECISIONS.md),
[Render R1 to R18](../render-engine/DECISIONS.md), [Video Editing V1 to V24](../video-editing/DECISIONS.md),
[Audio A1 to A16](../audio-engine/DECISIONS.md).

| # | Decision | Why | Reverse it by |
|---|----------|-----|---------------|
| K1 | The curve maths is a separate crate with no dependencies; times are plain `i64` ticks and values `f64` | It can be tested and reused (curve editors, motion graphics) without a timeline, and it does not need to know what the numbers mean | Move `Curve` into the timeline crate |
| K2 | A curve replaces the fixed value; changing the fixed value while animated is an error | Two sources for one value is how projects end up playing something nobody chose. The message tells you to clear the keys | `Animated` checks in `ops.rs` |
| K3 | Only three interpolations: hold, linear, smooth | These cover volume rides, pans and speed ramps. More shapes (eases, bezier handles) can be added to `Interp` without a file change, since names are strings | `Interp` in `curve.rs` |
| K4 | `smooth` is a cubic Hermite with automatic slopes: zero at the ends, zero at turning points, a monotone average elsewhere | The curve never overshoots its keys, so a smooth speed ramp cannot go backwards or past its range, and a smooth fade cannot boost above its keys | `tangent_at` in `curve.rs` |
| K5 | Key slopes can be stored but nothing in the edit API sets them yet | Cutting a curve needs to freeze slopes at the cut, so the field exists; handle editing needs an interface to be useful | Add a `set_tangent` edit |
| K6 | Gain is interpolated in decibels | A straight line in dB is an even-sounding fade; in linear level it sounds like it drops all at once | Convert in `Clip::gain_db_at` and the renderer |
| K7 | Before the first key and after the last the value is held | The least surprising behaviour, and it makes cutting and trimming exact | `Curve::eval` |
| K8 | Clip curves are measured from the clip's start; track and master curves from the sequence's start | Keys on a clip should travel with it; a track's volume ride belongs to the programme, not to whichever clip is under it | `Clip::gain_db_at`, `Track::gain_db_at` |
| K9 | Track and master keys do not move when clips are moved or ripple-edited | Same reason; ripple-aware automation is a feature of its own | See [ROADMAP.md](ROADMAP.md) |
| K10 | The first key set on an unanimated value also puts a key at the start holding the old fixed value | Setting one key must not change the part of the clip before it | `key_into` in `ops.rs` |
| K11 | Cutting a curve is exact: new keys are placed on the cut with the exact value and slope | Splitting a clip must never change how it looks or sounds | `Curve::slice` |
| K12 | Speed ramps use the exact integral of the speed curve to find how much media has played, rounded to a tick | A ramp that drifts from its own definition would desync picture and sound; the integral of every segment type has a closed form | `Clip::media_len` in `model.rs` |
| K13 | The speed range stays 0.01x to 100x at every key; smooth curves cannot overshoot, so it holds between keys too | Same limits as a fixed speed | `Param::range` |
| K14 | A speed key follows links, a gain or pan key does not | A ramp must keep picture and sound in step; gain and pan are per clip | `place_of` in `ops.rs` |
| K15 | A ramp that would run past the end of the media is refused, not clipped | The same rule as for every other edit; silent clipping would change the clip's length without being asked | `validate` in `model.rs` |
| K16 | `set_speed` is refused while the speed is animated | It would silently throw the ramp away | `set_speed` in `ops.rs` |
| K17 | Gain, pan and master curves are evaluated for every sample while mixing | Smooth, with no stepping noise; the cost is small next to decoding | `audio_samples` in `renderer.rs` |
| K18 | Project file format version 4; older files load unchanged; no animation writes nothing; numbers are read back exactly (`float_roundtrip`) | Old projects keep working; a saved ramp must reload to the same media positions | `FORMAT_VERSION` in `file.rs`; the `serde_json` feature |
