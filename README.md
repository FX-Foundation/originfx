# OriginFX

> A professional, cross-platform video editor. Built in Rust. Built by the community.

[![License](https://img.shields.io/badge/license-GPL%203.0-blue.svg)](LICENSE)
[![Foundation](https://img.shields.io/badge/backed%20by-FX%20Foundation-gold.svg)](https://github.com/FX-Foundation)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org)
**UI designs:** [Browse the OriginFX interface designs](https://github.com/FX-Foundation/originfx/tree/design/ui)
---

## The Vision

Video editing software has been locked behind expensive subscriptions and proprietary ecosystems for too long.

OriginFX is the answer.

A fully open-source, professional-grade video editor built from scratch in Rust — with the performance, stability, and feature depth that creators actually need. No subscriptions. No telemetry. No compromises.

This is not a side project. This is a movement.

---

## Why Rust?

- **Performance**: Real-time preview, fast export, no GC pauses. Rust gives us bare-metal speed without sacrificing safety.
- **Reliability**: Memory safety by default. No crashes from dangling pointers or data races.
- **Cross-platform**: One codebase. Linux, Windows, macOS — all first-class citizens.
- **Future-proof**: Built for the hardware of today and tomorrow.

---

## Current Status

> OriginFX is in early development. The first three engines are done; the rest are being designed.

**The Media Engine is built and tested.** It is a Rust library plus a small `originfx`
command-line tool that opens any video, reports what is inside it, jumps to any frame
exactly, caches frames, writes 19 export formats, makes proxies, carries subtitles and
chapters, and can use the GPU (NVIDIA CUDA and NVENC, VA-API) when asked. Software output
is the default and GPU use is always opt-in.

**The Timeline, Render, Video Editing, Audio, Keyframe, Effects, Compositing, Motion Graphics and Plugin engines are built and tested too.** You can build a
project from the command line (`originfx timeline`), edit it with undo and redo, and render it
to any of the 19 formats with `originfx render`, or save one picture with `originfx render-frame`.
Clips can be sped up, slowed down and reversed, and joined with dissolves, dips, wipes and pushes.
Sound has gain, pan, fades, track and master levels and a loudness meter (`originfx loudness`, `master --normalize`).
Values can change over time with keyframes: volume and pan automation and speed ramps (`originfx timeline key`).
Picture clips take a stack of effects: colour, blur, sharpen, vignette and more (`originfx timeline effect`).
Layers have opacity, blend modes, position, scale, rotation, crop and masks, all animatable (`originfx timeline look|blend|mask`).
Graphics are drawn rather than filmed: solid colours, gradients, shapes and text (`originfx timeline graphic`).
Plugins (sandboxed WebAssembly) add new effects to the effect stack (`originfx plugin`).

### Engines

OriginFX is built as ten engines, one at a time, in this order. Each has its own crate and its own docs folder.

| # | Engine | Status | Docs |
|---|--------|--------|------|
| 1 | Media | **Done and tested** | [Guide](docs/media-engine/README.md) |
| 2 | Timeline | **Done and tested** | [Guide](docs/timeline-engine/README.md) |
| 3 | Render | **Done and tested** | [Guide](docs/render-engine/README.md) |
| 4 | Video Editing | **Done and tested** | [Guide](docs/video-editing/README.md) |
| 5 | Audio | **Done and tested** | [Guide](docs/audio-engine/README.md) |
| 6 | Keyframe | **Done and tested** | [Guide](docs/keyframe-engine/README.md) |
| 7 | Effects | **Done and tested** | [Guide](docs/effects-engine/README.md) |
| 8 | Compositing | **Done and tested** | [Guide](docs/compositing-engine/README.md) |
| 9 | Motion Graphics | **Done and tested** | [Guide](docs/motion-graphics-engine/README.md) |
| 10 | Plugin | **Done and tested** | [Guide](docs/plugin-engine/README.md) |

The full roadmap is in [docs/ROADMAP.md](docs/ROADMAP.md).

**Try it** (Ubuntu 24.04; full steps in [docs/media-engine](docs/media-engine/README.md#2-quick-start)):

```bash
git clone https://github.com/FX-Foundation/originfx && cd originfx
cargo build --release
./target/release/originfx probe ~/Videos/clip.mp4
```

This is the moment to get involved. The decisions made now will shape everything.

---

## Contributing

OriginFX is community-driven. Every contribution matters — code, design, documentation, testing, ideas.

Read [CONTRIBUTING.md](CONTRIBUTING.md) to get started.

**We especially need:**
- Rust developers (all levels)
- GPU/rendering engineers
- UI/UX designers
- Technical writers
- People who care

---

## Governance

OriginFX is stewarded by the **FX Foundation**, an open-source foundation founded and backed by [Origin Empire](https://originempire.xyz).

Architectural decisions go through the [RFC process](https://github.com/FX-Foundation/rfcs). Major decisions are made in the open.

See [GOVERNANCE.md](https://github.com/FX-Foundation/governance) for full details.

---

## License

OriginFX is licensed under the [GNU General Public License v3.0 or later](LICENSE). Third-party components are listed in [THIRD_PARTY.md](THIRD_PARTY.md).

---

*FX Foundation — Backed by Origin Empire. Built by the world.*
