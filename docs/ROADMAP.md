# Media Engine roadmap and known limitations

## Decided: cinema RAW is a future plugin, not core

RED, ARRI and Blackmagic RAW need each vendor's SDK, which are separate
downloads with their own licences, and FFmpeg cannot decode them. They are
removed from the engine's scope. Once the Plugin Engine exists they can ship as
optional plugins, by us or by contributors who own those cameras. Until then,
users convert RAW to ProRes or DNxHR first, which is what many pros do anyway.

## Next, in order

1. **GPU decode and encode (needs a real GPU).** NVIDIA first (NVDEC/NVENC via
   FFmpeg `cuda`), then Intel (VA-API / QSV). The groundwork exists:
   `originfx hw` reports what a machine exposes and `originfx bench` measures
   decode speed, so GPU paths can be compared against the software baseline.
   AMD and Apple can be written but nobody here can test them; mark them
   untested and let contributors with that hardware verify.
2. **Zero-copy hand-off to wgpu.** Only meaningful once the Render Engine can
   receive GPU frames. Today decoded frames are RGBA in system memory.
3. **Project interchange**, once the Timeline Engine exists: EDL, then
   OpenTimelineIO, then FCPXML, then AAF last.

## Known limitations (honest list)

* **Colour management.** Decode converts YUV to RGBA with FFmpeg's defaults
  (BT.601, limited range assumed). HD/UHD footage mastered as BT.709 or HDR
  will look slightly off in `FrameReader` output, and exports do not yet write
  explicit colour tags (primaries, transfer, range). Proper colour management
  belongs in the Render Engine; the decoder should then hand over native YUV
  plus colour metadata instead of RGBA.
* **Variable frame rate.** `FrameReader` maps time to frame index with the
  nominal frame rate, which is exact for constant-rate footage and approximate
  for VFR. Export always conforms to constant frame rate.
* **Audio export is stereo or mono.** Surround sources are downmixed.
* **No subtitle or chapter handling** beyond listing subtitle streams in probe.
* **FLAC export** has no total-sample count in its header (FFmpeg's muxer
  rejects the final metadata packet through this API), so some tools show no
  duration. Audio data is complete and decodes fine.
* **GIF export** buffers the palette over the whole clip; fine for short
  clips, memory-hungry for long ones.
* **Trimming/cutting** is deliberately absent. That is the Timeline Engine's job.
* **Hardware encoders are not used**, so exports are CPU-bound. On the 2-core
  cloud machine this was built on, 1080p to 720p H.264 at the Standard preset
  ran at roughly 2x real time; decode with RGBA conversion ran at ~270 fps.

## Licensing: needs a decision before the first release

* The repository has **no LICENSE file yet** and the crates are marked
  `publish = false` so nothing asserts a licence on your behalf.
* FFmpeg builds that include **libx264 / libx265 are GPL**. Shipping a binary
  that links such a build generally makes the whole binary GPL-compatible-only.
  Options: release OriginFX under GPL-3.0 (fits an open-source mission), or link
  an LGPL-only FFmpeg and ship x264/x265 support separately. This is a real
  legal question; check with someone qualified before releasing binaries.
