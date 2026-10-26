# Licensing

* OriginFX is **GPL-3.0-or-later** (`LICENSE`, `THIRD_PARTY.md`, an SPDX header on
  every source file, `license` in `Cargo.toml`). Decided because FFmpeg builds that
  include libx264 / libx265 are GPL, so a shipped binary must be GPL anyway.
* No contributor agreement (CLA) is used: contributors keep their copyright and
  license under the same GPL. Relicensing to closed source later would need every
  contributor's consent, which is the price of that choice.
* The crates stay `publish = false` until a first release is cut.
* H.264/H.265 patent pools are a separate matter from copyright; get advice before
  shipping a commercial binary. This is not legal advice.

The list of third-party software and its licences is in [THIRD_PARTY.md](../THIRD_PARTY.md).
