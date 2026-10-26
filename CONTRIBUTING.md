# Contributing to OriginFX

Thank you for your interest in contributing to OriginFX. This project is built by the community, for the community.

---

## Before You Start

- Read the [README](README.md) to understand the project vision
- Browse [open issues](https://github.com/FX-Foundation/originfx/issues) to find something to work on
- Check the [RFC repository](https://github.com/FX-Foundation/rfcs) for ongoing architectural discussions
- Join the conversation in [Discussions](https://github.com/FX-Foundation/originfx/discussions)

---

## How to Contribute

### Reporting Bugs

Open an issue with:
- A clear title and description
- Steps to reproduce
- Expected vs actual behavior
- Your OS and environment details

### Suggesting Features

Open a Discussion first. Large features should go through the [RFC process](https://github.com/FX-Foundation/rfcs).

### Submitting Code

1. Fork the repository
2. Create a branch: `git checkout -b your-feature-name`
3. Write your code. Write tests.
4. Run the checks below
5. Open a pull request with a clear description. CI runs the same checks and must pass.

### Building and testing

You need Rust (stable) and the FFmpeg 6.1 development libraries (Ubuntu 24.04 ships them).
The install line is in the [Media Engine quick start](docs/media-engine/README.md#2-quick-start).
The tests also use the `ffmpeg` command-line tool to generate their own clips; no video
files are stored in the repository.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### Writing Documentation

Documentation lives alongside the code, in [`docs/`](docs), with one folder per engine. Clear docs are as valuable
as good code. If you change what the software does, update the page that describes it in
the same pull request.

---

## Code Standards

- Follow Rust idioms and the official [Rust Style Guide](https://doc.rust-lang.org/nightly/style-guide/)
- Run `cargo fmt` before committing
- No warnings allowed — `cargo clippy` must pass clean
- Write tests for new functionality, and a test that fails without the fix for every bug fix
- Anything `unsafe` needs a `SAFETY:` comment explaining why it is sound
- The public API of the engines must not expose FFmpeg types

---

## Licence of contributions

OriginFX is licensed under the [GNU General Public License v3.0 or later](LICENSE). By
submitting a contribution you agree that it is released under the same licence. There is no
contributor agreement to sign, and you keep the copyright on what you write. Add the
one-line header `// SPDX-License-Identifier: GPL-3.0-or-later` to every new source file.

---

## Code of Conduct

Be respectful. Be constructive. See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

---

*FX Foundation — Built by the world.*
