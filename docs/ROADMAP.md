# Media Engine roadmap and known limitations

## Decided: cinema RAW is a future plugin, not core

RED, ARRI and Blackmagic RAW need each vendor's SDK, which are separate
downloads with their own licences, and FFmpeg cannot decode them. They are
removed from the engine's scope. Once the Plugin Engine exists they can ship as
optional plugins, by us or by contributors who own those cameras. Until then,
users convert RAW to ProRes or DNxHR first, which is what many pros do anyway.

## Next, in order

1. **GPU encode.** GPU *decode* is done (CUDA/NVDEC and VA-API, tested on an
   RTX 3050 Laptop and Intel iHD; see `DecodeOptions`). Next is NVENC for the
   H.264/H.265 presets, then VA-API / QSV. AMD and Apple can be written but
   nobody here can test them; mark them untested and let contributors with
   that hardware verify.
2. **Zero-copy hand-off to wgpu.** Only meaningful once the Render Engine can
   receive GPU frames. Today decoded frames are RGBA in system memory.
3. **Project interchange**, once the Timeline Engine exists: EDL, then
   OpenTimelineIO, then FCPXML, then AAF last.

## Known limitations (honest list)

* **Colour management.** Decode and export honour the clip's YUV matrix and
  range and tag exports; 10-bit sources stay 10-bit on H.265/VP9/AV1. Still
  missing: HDR tone mapping for the preview, wide-gamut conversion, and 12-bit
  output. Full colour management belongs in the Render Engine, where the
  decoder should hand over native YUV plus colour metadata.
* **Variable frame rate** is handled exactly by `FrameReader` through a frame
  timestamp index (built automatically when the container reports VFR).
  Export still conforms to constant frame rate.
* **Audio:** surround is kept up to 8 channels (MP3 mixes down to stereo), one
  track is exported at a time, and mixing several tracks is the Audio Engine's job.
* **No subtitle or chapter handling** beyond listing subtitle streams in probe.
* **Long GIFs** (over 20 s) use a palette per frame to keep memory flat, so
  colours can flicker slightly.
* **Trimming** works on export (start/end). Cutting and joining clips is the Timeline Engine's job.
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
