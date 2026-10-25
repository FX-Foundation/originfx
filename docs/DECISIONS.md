# Decisions made while building the Media Engine overnight

You were away, so these were made with the sensible default. Every one can be
reversed; none touched your existing files.

| # | Decision | Why | Reverse it by |
|---|----------|-----|---------------|
| 1 | Work on branch `claude/media-engine`, not `main` | Your `main` stays untouched until you review | Merge the branch, or delete it |
| 2 | Cargo workspace at the repo root, crates under `crates/` | Room for the other 9 engines as sibling crates | Move folders |
| 3 | FFmpeg 6.1 via the `ffmpeg-next 6.1` crate | Matches Ubuntu 24.04's FFmpeg; the newest crate versions target FFmpeg 7+ | Bump both together |
| 4 | Decode output is RGBA8 in system memory | Simplest correct contract for now; GPU/YUV paths come with the Render Engine | See ROADMAP |
| 5 | Export is always constant frame rate | What editors and delivery formats expect | Remove the `fps` filter |
| 6 | Cinema RAW removed from engine scope | Your call: future plugin | See ROADMAP |
| 7 | The 19 export presets are my own list | Your notes say "19 export formats" but not which; this covers the common web, editing, and audio formats | Edit `preset.rs` (one table) |
| 8 | No licence declared (`publish = false`) | Choosing a licence is yours, and FFmpeg+x264 affects it | Add LICENSE, remove `publish = false` |
| 9 | Hardware acceleration is probe-only | No GPU in the build environment; untested GPU code is worse than none | See ROADMAP |
| 10 | Did not touch the root `README.md`, `CONTRIBUTING.md` or `CODE_OF_CONDUCT.md` | They are your voice | n/a |
| 11 | A stray test branch `claude/push-test` exists on GitHub (one empty commit) | I could not delete it from here | Delete it in GitHub's Branches page |
