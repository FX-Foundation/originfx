# OriginFX

> A professional, cross-platform video editor. Built in Rust. Built by the community.

[![License](https://img.shields.io/badge/license-GPL%203.0-blue.svg)](LICENSE)
[![Foundation](https://img.shields.io/badge/backed%20by-FX%20Foundation-gold.svg)](https://github.com/FX-Foundation)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org)

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

> OriginFX is in early development. The first engine is done; the rest are being designed.

**The Media Engine is built and tested.** It is a Rust library plus a small `originfx`
command-line tool that opens any video, reports what is inside it, jumps to any frame
exactly, caches frames, writes 19 export formats, makes proxies, carries subtitles and
chapters, and can use the GPU (NVIDIA CUDA and NVENC, VA-API) when asked. Software output
is the default and GPU use is always opt-in.

- [x] Project structure and workspace layout
- [x] Media import/decode layer (FFmpeg integration): the [Media Engine](docs/MEDIA_ENGINE.md)
- [ ] Core timeline data model
- [ ] Renderer pipeline (GPU-accelerated)
- [ ] Plugin/extension system
- [ ] UI framework selection

**Try it** (Ubuntu 24.04; full steps in [docs/MEDIA_ENGINE.md](docs/MEDIA_ENGINE.md#2-quick-start)):

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
