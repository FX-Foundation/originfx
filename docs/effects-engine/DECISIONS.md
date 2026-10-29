# Decisions made while building the Effects Engine

Defaults chosen while building this engine, each with the way to reverse it. Decisions that apply to the whole project are in
[../DECISIONS.md](../DECISIONS.md). Earlier engines: [Timeline T1 to T18](../timeline-engine/DECISIONS.md),
[Render R1 to R18](../render-engine/DECISIONS.md), [Video Editing V1 to V24](../video-editing/DECISIONS.md),
[Audio A1 to A16](../audio-engine/DECISIONS.md), [Keyframe K1 to K18](../keyframe-engine/DECISIONS.md).

| # | Decision | Why | Reverse it by |
|---|----------|-----|---------------|
| E1 | The pixel maths is a separate crate with no dependencies; the timeline depends on it only for the effect names and settings | The effects can be tested, benchmarked and later moved to the GPU without a timeline; the project only needs to know what a setting is called and its range | Move `Kind` into the timeline crate |
| E2 | An effect's settings are plain `f64`s in the order of `Kind::params()`, with name, unit, default, range and neutral value in one table | One table drives the command line, validation, the file format and (later) a settings panel; adding an effect adds one entry | `params()` in `lib.rs` |
| E3 | Every effect has a neutral setting that changes no bits, and neutral effects are skipped | An effect you added but did not move must not alter the picture; skipping them saves time | `is_neutral` in `lib.rs` |
| E4 | Sizes (blur radius, sharpen radius, pixelate size) are a percent of the picture height | The same look at preview size and export size | `apply` in `lib.rs` |
| E5 | Colour effects work on the sRGB values as stored, except exposure, which works in linear light | That is what editors do and what people expect from brightness, contrast and saturation; exposure is defined by light, so it is physically right | `colour.rs` |
| E6 | Colour effects are done with 256-entry tables or one 3x3 matrix per pixel, rounded once at the end | Fast, exact at the ends (black and white stay), and the same result everywhere | `colour.rs` |
| E7 | Blur is three box passes (a close match to a gaussian) on premultiplied floats, with the edges extended | Its cost does not grow with the radius, and transparent shapes do not gain dark or bright edges | `blur.rs` |
| E8 | Sharpen is an unsharp mask on colour only; alpha is unchanged and the result is kept within the pixel's own alpha | No halos outside shapes, no invented transparency | `sharpen` in `blur.rs` |
| E9 | Effects are a stack on the clip, applied in order, at most 32 per clip, on picture clips only | The order matters and people expect to change it; a cap keeps frames from taking forever; sound effects are a different, stateful problem | `MAX_EFFECTS`; `NotPicture` check in `ops.rs` |
| E10 | An effect's id is unique within its clip, not across the project | A cut clip's halves can keep the same effects without making new ids, and the id is only ever used together with the clip | `EffectId` in `effects.rs` |
| E11 | A setting is either fixed or animated, never both; the first key starts from the fixed value | Same rules as the other animated values (K2, K10) | `ops.rs` |
| E12 | The renderer draws the clip on its own transparent canvas, applies the stack, then composites | Blur, vignette and pixelate must not mix with the layers beneath; it also makes effects work inside transitions | `draw_clip` in `renderer.rs` |
| E13 | Effects are applied at the sequence's size, after the clip is fitted to the frame | The picture the viewer sees is what the effect changes (a vignette follows the frame, not the original file) | `draw_clip` |
| E14 | Project file format version 5; older files load unchanged; settings are saved by name | Old projects keep working; names survive adding settings to an effect later | `FORMAT_VERSION` in `file.rs` |
| E15 | Effects run on the CPU, one thread | Correct and the same everywhere first; a GPU path can match it later | See [ROADMAP.md](ROADMAP.md) |
