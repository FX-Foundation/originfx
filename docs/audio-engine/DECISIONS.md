# Decisions made while building the Audio Engine

Defaults chosen while building this engine, each with the way to reverse it. Decisions that apply to the whole project are in
[../DECISIONS.md](../DECISIONS.md). Earlier engines: [Timeline T1 to T18](../timeline-engine/DECISIONS.md),
[Render R1 to R18](../render-engine/DECISIONS.md), [Video Editing V1 to V24](../video-editing/DECISIONS.md).
R8 ("sound is a plain sum, then clipped") is superseded by A6 and A7 below.

| # | Decision | Why | Reverse it by |
|---|----------|-----|---------------|
| A1 | The sound maths is a separate crate with no dependencies; the Timeline Engine depends on it only for the fade curve names | The maths can be tested and reused (meters, waveforms, a future player) without a timeline. The settings still belong to the timeline, because they are edited and saved with it | Move `FadeCurve` into the timeline crate |
| A2 | Gain is stored as a whole number of hundredths of a decibel, from -120.00 to +24.00; pan as a whole number of percent | Exact comparison, exact saving, no drift from repeated edits, and the model's `Eq` works. -120 dB is silence (no `-inf` in files) | `Gain` and `Pan` in `audio.rs` |
| A3 | Maximum boost is +24 dB | Enough for quiet recordings; more is almost always a mistake, and the limiter is a net, not a cure | `Gain::MAX_DB` |
| A4 | Pan is a balance law: centre is exactly 1.0 on both sides, the far side falls on a constant-power curve | Untouched clips stay bit-exact and keep their level; panning only ever turns things down, so it cannot cause clipping | `pan_gains` in `gain.rs` |
| A5 | Fades are stored as a duration and a curve per end of a clip, and measured from the clip's own edges | What editors expect, and independent of speed and transitions | `Clip::fade_in` and `fade_out` |
| A6 | Mix order: clip fades and transition, clip gain and pan, track bus gain and pan, sum, master gain, limiter | Matches a mixing desk, so the numbers mean what an engineer expects | `Renderer::audio_samples` |
| A7 | Limiters work on one sample at a time (hard clip is the default, soft clip and off are options) | A mix of any range is then the same however it is cut into blocks, which players and exporters rely on. Hard clipping keeps the old behaviour | `Limiter` in `limit.rs`; a look-ahead limiter needs the stateful path in the [roadmap](ROADMAP.md) |
| A8 | A cut removes the fade at the new edge; a trim keeps fades and shortens them only when they no longer fit (fade out first) | A fade in the middle of a take is never intended; shrinking silently on trim would change what the user set | `split_tracks_at` and `clear_range` in `ops.rs`; `fit_fades` in `model.rs` |
| A9 | Sound settings on a picture clip are refused; a linked picture is skipped | Pictures have no sound settings, and linked pairs should still be editable from either half | `set_clip_audio` in `ops.rs` |
| A10 | Track gain and pan exist only on audio tracks | Same reason | `update_track` in `ops.rs` |
| A11 | Loudness follows BS.1770-4 / EBU R128 with integrated loudness only; no loudness range, short-term or true peak yet | Integrated loudness is what deliverable targets (-23, -16, -14 LUFS) are written in; the others need more state and a UI to be useful | See [ROADMAP.md](ROADMAP.md) item 6 |
| A12 | The K-weighting filters are computed for any sample rate from the standard's analogue parameters | The published coefficients are for 48 kHz only; the formulas reproduce them exactly at 48 kHz (tested) and work at 44.1, 96 and the rest | `k_weighting` in `loudness.rs` |
| A13 | The meter weights all channels equally | Correct for mono and stereo, the only layouts the mixer produces | See [ROADMAP.md](ROADMAP.md) item 7 |
| A14 | The loudness of a project is measured on the mix with the limiter off | Measuring after clipping hides how hot a mix really is, and normalising to a clipped measurement under-corrects | `measure_sequence` in `crates/originfx-cli/src/audio.rs` |
| A15 | `master --normalize` warns when the peaks would then exceed full scale, instead of refusing or limiting | The target is a number the user chose; the fix (soft limiter, lower target) is theirs | `Cmd::Master` in `timeline.rs` |
| A16 | Project file format version 3; older files load with neutral sound settings; zero values are not written | Old projects keep working; files stay small and readable | `FORMAT_VERSION` in `file.rs` |
