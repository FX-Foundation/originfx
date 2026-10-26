# Media Engine: roadmap and known limitations

This page covers the Media Engine only. The whole-project order of work is in [../ROADMAP.md](../ROADMAP.md).

## Decided: cinema RAW is a future plugin, not core

RED, ARRI and Blackmagic RAW need each vendor's SDK, which are separate
downloads with their own licences, and FFmpeg cannot decode them. They are
removed from the engine's scope. Once the Plugin Engine exists they can ship as
optional plugins, by us or by contributors who own those cameras. Until then,
users convert RAW to ProRes or DNxHR first, which is what many pros do anyway.

## Next, in order

1. **Wider import testing.** Generate a clip for each less common container and codec (AVI, MXF, FLV, MPG, M2TS, 3GP, OGV; MPEG-2, MPEG-4, DNxHD, DV; 4:2:2 and 4:4:4) and check probe, decode and seek on each. Today only the common set is tested.
2. **More GPU encoders.** GPU decode (CUDA/NVDEC, VA-API) and NVENC encoding
   for the H.264/H.265 presets are done and tested on an RTX 3050 Laptop. Next:
   NVENC AV1 (RTX 40 and newer), VA-API / QSV encoding, and decoding on the GPU
   during export. AMD and Apple can be written but nobody here can test them;
   mark them untested and let contributors with that hardware verify.
3. **Zero-copy hand-off to wgpu.** Only meaningful once the Render Engine can
   receive GPU frames. Today decoded frames are RGBA in system memory.
4. **Project interchange**, once the Timeline Engine exists: EDL, then
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
* **Subtitles are text only.** SRT, ASS/SSA, WebVTT and mov_text can be read and
  carried into MKV and MP4/MOV exports (with chapters); styling and positioning are
  dropped, picture subtitles (PGS, DVD, DVB) are skipped, and WebM and AVI outputs
  cannot hold them. Burning subtitles into the picture belongs in the Render Engine.
* **VA-API cannot decode 4K AV1 on the Intel iGPU it was tested on.** `Auto` falls back
  to software and reports why. Untested on other GPUs.
* **Long GIFs** (over 20 s) use a palette per frame to keep memory flat, so
  colours can flicker slightly.
* **Trimming** works on export (start/end). Cutting and joining clips is the Timeline Engine's job.
* **Hardware encoding is NVENC only** (H.264/H.265, opt-in), so other exports are CPU-bound. On the 2-core
  cloud machine this was built on, 1080p to 720p H.264 at the Standard preset
  ran at roughly 2x real time; decode with RGBA conversion ran at ~270 fps.
