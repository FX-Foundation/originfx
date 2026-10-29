# Decisions made while building the Video Editing Engine

Defaults chosen while building this engine, each with the way to reverse it. Decisions that apply to the whole project are in
[../DECISIONS.md](../DECISIONS.md). The edit rules these build on are in
[../timeline-engine/DECISIONS.md](../timeline-engine/DECISIONS.md) (T1 to T18) and the rendering rules in
[../render-engine/DECISIONS.md](../render-engine/DECISIONS.md) (R1 to R18).

| # | Decision | Why | Reverse it by |
|---|----------|-----|---------------|
| V1 | No crate of its own: the data and edits are in the Timeline Engine, the blending is in the Render Engine | Speed and transitions are properties of clips (so every edit must understand them) and of rendering (so the renderer must). A third crate would have to reach into both and would not be able to keep edits valid | Move `retime.rs` and `transition.rs` into a new crate that both depend on |
| V2 | Speed is an exact reduced fraction (`u32 / u32`), from 1/100 to 100 | 50%, 250% and 3/2 never drift, and equal fractions compare equal. Floats would make "same speed" a guess | `Speed` in `retime.rs` |
| V3 | A clip's `duration` is always timeline time; the media it shows is `duration x speed` | Everything that places clips (overlap, ripple, gaps, markers) keeps working on timeline time with no change | `Clip::source_len` |
| V4 | `source_in` is always the early end of the media stretch, for forward and reverse clips alike | One rule for validation and for "does it fit the file". Only the direction of playback differs | `Clip::source_time_at` |
| V5 | Reverse is a flag, and each frame is the exact mirror of the forward one (the first frame shown is the last frame of the stretch) | A reversed clip then covers precisely the same media as before, with no half-frame shift | `Clip::frame_source_time` |
| V6 | Every edit that cuts or moves an edge goes through one function, `Clip::window(start, end)`, which sets `source_in` for the new edges | Forward, reversed and sped-up clips each need a different rule for which media an edge exposes. One function means one place to get it right (split, trim, roll, slide and overwrite all use it) | `Clip::window` in `model.rs` |
| V7 | `set_speed` has two modes: keep the media (length changes, rounded down to frames) or keep the length | Both are common in editors. Rounding down means a clip never needs more media than it had | `SpeedMode` |
| V8 | A transition that an edit makes impossible is **removed**, and the edit still succeeds; undo restores it | The alternative is every move, trim and delete failing with "this would break a transition", which makes editing miserable. The command line prints a note | `prune_transitions` in `model.rs`, called from `Editor::apply` |
| V9 | `set_transition` itself fails clearly if the transition cannot work (not touching, no spare media, too long) | Here the user asked for exactly this, so a silent drop would be wrong | `transition_problem` in `model.rs` |
| V10 | A transition belongs to the incoming clip (`transition_in`) and needs the previous clip to touch it | One owner means no two records can disagree, and moving or splitting the incoming clip carries it along | `Clip::transition_in` |
| V11 | Three alignments (centre, start at cut, end at cut); a centred transition puts the half rounded down to whole frames before the cut | Covers what editors actually need. Whole frames keep the blend on the frame grid | `Transition::before_cut` in `retime.rs` |
| V12 | Transitions last at most 60 seconds | Lets the audio mixer look a fixed distance for clips that overlap the range it is mixing | `MAX_TRANSITION_SECS` |
| V13 | Both transition clips read media beyond their edges; the check is that the media exists in the file | This is what a real transition does. Freezing the end frames instead would be wrong for moving footage | `Clip::media_covers` |
| V14 | Progress is measured to the middle of each frame | The first frame of a transition is never a bare outgoing picture and the last is never a bare incoming one | `video_layers_at` in `query.rs` |
| V15 | A transition is rendered by drawing each side onto its own transparent canvas, blending the two, and putting the result over the layers below | Letterbox bars, stills and different sized clips need no special cases, and the transition covers lower tracks like any other layer | `Renderer::frame_at` |
| V16 | Dissolve blends in premultiplied space | A transparent pixel contributes no colour, so bars and fringes do not tint the picture | `lerp_px` in `transition.rs` |
| V17 | Wipes have a soft edge about 1% of the frame wide; pushes are hard | A hard wipe shimmers as it moves; a push is two rigid pictures | `wipe` in `transition.rs` |
| V18 | Sound transitions are equal-power crossfades (sin and cos), and any other kind asked for on audio becomes a dissolve | A straight fade dips in level in the middle; equal power keeps it steady. Wipes make no sense for sound | `fade_in_gain` and `fade_out_gain` |
| V19 | Sound speed and reverse work by resampling with Catmull-Rom interpolation, so pitch follows speed, and there is no anti-aliasing filter | It is exact for reverse at normal speed (sample for sample), correct for ramps, simple and fast. Quality time-stretching is the Audio engine's job | `clip_audio` in `renderer.rs` |
| V20 | Slow motion repeats frames; there is no frame blending | Honest and exact; blending and optical flow are listed in the roadmap | See [ROADMAP.md](ROADMAP.md) item 2 |
| V21 | Linked clips follow speed, reverse and transitions (`LinkMode::Follow`), like every other edit | Picture and sound stay in sync | `LinkMode::Ignore` |
| V22 | Media may overshoot a file's length by 4 ticks (about 16 picoseconds) | Rounding in speed arithmetic must not reject a clip that ends exactly at the end of its file | `SOURCE_SLACK` in `model.rs` |
| V23 | Project file format version 2; version 1 files load as normal speed with no transitions; older builds refuse version 2 | Old files keep working. An old build that ignored the new fields would play the project wrong without saying so | `FORMAT_VERSION` in `file.rs` |
| V24 | Slip moves `source_in` by an amount of *media* time, not timeline time | "Show 2 seconds later in the file" is what slip means, whatever the speed | `ops::slip` |
