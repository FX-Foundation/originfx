# OriginFX roadmap

The ten engines are built one at a time, in this order. Each engine lives in its own crate under
`crates/` and has its own folder under `docs/` once it exists.

| # | Engine | Status | Docs |
|---|--------|--------|------|
| 1 | Media | **Done and tested** | [docs/media-engine](media-engine/README.md) |
| 2 | Timeline | **Done and tested** | [docs/timeline-engine](timeline-engine/README.md) |
| 3 | Render | **Done and tested** | [docs/render-engine](render-engine/README.md) |
| 4 | Video Editing | **Done and tested** | [docs/video-editing](video-editing/README.md) |
| 5 | Audio | **Done and tested** | [docs/audio-engine](audio-engine/README.md) |
| 6 | Keyframe | **Done and tested** | [docs/keyframe-engine](keyframe-engine/README.md) |
| 7 | Effects | **Done and tested** | [docs/effects-engine](effects-engine/README.md) |
| 8 | Compositing | Planned | n/a |
| 9 | Motion Graphics | Planned | n/a |
| 10 | Plugin | Planned | n/a |

A Resource Governor, which keeps the engines within the machine's memory and CPU/GPU budget, sits alongside them. It is also planned.

Each engine's own roadmap, known limits and decisions are kept with that engine, for example the
[Media Engine roadmap](media-engine/ROADMAP.md). Licensing is covered once for the whole project in
[LICENSING.md](LICENSING.md), and project-wide decisions are in [DECISIONS.md](DECISIONS.md).
