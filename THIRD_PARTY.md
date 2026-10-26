# Third-party software

OriginFX is licensed under the **GNU General Public License v3.0 or later**
(see [`LICENSE`](LICENSE)). It builds on the software below. This file is a
good-faith summary for people packaging or redistributing OriginFX, not legal advice;
check each project's own licence before you ship binaries.

## FFmpeg and codec libraries (system libraries, not copied into this repository)

The Media Engine links to FFmpeg 6.1 and its libraries at build time. They are not
vendored here. What you must comply with depends on how your FFmpeg is built:

| Component | Licence | Why it matters |
|---|---|---|
| FFmpeg (libavcodec, libavformat, libavutil, libswscale, libswresample, libavfilter) | LGPL-2.1+, or GPL-2+ when built with GPL parts | Ubuntu's FFmpeg is built with `--enable-gpl`, so the combined program must be GPL. OriginFX is GPL-3.0-or-later, which is compatible. |
| x264 (H.264 encoder) | GPL-2.0-or-later | The reason OriginFX is GPL. A build that ships x264 must be GPL. |
| x265 (H.265 encoder) | GPL-2.0-or-later | Same. |
| libvpx (VP8/VP9), libopus, libvorbis, libmp3lame | BSD-3-Clause, BSD-3-Clause, BSD-3-Clause, LGPL-2.1+ | Permissive or LGPL; fine in a GPL program. |
| SVT-AV1, libaom, libdav1d (AV1) | BSD-3-Clause-Clear / BSD-2-Clause | Permissive. AV1 has its own patent licence from the Alliance for Open Media. |
| NVIDIA NVENC/NVDEC headers (nv-codec-headers) | MIT | Only the headers are used; the GPU driver is the user's own. |

Do not build OriginFX against `--enable-nonfree` FFmpeg (for example with fdk-aac);
the result cannot be redistributed.

**Patents are separate from copyright licences.** H.264 and H.265 are covered by patent
pools (MPEG LA / Via LA, Access Advance and others). They matter if you sell or distribute
a product commercially at scale, not for building and using the software. Take advice
before shipping a commercial binary.

## Rust crates

Resolved from `Cargo.lock` (58 crates, all compatible with GPL-3.0). Regenerate the list
with `cargo metadata --format-version 1 --locked`.

| Crate | Version | Licence |
|---|---|---|
| aho-corasick | 1.1.5 | Unlicense OR MIT |
| allocator-api2 | 0.2.21 | MIT OR Apache-2.0 |
| anstream | 1.0.0 | MIT OR Apache-2.0 |
| anstyle | 1.0.14 | MIT OR Apache-2.0 |
| anstyle-parse | 1.0.0 | MIT OR Apache-2.0 |
| anstyle-query | 1.1.5 | MIT OR Apache-2.0 |
| anstyle-wincon | 3.0.11 | MIT OR Apache-2.0 |
| bindgen | 0.64.0 | BSD-3-Clause |
| bitflags | 1.3.2 | MIT/Apache-2.0 |
| cc | 1.10.0 | MIT OR Apache-2.0 |
| cexpr | 0.6.0 | Apache-2.0/MIT |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 |
| clang-sys | 1.9.1 | Apache-2.0 |
| clap | 4.6.7 | MIT OR Apache-2.0 |
| clap_builder | 4.6.7 | MIT OR Apache-2.0 |
| clap_derive | 4.6.7 | MIT OR Apache-2.0 |
| clap_lex | 1.1.1 | MIT OR Apache-2.0 |
| colorchoice | 1.0.5 | MIT OR Apache-2.0 |
| equivalent | 1.0.3 | Apache-2.0 OR MIT |
| ffmpeg-next | 6.1.1 | WTFPL |
| ffmpeg-sys-next | 6.1.0 | WTFPL |
| find-msvc-tools | 0.1.15 | MIT OR Apache-2.0 |
| foldhash | 0.1.5 | Zlib |
| glob | 0.3.4 | MIT OR Apache-2.0 |
| hashbrown | 0.15.5 | MIT OR Apache-2.0 |
| heck | 0.5.0 | MIT OR Apache-2.0 |
| hermit-abi | 0.5.3 | MIT OR Apache-2.0 |
| is_terminal_polyfill | 1.70.2 | MIT OR Apache-2.0 |
| lazy_static | 1.5.1 | MIT OR Apache-2.0 |
| lazycell | 1.3.0 | MIT/Apache-2.0 |
| libc | 0.2.191 | MIT OR Apache-2.0 |
| libloading | 0.8.9 | ISC |
| lru | 0.12.5 | MIT |
| memchr | 2.8.3 | Unlicense OR MIT |
| minimal-lexical | 0.2.1 | MIT/Apache-2.0 |
| nom | 7.1.3 | MIT |
| num_cpus | 1.17.0 | MIT OR Apache-2.0 |
| once_cell_polyfill | 1.70.2 | MIT OR Apache-2.0 |
| peeking_take_while | 0.1.2 | Apache-2.0/MIT |
| pkg-config | 0.3.34 | MIT OR Apache-2.0 |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 |
| quote | 1.0.47 | MIT OR Apache-2.0 |
| regex | 1.13.1 | MIT OR Apache-2.0 |
| regex-automata | 0.4.18 | MIT OR Apache-2.0 |
| regex-syntax | 0.8.11 | MIT OR Apache-2.0 |
| rustc-hash | 1.1.0 | Apache-2.0/MIT |
| shlex | 1.3.0 | MIT OR Apache-2.0 |
| shlex | 2.0.1 | MIT OR Apache-2.0 |
| strsim | 0.11.1 | MIT |
| syn | 1.0.109 | MIT OR Apache-2.0 |
| syn | 3.0.9 | MIT OR Apache-2.0 |
| thiserror | 2.0.21 | MIT OR Apache-2.0 |
| thiserror-impl | 2.0.21 | MIT OR Apache-2.0 |
| unicode-ident | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| utf8parse | 0.2.2 | Apache-2.0 OR MIT |
| vcpkg | 0.2.15 | MIT/Apache-2.0 |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 |
