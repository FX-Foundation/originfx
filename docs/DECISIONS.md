# Project-wide decisions

Decisions that apply to the whole of OriginFX, each with the way to reverse it. Decisions that belong to one engine live with that engine, for example [Media Engine decisions](media-engine/DECISIONS.md).

| # | Decision | Why | Reverse it by |
|---|----------|-----|---------------|
| P1 | Work happens on feature branches and reaches `main` through pull requests that pass CI | `main` always builds and passes its tests | n/a |
| P2 | Cargo workspace at the repo root, crates under `crates/` | Room for the other 9 engines as sibling crates | Move folders |
| P3 | Licence: GPL-3.0-or-later, no CLA; crates stay `publish = false` until a release | Chosen by the owner. FFmpeg builds with x264/x265 are GPL, so a GPL app is the simple, honest fit. "or later" is the FSF's usual recommendation. Not publishing to crates.io yet avoids an accidental release | `LICENSE`, `Cargo.toml`; see [LICENSING.md](LICENSING.md) |
| P4 | The root `README.md`, `CONTRIBUTING.md` and `CODE_OF_CONDUCT.md` are the owner's voice; they were left alone until 26 Oct 2026, then updated for facts only (status, build steps, licence) at the owner's request | Keeps the project's tone consistent | n/a |
| P5 | Merged and test branches are deleted from GitHub after use | Keeps the branch list short; the cloud tooling used here cannot delete remote branches, so the owner does it in GitHub's Branches page | n/a |
