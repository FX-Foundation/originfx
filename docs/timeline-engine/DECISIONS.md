# Decisions made while building the Timeline Engine

Defaults chosen while building this engine, each with the way to reverse it. Decisions that apply to
the whole project are in [../DECISIONS.md](../DECISIONS.md).

| # | Decision | Why | Reverse it by |
|---|----------|-----|---------------|
| T1 | Time is an `i64` count of ticks, 254,016,000,000 per second | Divides exactly into every standard video frame rate and audio sample rate, so there is no rounding drift; floats and fractions both fail this | Change `TICKS_PER_SECOND` in `time.rs` (the tick-exactness tests show what breaks) |
| T2 | Frame rates are exact fractions (24000/1001 for 23.976) | The industry rate is a fraction, and a float would break equality and drop-frame maths | `FrameRate` in `time.rs` |
| T3 | Drop-frame timecode only for 29.97 and 59.94 | It does not exist for other rates; asking for it elsewhere is an error, not a silent fallback | `Timecode::from_frame_count` |
| T4 | The project is persistent immutable data (`im` crate), edited through one `Editor` | Undo and redo become "swap the old project back": no inverse operations to get wrong, and snapshots for other threads are free | Replace the maps and vectors in `model.rs`; the `Editor` API stays |
| T5 | Every edit runs on a copy and is validated before it commits | A failed or invalid edit can never leave a half-changed project | `Editor::apply` in `editor.rs` |
| T6 | Everything snaps to whole frames of the sequence | Clips that start or end between frames cause flicker and off-by-one errors everywhere downstream | `Sequence::snap`, called from `ops.rs` |
| T7 | Placing a clip over others overwrites them; moving a clip does the same | It is what editors do by default; inserting is the explicit alternative | `place_clip` and `move_clips` in `ops.rs` |
| T8 | A sync ripple delete is refused if another track has a clip in the gap | Closing it would cut the clip or break sync; refusing is safe and the user can unlock or lift the track | `ripple_close` in `ops.rs` |
| T9 | Picture and sound placed together are *linked*; linked clips edit together unless `LinkMode::Ignore` is given | Matches how footage with sound is edited | `place_asset` in `ops.rs` |
| T10 | When a clip is split, the left part keeps its id and the right part gets a new one; right parts of linked clips share one new link | Ids stay stable for the first half, and sync survives the cut | `split_tracks_at` in `ops.rs` |
| T11 | An asset stores facts about the media, not pixels; stills have no duration | The Timeline Engine stays pure Rust; a still can be held for any length | `Asset` in `model.rs` |
| T12 | The Media Engine is an optional cargo feature (`media`) | Tests and tools that only edit timelines build without FFmpeg | `Cargo.toml` features |
| T13 | Project file is pretty-printed JSON with a `format` name and a `version`; newer versions are refused | Readable, diffable, and safe: an old build never guesses at a newer file | `file.rs` (`FORMAT_VERSION`) |
| T14 | Loading validates everything and fails on the first broken rule | A corrupted file should be reported, not edited into more damage | `Project::from_json` |
| T15 | Saving writes to `<file>.tmp` and renames it into place | A crash or full disk never leaves a half-written project | `Project::save` |
| T16 | Undo keeps at most 10,000 steps | Memory bound; each step is cheap because copies share structure | `UNDO_LIMIT` in `editor.rs` |
| T17 | Ids are unique across the whole project (one counter) and never reused | An id in a log or a file always means the same thing | `Project::next_id` |
| T18 | Command line tool works one edit per run (load, edit, save) | Simple, scriptable, and a failed edit leaves the file untouched | `crates/originfx-cli/src/timeline.rs` |
