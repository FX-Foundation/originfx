//! Software transcoding: read any supported file, write it out with an
//! [`ExportPreset`].
//!
//! The pipeline is the classic one: demux -> decode -> filter graph (scale,
//! frame-rate conform, pixel/sample format) -> encode -> mux. Output is always
//! constant frame rate, which is what editors and most delivery formats want.

use std::path::Path;
use std::time::{Duration, Instant};

use ffmpeg::format::Pixel;
use ffmpeg::media::Type;
use ffmpeg::{codec, filter, format, frame, picture, Dictionary, Packet, Rational};
use ffmpeg_next as ffmpeg;

use crate::decode::start_offset_secs;
use crate::error::{Error, Result};
use crate::hwaccel::{HwApi, HwPolicy};
use crate::preset::{find_encoder, AudioSpec, ExportPreset, Quality, VideoSpec};
use crate::probe::stream_rotation;

/// Everything configurable about an export.
#[derive(Debug, Clone)]
pub struct ExportSettings {
    pub preset: ExportPreset,
    pub quality: Quality,
    /// Scale to this height keeping the aspect ratio. Never upscales.
    pub height: Option<u32>,
    /// Output frame rate; `None` keeps the source rate.
    pub fps: Option<f64>,
    /// Video bit rate in bits/s. Overrides the quality-based setting.
    pub video_bitrate: Option<u64>,
    /// Audio bit rate in bits/s for lossy audio codecs.
    pub audio_bitrate: Option<u64>,
    /// Distance between keyframes in frames (smaller = faster scrubbing, bigger files).
    pub keyint: Option<u32>,
    /// Put the index at the front of MP4/MOV files so they stream and open instantly.
    pub fast_start: bool,
    /// Keep only the part of the clip from this time (seconds from the start).
    pub start_secs: Option<f64>,
    /// Keep only the part of the clip up to this time (seconds from the start).
    pub end_secs: Option<f64>,
    /// Bits per colour channel: 8 or 10. `None` keeps 10-bit sources at 10-bit for
    /// H.265, VP9 and AV1 and writes 8-bit otherwise. Asking for 10 explicitly also
    /// works for H.264 (High 10 profile, which many devices cannot play).
    pub bit_depth: Option<u8>,
    /// Container stream index of the video stream to use (see `probe`); `None` picks the best one.
    pub video_stream: Option<usize>,
    /// Container stream index of the audio track to use; `None` picks the default track.
    pub audio_stream: Option<usize>,
    /// GPU video encoding (NVIDIA NVENC) for the H.264 and H.265 presets.
    /// Defaults to [`HwPolicy::Off`] (software). With [`HwPolicy::Auto`] the export
    /// quietly runs in software when NVENC cannot be used, including when it fails
    /// part-way (the export then restarts in software, so progress starts again).
    pub hw_encode: HwPolicy,
    /// Copy the source's text subtitle tracks into the output. Supported for MKV
    /// (as SRT) and MP4/MOV (as mov_text); other formats skip them and say so in
    /// [`ExportReport::notes`]. Text only: styling and positioning are dropped, and
    /// picture subtitles (PGS, DVD) are skipped. Off by default.
    pub subtitles: bool,
    /// Copy chapter markers into the output (MKV, MP4, MOV, M4A). Off by default.
    pub chapters: bool,
    /// Testing aid: make the GPU encoder fail after this many frames.
    #[doc(hidden)]
    pub simulate_gpu_failure_after: Option<u64>,
}

impl ExportSettings {
    pub fn new(preset: ExportPreset) -> Self {
        Self {
            preset,
            quality: Quality::Standard,
            height: None,
            fps: None,
            video_bitrate: None,
            audio_bitrate: None,
            keyint: None,
            fast_start: true,
            start_secs: None,
            end_secs: None,
            bit_depth: None,
            video_stream: None,
            audio_stream: None,
            hw_encode: HwPolicy::Off,
            subtitles: false,
            chapters: false,
            simulate_gpu_failure_after: None,
        }
    }

    pub fn quality(mut self, quality: Quality) -> Self {
        self.quality = quality;
        self
    }

    pub fn height(mut self, height: u32) -> Self {
        self.height = Some(height);
        self
    }

    pub fn fps(mut self, fps: f64) -> Self {
        self.fps = Some(fps);
        self
    }

    pub fn video_bitrate(mut self, bits_per_sec: u64) -> Self {
        self.video_bitrate = Some(bits_per_sec);
        self
    }

    pub fn audio_bitrate(mut self, bits_per_sec: u64) -> Self {
        self.audio_bitrate = Some(bits_per_sec);
        self
    }

    pub fn keyint(mut self, frames: u32) -> Self {
        self.keyint = Some(frames);
        self
    }

    /// Export only `start..end` (seconds from the start of the clip). Either
    /// side may be `None` to keep the beginning or the end.
    pub fn trim(mut self, start_secs: Option<f64>, end_secs: Option<f64>) -> Self {
        self.start_secs = start_secs;
        self.end_secs = end_secs;
        self
    }

    /// Force 8-bit or 10-bit video output (see [`ExportSettings::bit_depth`]).
    pub fn bit_depth(mut self, bits: u8) -> Self {
        self.bit_depth = Some(bits);
        self
    }

    /// Encode video on the GPU (see [`ExportSettings::hw_encode`]).
    pub fn hw_encode(mut self, policy: HwPolicy) -> Self {
        self.hw_encode = policy;
        self
    }

    /// Carry text subtitle tracks into the output (see [`ExportSettings::subtitles`]).
    pub fn subtitles(mut self, on: bool) -> Self {
        self.subtitles = on;
        self
    }

    /// Carry chapter markers into the output (see [`ExportSettings::chapters`]).
    pub fn chapters(mut self, on: bool) -> Self {
        self.chapters = on;
        self
    }

    /// Use a specific video stream (container index, as listed by `probe`).
    pub fn video_stream(mut self, index: usize) -> Self {
        self.video_stream = Some(index);
        self
    }

    /// Use a specific audio track (container index, as listed by `probe`).
    pub fn audio_stream(mut self, index: usize) -> Self {
        self.audio_stream = Some(index);
        self
    }

    fn trim_range(&self) -> Result<Option<(f64, Option<f64>)>> {
        let start = self.start_secs.unwrap_or(0.0);
        if !start.is_finite() || start < 0.0 {
            return Err(Error::InvalidArgument(
                "trim start must be 0 or more".into(),
            ));
        }
        if let Some(end) = self.end_secs {
            if !end.is_finite() || end <= start {
                return Err(Error::InvalidArgument(
                    "trim end must be after the start".into(),
                ));
            }
        }
        if self.start_secs.is_none() && self.end_secs.is_none() {
            return Ok(None);
        }
        Ok(Some((start, self.end_secs)))
    }

    /// ffmpeg `trim` arguments (without the filter name), or empty.
    fn trim_args(&self) -> String {
        let mut parts = Vec::new();
        if let Some(s) = self.start_secs.filter(|s| *s > 0.0) {
            parts.push(format!("start={s:.6}"));
        }
        if let Some(e) = self.end_secs {
            parts.push(format!("end={e:.6}"));
        }
        parts.join(":")
    }
}

/// Progress snapshot handed to the callback while exporting.
#[derive(Debug, Clone, Copy)]
pub struct Progress {
    /// How far into the source we are, in seconds.
    pub position_secs: f64,
    /// Source duration if known.
    pub total_secs: Option<f64>,
    /// Video frames written so far.
    pub frames: u64,
}

impl Progress {
    /// 0.0 to 1.0 when the total is known.
    pub fn fraction(&self) -> Option<f64> {
        self.total_secs
            .filter(|t| *t > 0.0)
            .map(|t| (self.position_secs / t).clamp(0.0, 1.0))
    }
}

/// What an export produced.
#[derive(Debug, Clone)]
pub struct ExportReport {
    pub frames: u64,
    pub video_encoder: Option<String>,
    pub audio_encoder: Option<String>,
    pub duration_secs: f64,
    pub output_bytes: u64,
    pub elapsed: Duration,
    /// Why GPU encoding was requested but the video was encoded in software.
    pub hw_fallback_reason: Option<String>,
    /// Subtitle tracks written to the output.
    pub subtitle_tracks: usize,
    /// Chapter markers written to the output.
    pub chapters: usize,
    /// Things that were asked for but not done, in plain words (for example a
    /// subtitle track that could not be carried over).
    pub notes: Vec<String>,
}

/// Export without progress reporting.
pub fn export(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    settings: &ExportSettings,
) -> Result<ExportReport> {
    export_with_progress(input, output, settings, |_| true)
}

/// Export and report progress. Return `false` from the callback to cancel;
/// the partial output file is deleted and [`Error::Cancelled`] is returned.
pub fn export_with_progress(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    settings: &ExportSettings,
    on_progress: impl FnMut(&Progress) -> bool,
) -> Result<ExportReport> {
    crate::init();
    let (input, output) = (input.as_ref(), output.as_ref());
    if !input.exists() {
        return Err(Error::NotFound(input.to_path_buf()));
    }
    settings.preset.check_available()?;

    let mut on_progress = on_progress;
    // With GPU encoding off, nothing GPU-related is touched (not even a device probe).
    let gpu = if settings.hw_encode == HwPolicy::Off {
        Ok(None)
    } else {
        match (settings.hw_encode, nvenc_encoder(settings)) {
            (_, Ok(name)) => Ok(Some(name)),
            (HwPolicy::Require(_), Err(reason)) => return Err(Error::Unsupported(reason)),
            (_, Err(reason)) => Err(reason),
        }
    };
    let result = match gpu {
        Ok(None) => run_export(input, output, settings, None, &mut on_progress),
        Err(reason) => {
            run_export(input, output, settings, None, &mut on_progress).map(|r| ExportReport {
                hw_fallback_reason: Some(reason),
                ..r
            })
        }
        Ok(Some(name)) => match run_export(input, output, settings, Some(name), &mut on_progress) {
            Err(e) if settings.hw_encode == HwPolicy::Auto && !matches!(e, Error::Cancelled) => {
                // Anything going wrong with NVENC is retried in software from the
                // start; a problem that is not the GPU's fails again there.
                let _ = std::fs::remove_file(output);
                run_export(input, output, settings, None, &mut on_progress).map(|r| ExportReport {
                    hw_fallback_reason: Some(format!("{name} failed: {e}")),
                    ..r
                })
            }
            other => other,
        },
    };
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

/// The NVENC encoder for this export, or why it cannot be used.
fn nvenc_encoder(settings: &ExportSettings) -> std::result::Result<&'static str, String> {
    use ExportPreset::*;
    if let HwPolicy::Require(HwApi::Vaapi) = settings.hw_encode {
        return Err("GPU encoding supports NVIDIA (cuda) only so far, not vaapi".into());
    }
    let name = match settings.preset {
        Mp4H264 | MovH264 | MkvH264 => "h264_nvenc",
        Mp4H265 | MkvH265 => "hevc_nvenc",
        other => {
            return Err(format!(
                "GPU encoding covers the H.264 and H.265 presets, not {}",
                other.label()
            ))
        }
    };
    if name == "h264_nvenc" && settings.bit_depth == Some(10) {
        return Err("NVENC cannot write 10-bit H.264".into());
    }
    if ffmpeg::encoder::find_by_name(name).is_none() {
        return Err(format!("this FFmpeg build has no {name}"));
    }
    if !HwApi::Cuda.is_usable() {
        return Err("no usable CUDA device".into());
    }
    Ok(name)
}

// ---------------------------------------------------------------------------

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 {
        a.abs()
    } else {
        gcd(b, a % b)
    }
}

fn rational_from_fps(fps: f64) -> Rational {
    for (n, d) in [
        (24_000, 1001),
        (30_000, 1001),
        (60_000, 1001),
        (120_000, 1001),
    ] {
        if (fps - f64::from(n) / f64::from(d)).abs() < 0.01 {
            return Rational::new(n, d);
        }
    }
    let n = (fps * 1000.0).round() as i64;
    let g = gcd(n, 1000).max(1);
    Rational::new((n / g) as i32, (1000 / g) as i32)
}

fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

fn target_dims(iw: u32, ih: u32, settings: &ExportSettings) -> (u32, u32) {
    let mut h = settings.height.unwrap_or(ih);
    if settings.preset == ExportPreset::Gif && settings.height.is_none() {
        h = h.min(480);
    }
    let h = even(h.min(ih).max(2));
    let w = even(((f64::from(iw) * f64::from(h) / f64::from(ih.max(1))).round() as u32).max(2));
    (w, h)
}

/// Channel layouts to try for the output, best first. We keep the source's
/// channels (up to what the format carries) and fall back to a stereo mix.
/// Several layouts with the same channel count are offered because encoders
/// differ on which one they accept (Opus wants 5.1 "back", AAC takes either).
fn layout_candidates(
    codec: &ffmpeg::codec::Audio,
    source: ffmpeg::ChannelLayout,
    source_channels: u16,
    max_channels: u16,
    prefer_back: bool,
) -> Vec<ffmpeg::ChannelLayout> {
    use ffmpeg::ChannelLayout as L;
    if source_channels <= 1 {
        return vec![L::MONO];
    }
    let mut out: Vec<L> = Vec::new();
    if source_channels > 2 && max_channels > 2 {
        let supported: Option<Vec<L>> = codec.channel_layouts().map(|l| l.collect());
        let accepts = |l: &L| supported.as_ref().is_none_or(|s| s.contains(l));
        let n = source_channels.min(max_channels);
        let mut all = Vec::new();
        if source_channels <= max_channels && !source.is_empty() {
            all.push(source);
        }
        all.push(L::default(i32::from(n)));
        all.extend(match n {
            3 => vec![L::SURROUND],
            4 => vec![L::QUAD, L::_4POINT0],
            5 => vec![L::_5POINT0_BACK, L::_5POINT0],
            6 => vec![L::_5POINT1_BACK, L::_5POINT1],
            8 => vec![L::_7POINT1, L::_7POINT1_WIDE_BACK],
            _ => vec![],
        });
        if prefer_back {
            // Encoders such as Opus only take the "back" variants; try those first
            // so a rejected attempt does not log noise.
            let back = [L::_5POINT1_BACK, L::_5POINT0_BACK, L::_7POINT1, L::QUAD];
            all.sort_by_key(|l| !back.contains(l)); // stable: back variants first
        }
        for l in all {
            if i32::from(n) == l.channels() && accepts(&l) && !out.contains(&l) {
                out.push(l);
            }
        }
    }
    out.push(L::STEREO);
    out
}

/// Bits per channel of a pixel format, judged from its name (`yuv420p10le` is 10).
fn source_bit_depth(fmt: Pixel) -> u8 {
    let name = format!("{fmt:?}").to_lowercase();
    if name.starts_with("p010") {
        return 10;
    }
    let name = name
        .trim_end_matches("le")
        .trim_end_matches("be")
        .to_string();
    for (suffix, bits) in [("16", 16), ("14", 14), ("12", 12), ("10", 10)] {
        if name.ends_with(suffix) && name.starts_with(['y', 'g', 'n']) {
            return bits;
        }
    }
    8
}

/// Output pixel format for this export, honouring the requested bit depth.
fn output_pix_fmt(settings: &ExportSettings, spec: &VideoSpec, src: Pixel) -> Result<&'static str> {
    use ExportPreset::*;
    let ten_bit_capable = matches!(
        settings.preset,
        Mp4H265 | MkvH265 | WebmVp9 | WebmAv1 | Mp4H264 | MovH264 | MkvH264
    );
    let wants_ten = match settings.bit_depth {
        Some(8) => false,
        Some(10) => true,
        Some(other) => {
            return Err(Error::InvalidArgument(format!(
                "bit depth must be 8 or 10, not {other}"
            )))
        }
        None => {
            // Automatic: only where 10-bit is widely playable.
            source_bit_depth(src) > 8
                && matches!(settings.preset, Mp4H265 | MkvH265 | WebmVp9 | WebmAv1)
        }
    };
    if !wants_ten {
        return Ok(spec.pix_fmt);
    }
    if settings.bit_depth == Some(10) && !ten_bit_capable && !spec.pix_fmt.contains("10") {
        return Err(Error::Unsupported(format!(
            "{} cannot be written in 10-bit",
            settings.preset.label()
        )));
    }
    Ok(if ten_bit_capable {
        "yuv420p10le"
    } else {
        spec.pix_fmt
    })
}

fn pixel_from_name(name: &str) -> Result<Pixel> {
    Ok(match name {
        "yuv420p" => Pixel::YUV420P,
        "yuv420p10le" => Pixel::YUV420P10LE,
        "p010le" => Pixel::P010LE,
        "yuvj420p" => Pixel::YUVJ420P,
        "yuv422p" => Pixel::YUV422P,
        "yuv422p10le" => Pixel::YUV422P10LE,
        "yuv444p10le" => Pixel::YUV444P10LE,
        "pal8" => Pixel::PAL8,
        other => return Err(Error::Unsupported(format!("pixel format {other}"))),
    })
}

fn video_options(settings: &ExportSettings, encoder: &str) -> Vec<(&'static str, String)> {
    use Quality::{Draft, High, Standard};
    let q = settings.quality;
    let pick = |draft: &str, standard: &str, high: &str| -> String {
        match q {
            Draft => draft,
            Standard => standard,
            High => high,
        }
        .to_string()
    };
    let by_bitrate = settings.video_bitrate.is_some();
    let mut o: Vec<(&'static str, String)> = Vec::new();
    match encoder {
        "libx264" => {
            o.push(("preset", pick("veryfast", "medium", "slow")));
            if !by_bitrate {
                o.push(("crf", pick("28", "21", "17")));
            }
        }
        "libx265" => {
            o.push(("preset", pick("veryfast", "medium", "slow")));
            if !by_bitrate {
                o.push(("crf", pick("30", "24", "19")));
            }
            o.push(("x265-params", "log-level=error".into()));
            o.push(("tag", "hvc1".into()));
        }
        "h264_nvenc" | "hevc_nvenc" => {
            let hevc = encoder == "hevc_nvenc";
            o.push(("preset", pick("p2", "p5", "p7")));
            o.push(("tune", "hq".into()));
            o.push(("rc", "vbr".into()));
            if !by_bitrate {
                // Constant quality with no bitrate cap, like CRF in x264/x265.
                // Tuned against x264/x265 at the same quality level: similar
                // quality, somewhat larger files (NVENC compresses less well).
                let cq = if hevc {
                    pick("32", "27", "22")
                } else {
                    pick("31", "26", "21")
                };
                o.push(("cq", cq));
                o.push(("b", "0".into()));
                // Without a maximum, NVENC caps itself near 21 Mbit/s, which
                // limits 4K quality whatever `cq` asks for.
                o.push(("maxrate", "400M".into()));
                o.push(("bufsize", "800M".into()));
            }
            if hevc {
                o.push(("tag", "hvc1".into()));
            } else if settings.bit_depth != Some(10) {
                // NVENC picks Main by default; High compresses better and plays everywhere.
                o.push(("profile", "high".into()));
            }
        }
        "libvpx-vp9" => {
            o.push(("deadline", "good".into()));
            o.push(("cpu-used", pick("6", "4", "2")));
            o.push(("row-mt", "1".into()));
            if !by_bitrate {
                o.push(("crf", pick("40", "33", "27")));
                o.push(("b:v", "0".into()));
            }
        }
        "libvpx" => {
            o.push(("deadline", "good".into()));
            o.push(("cpu-used", pick("6", "4", "2")));
            if !by_bitrate {
                o.push(("crf", pick("14", "10", "6")));
                o.push(("b:v", pick("2M", "4M", "8M")));
            }
        }
        "libsvtav1" => {
            o.push(("preset", pick("10", "8", "6")));
            if !by_bitrate {
                o.push(("crf", pick("42", "35", "28")));
            }
        }
        "libaom-av1" => {
            o.push(("cpu-used", pick("8", "6", "4")));
            o.push(("row-mt", "1".into()));
            if !by_bitrate {
                o.push(("crf", pick("42", "35", "28")));
                o.push(("b:v", "0".into()));
            }
        }
        "prores_ks" => {
            let profile = match settings.preset {
                ExportPreset::MovProRes422 => "standard",
                ExportPreset::MovProRes4444 => "4444",
                _ => "hq",
            };
            o.push(("profile", profile.into()));
            o.push(("vendor", "apl0".into()));
        }
        "dnxhd" => o.push(("profile", "dnxhr_hq".into())),
        _ => {}
    }
    o
}

/// Allocate an encoder context *for a specific codec*.
///
/// `Context::new()` allocates without a codec, which skips the codec's default
/// settings; libx264 then rejects the context with "broken ffmpeg default
/// settings detected". Allocating with the codec applies those defaults.
fn new_encoder_context(codec: &ffmpeg::Codec) -> Result<codec::context::Context> {
    // SAFETY: `avcodec_alloc_context3` returns a freshly allocated context or
    // null. We check for null, and `wrap(.., None)` makes the wrapper own it,
    // so it is freed exactly once when dropped.
    unsafe {
        let ptr = ffmpeg::ffi::avcodec_alloc_context3(codec.as_ptr());
        if ptr.is_null() {
            return Err(Error::Unsupported(
                "out of memory allocating encoder".into(),
            ));
        }
        Ok(codec::context::Context::wrap(ptr, None))
    }
}

fn dictionary(pairs: &[(&str, String)]) -> Dictionary<'static> {
    let mut d = Dictionary::new();
    for (k, v) in pairs {
        d.set(k, v);
    }
    d
}

// --- video pipeline ---------------------------------------------------------

struct VideoPipe {
    decoder: ffmpeg::decoder::Video,
    filter: filter::Graph,
    encoder: ffmpeg::encoder::Video,
    in_index: usize,
    out_index: usize,
    in_tb: Rational,
    enc_tb: Rational,
    out_tb: Rational,
    frames: u64,
    last_pts_secs: f64,
    encoder_name: &'static str,
    /// Testing aid, see [`ExportSettings::simulate_gpu_failure_after`].
    fail_after: Option<u64>,
}

impl VideoPipe {
    #[allow(clippy::too_many_arguments)]
    fn new(
        ictx: &format::context::Input,
        octx: &mut format::context::Output,
        in_index: usize,
        settings: &ExportSettings,
        spec: VideoSpec,
        gpu_encoder: Option<&'static str>,
        origin_secs: f64,
        global_header: bool,
    ) -> Result<Self> {
        let ist = ictx.stream(in_index).ok_or(Error::NoStream("video"))?;
        let in_tb = ist.time_base();

        let mut dctx = codec::context::Context::from_parameters(ist.parameters())?;
        dctx.set_threading(ffmpeg::threading::Config {
            kind: ffmpeg::threading::Type::Frame,
            count: 0,
        });
        let decoder = dctx.decoder().video()?;
        let (iw, ih) = (decoder.width(), decoder.height());
        if iw == 0 || ih == 0 {
            return Err(Error::InvalidArgument(
                "video stream has no dimensions".into(),
            ));
        }
        let rotation = stream_rotation(&ist);
        // Dimensions as the viewer sees them, after rotation.
        let (vw, vh) = if rotation % 180 == 90 {
            (ih, iw)
        } else {
            (iw, ih)
        };
        let (ow, oh) = target_dims(vw, vh, settings);

        let avg = ist.avg_frame_rate();
        let src_fps = if avg.numerator() > 0 && avg.denominator() > 0 {
            f64::from(avg)
        } else {
            30.0
        };
        let mut fps_f = settings.fps.unwrap_or(src_fps).clamp(1.0, 240.0);
        if settings.preset == ExportPreset::Gif {
            fps_f = fps_f.min(15.0);
        }
        let fps = if settings.fps.is_none()
            && avg.numerator() > 0
            && avg.denominator() > 0
            && fps_f == src_fps
        {
            Rational::new(avg.numerator(), avg.denominator())
        } else {
            rational_from_fps(fps_f)
        };

        let (codec_found, name) = match gpu_encoder {
            Some(n) => (
                ffmpeg::encoder::find_by_name(n)
                    .ok_or_else(|| Error::Unsupported(format!("encoder {n}")))?,
                n,
            ),
            None => find_encoder(spec.encoders)?,
        };
        let pix_name = output_pix_fmt(settings, &spec, decoder.format())?;
        // NVENC takes 10-bit as P010 (same values, interleaved chroma), not planar.
        let pix_name = match (gpu_encoder, pix_name) {
            (Some(_), "yuv420p10le") => "p010le",
            _ => pix_name,
        };
        let pix = pixel_from_name(pix_name)?;

        let mut ost = octx.add_stream(codec_found)?;
        let out_index = ost.index();
        let mut enc = new_encoder_context(&codec_found)?.encoder().video()?;
        enc.set_width(ow);
        enc.set_height(oh);
        enc.set_format(pix);
        enc.set_time_base(fps.invert());
        enc.set_frame_rate(Some(fps));
        let sar = decoder.aspect_ratio();
        if sar.numerator() > 0 && sar.denominator() > 0 {
            enc.set_aspect_ratio(sar);
        }
        if global_header {
            enc.set_flags(codec::Flags::GLOBAL_HEADER);
        }

        // Colour: keep the source's matrix and tag the output with it, so players
        // do not have to guess (untagged HD is usually guessed right, SD often not).
        let src_fmt = decoder.format();
        let colour = crate::colour::resolve(
            decoder.color_space(),
            decoder.color_range(),
            decoder.color_primaries(),
            decoder.color_transfer_characteristic(),
            iw,
            ih,
            crate::colour::is_full_range_format(src_fmt),
        );
        let out_full = pix_name.starts_with("yuvj");
        let tag_colour = settings.preset != ExportPreset::Gif;
        if tag_colour {
            enc.set_colorspace(colour.space);
            enc.set_color_range(if out_full {
                ffmpeg::color::Range::JPEG
            } else {
                ffmpeg::color::Range::MPEG
            });
            // SAFETY: `enc` owns a valid, not yet opened AVCodecContext; these are
            // plain integer fields that ffmpeg-next has no setter for.
            unsafe {
                let ctx = enc.as_mut_ptr();
                (*ctx).color_primaries = colour.primaries.into();
                (*ctx).color_trc = colour.transfer.into();
            }
        }
        if let Some(g) = settings.keyint {
            enc.set_gop(g);
        }
        if let Some(b) = settings.video_bitrate {
            enc.set_bit_rate(b as usize);
        } else if name == "mjpeg" {
            enc.set_bit_rate(25_000_000);
        }
        let opened = enc.open_as_with(codec_found, dictionary(&video_options(settings, name)))?;
        ost.set_parameters(&opened);
        ost.set_time_base(fps.invert());

        // Filter graph: rebase timestamps, conform to constant fps, scale, convert pixel format.
        let ticks = (origin_secs * f64::from(in_tb.denominator())
            / f64::from(in_tb.numerator().max(1)))
        .round() as i64;
        let rotate = match rotation {
            90 => ",transpose=clock",
            180 => ",hflip,vflip",
            270 => ",transpose=cclock",
            _ => "",
        };
        let trim = settings.trim_args();
        let trim = if trim.is_empty() {
            String::new()
        } else {
            format!("trim={trim},setpts=PTS-STARTPTS,")
        };
        let matrix = crate::colour::filter_matrix_name(colour.space);
        let scale_colour = if !tag_colour {
            String::new()
        } else if crate::colour::is_yuv_format(src_fmt) {
            format!(
                ":in_color_matrix={matrix}:in_range={}:out_color_matrix={matrix}:out_range={}",
                if colour.full_range { "pc" } else { "tv" },
                if out_full { "pc" } else { "tv" }
            )
        } else {
            format!(
                ":out_color_matrix={matrix}:out_range={}",
                if out_full { "pc" } else { "tv" }
            )
        };
        let mut spec_str = format!(
            "setpts=PTS-{ticks},{trim}fps={}/{}{rotate},scale={ow}:{oh}{scale_colour}",
            fps.numerator(),
            fps.denominator()
        );
        if settings.preset == ExportPreset::Gif {
            // One shared palette looks best but needs the whole clip in memory.
            // For longer clips use a palette per frame, which streams.
            let long_clip = ictx.duration() > 20 * i64::from(ffmpeg::ffi::AV_TIME_BASE);
            if long_clip {
                spec_str.push_str(
                    ",split[s0][s1];[s0]palettegen=stats_mode=single[p];[s1][p]paletteuse=new=1:dither=bayer:bayer_scale=5",
                );
            } else {
                spec_str.push_str(
                    ",split[s0][s1];[s0]palettegen=stats_mode=diff[p];[s1][p]paletteuse=dither=bayer:bayer_scale=5",
                );
            }
        } else {
            spec_str.push_str(&format!(",format={pix_name}"));
        }

        let sar_args = if sar.numerator() > 0 && sar.denominator() > 0 {
            format!("{}/{}", sar.numerator(), sar.denominator())
        } else {
            "1/1".to_string()
        };
        let args = format!(
            "video_size={iw}x{ih}:pix_fmt={}:time_base={}/{}:pixel_aspect={sar_args}",
            ffmpeg::ffi::AVPixelFormat::from(decoder.format()) as i32,
            in_tb.numerator(),
            in_tb.denominator(),
        );
        let mut graph = filter::Graph::new();
        graph.add(
            &filter::find("buffer").ok_or_else(|| Error::Unsupported("buffer filter".into()))?,
            "in",
            &args,
        )?;
        graph.add(
            &filter::find("buffersink")
                .ok_or_else(|| Error::Unsupported("buffersink filter".into()))?,
            "out",
            "",
        )?;
        graph.output("in", 0)?.input("out", 0)?.parse(&spec_str)?;
        graph.validate()?;

        Ok(Self {
            decoder,
            filter: graph,
            encoder: opened,
            in_index,
            out_index,
            in_tb,
            enc_tb: fps.invert(),
            out_tb: fps.invert(),
            frames: 0,
            last_pts_secs: 0.0,
            encoder_name: name,
            fail_after: gpu_encoder.and(settings.simulate_gpu_failure_after),
        })
    }

    fn encode_drain(&mut self, octx: &mut format::context::Output) -> Result<()> {
        let mut pkt = Packet::empty();
        while self.encoder.receive_packet(&mut pkt).is_ok() {
            pkt.set_stream(self.out_index);
            // Constant frame rate: every packet lasts exactly one tick of the
            // encoder time base. Without this the MP4 muxer under-counts the
            // last frame's duration.
            if pkt.duration() == 0 {
                pkt.set_duration(1);
            }
            pkt.rescale_ts(self.enc_tb, self.out_tb);
            pkt.write_interleaved(octx)?;
        }
        Ok(())
    }

    fn filter_drain(&mut self, octx: &mut format::context::Output) -> Result<()> {
        let mut out = frame::Video::empty();
        while self
            .filter
            .get("out")
            .unwrap()
            .sink()
            .frame(&mut out)
            .is_ok()
        {
            out.set_kind(picture::Type::None);
            if self.fail_after == Some(self.frames) {
                return Err(Error::Unsupported("simulated GPU failure".into()));
            }
            self.encoder.send_frame(&out)?;
            self.frames += 1;
            self.encode_drain(octx)?;
        }
        Ok(())
    }

    fn decode_drain(&mut self, octx: &mut format::context::Output) -> Result<()> {
        let mut f = frame::Video::empty();
        while self.decoder.receive_frame(&mut f).is_ok() {
            let Some(ts) = f.timestamp() else { continue };
            f.set_pts(Some(ts));
            self.last_pts_secs =
                ts as f64 * f64::from(self.in_tb.numerator()) / f64::from(self.in_tb.denominator());
            self.filter.get("in").unwrap().source().add(&f)?;
            self.filter_drain(octx)?;
        }
        Ok(())
    }

    fn finish(&mut self, octx: &mut format::context::Output) -> Result<()> {
        self.decoder.send_eof().ok();
        self.decode_drain(octx)?;
        self.filter.get("in").unwrap().source().flush()?;
        self.filter_drain(octx)?;
        self.encoder.send_eof()?;
        self.encode_drain(octx)
    }
}

// --- audio pipeline ---------------------------------------------------------

struct AudioPipe {
    decoder: ffmpeg::decoder::Audio,
    filter: filter::Graph,
    encoder: ffmpeg::encoder::Audio,
    in_index: usize,
    out_index: usize,
    in_tb: Rational,
    enc_tb: Rational,
    out_tb: Rational,
    last_pts_secs: f64,
    /// Samples (per channel) handed to the encoder so far.
    samples_sent: u64,
    encoder_name: &'static str,
}

impl AudioPipe {
    fn new(
        ictx: &format::context::Input,
        octx: &mut format::context::Output,
        in_index: usize,
        settings: &ExportSettings,
        spec: AudioSpec,
        origin_secs: f64,
        global_header: bool,
    ) -> Result<Self> {
        let ist = ictx.stream(in_index).ok_or(Error::NoStream("audio"))?;
        let in_tb = ist.time_base();
        let dctx = codec::context::Context::from_parameters(ist.parameters())?;
        let decoder = dctx.decoder().audio()?;

        let in_layout = if decoder.channel_layout().is_empty() {
            ffmpeg::ChannelLayout::default(i32::from(decoder.channels()))
        } else {
            decoder.channel_layout()
        };

        let (codec_found, name) = find_encoder(spec.encoders)?;
        let audio_codec = codec_found.audio()?;
        let candidates = layout_candidates(
            &audio_codec,
            in_layout,
            decoder.channels(),
            spec.max_channels,
            name == "libopus",
        );
        let rate = spec.fixed_rate.unwrap_or_else(|| {
            let wanted = decoder.rate();
            match audio_codec.rates() {
                Some(mut r) => {
                    let all: Vec<i32> = r.by_ref().collect();
                    if all.is_empty() || all.contains(&(wanted as i32)) {
                        wanted
                    } else if all.contains(&48_000) {
                        48_000
                    } else {
                        all[0] as u32
                    }
                }
                None => wanted,
            }
        });
        let sample_fmt = audio_codec
            .formats()
            .and_then(|mut f| f.next())
            .ok_or_else(|| Error::Unsupported(format!("{name}: unknown sample formats")))?;

        // Try each candidate layout until the encoder accepts one.
        let mut found = None;
        let mut last_err = None;
        for layout in candidates {
            let mut enc = new_encoder_context(&codec_found)?.encoder().audio()?;
            if global_header {
                enc.set_flags(codec::Flags::GLOBAL_HEADER);
            }
            enc.set_rate(rate as i32);
            enc.set_channel_layout(layout);
            enc.set_channels(layout.channels());
            enc.set_format(sample_fmt);
            if spec.lossy {
                let default = match layout.channels() {
                    1 => 128_000,
                    2 => 192_000,
                    n => 64_000 * u64::from(n.unsigned_abs()),
                };
                enc.set_bit_rate(settings.audio_bitrate.unwrap_or(default) as usize);
            }
            enc.set_time_base((1, rate as i32));
            // Opus needs an explicit channel mapping for anything beyond stereo.
            let mut audio_opts: Vec<(&str, String)> = Vec::new();
            if name == "libopus" && layout.channels() > 2 {
                audio_opts.push(("mapping_family", "1".into()));
            }
            match enc.open_as_with(codec_found, dictionary(&audio_opts)) {
                Ok(opened) => {
                    found = Some(opened);
                    break;
                }
                Err(e) => last_err = Some(e),
            }
        }
        let Some(opened) = found else {
            return Err(last_err.map(Error::from).unwrap_or_else(|| {
                Error::Unsupported(format!("{name}: no usable channel layout"))
            }));
        };
        let mut ost = octx.add_stream(codec_found)?;
        let out_index = ost.index();
        ost.set_time_base((1, rate as i32));
        ost.set_parameters(&opened);

        let ticks = (origin_secs * f64::from(in_tb.denominator())
            / f64::from(in_tb.numerator().max(1)))
        .round() as i64;
        let args = format!(
            "time_base={}/{}:sample_rate={}:sample_fmt={}:channel_layout=0x{:x}",
            in_tb.numerator(),
            in_tb.denominator(),
            decoder.rate(),
            decoder.format().name(),
            in_layout.bits()
        );
        let mut graph = filter::Graph::new();
        graph.add(
            &filter::find("abuffer").ok_or_else(|| Error::Unsupported("abuffer filter".into()))?,
            "in",
            &args,
        )?;
        graph.add(
            &filter::find("abuffersink")
                .ok_or_else(|| Error::Unsupported("abuffersink filter".into()))?,
            "out",
            "",
        )?;
        {
            let mut out = graph.get("out").unwrap();
            out.set_sample_format(opened.format());
            out.set_channel_layout(opened.channel_layout());
            out.set_sample_rate(opened.rate());
        }
        graph.output("in", 0)?.input("out", 0)?.parse(&{
            let trim = settings.trim_args();
            if trim.is_empty() {
                format!("asetpts=PTS-{ticks}")
            } else {
                format!("asetpts=PTS-{ticks},atrim={trim},asetpts=PTS-STARTPTS")
            }
        })?;
        graph.validate()?;
        if !audio_codec
            .capabilities()
            .contains(ffmpeg::codec::capabilities::Capabilities::VARIABLE_FRAME_SIZE)
        {
            graph
                .get("out")
                .unwrap()
                .sink()
                .set_frame_size(opened.frame_size());
        }

        Ok(Self {
            decoder,
            filter: graph,
            encoder: opened,
            in_index,
            out_index,
            in_tb,
            enc_tb: Rational::new(1, rate as i32),
            out_tb: Rational::new(1, rate as i32),
            last_pts_secs: 0.0,
            samples_sent: 0,
            encoder_name: name,
        })
    }

    fn encode_drain(&mut self, octx: &mut format::context::Output) -> Result<()> {
        let mut pkt = Packet::empty();
        while self.encoder.receive_packet(&mut pkt).is_ok() {
            // FLAC emits a trailing zero-length packet that only carries the final
            // header (total sample count). The muxer rejects it through this API,
            // so skip it; `patch_flac_length` writes the count into the file afterwards.
            if pkt.size() == 0 {
                continue;
            }
            pkt.set_stream(self.out_index);
            pkt.rescale_ts(self.enc_tb, self.out_tb);
            pkt.write_interleaved(octx)?;
        }
        Ok(())
    }

    fn filter_drain(&mut self, octx: &mut format::context::Output) -> Result<()> {
        let mut out = frame::Audio::empty();
        while self
            .filter
            .get("out")
            .unwrap()
            .sink()
            .frame(&mut out)
            .is_ok()
        {
            self.samples_sent += out.samples() as u64;
            self.encoder.send_frame(&out)?;
            self.encode_drain(octx)?;
        }
        Ok(())
    }

    fn decode_drain(&mut self, octx: &mut format::context::Output) -> Result<()> {
        let mut f = frame::Audio::empty();
        while self.decoder.receive_frame(&mut f).is_ok() {
            let Some(ts) = f.timestamp() else { continue };
            f.set_pts(Some(ts));
            if f.channel_layout().is_empty() {
                f.set_channel_layout(ffmpeg::ChannelLayout::default(i32::from(f.channels())));
            }
            self.last_pts_secs =
                ts as f64 * f64::from(self.in_tb.numerator()) / f64::from(self.in_tb.denominator());
            self.filter.get("in").unwrap().source().add(&f)?;
            self.filter_drain(octx)?;
        }
        Ok(())
    }

    fn finish(&mut self, octx: &mut format::context::Output) -> Result<()> {
        self.decoder.send_eof().ok();
        self.decode_drain(octx)?;
        self.filter.get("in").unwrap().source().flush()?;
        self.filter_drain(octx)?;
        self.encoder.send_eof()?;
        self.encode_drain(octx)
    }
}

/// Write the total sample count into a finished FLAC file's STREAMINFO block.
///
/// FFmpeg's FLAC muxer normally does this from a final zero-length packet that
/// the API we use will not accept. Without it, players cannot show the length.
/// Does nothing if the file is not plain FLAC or already has a count.
fn patch_flac_length(path: &Path, total_samples: u64) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    if total_samples == 0 || total_samples >= (1 << 36) {
        return Ok(());
    }
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    // "fLaC", then block header (type 0 = STREAMINFO, length 34), then 34 bytes.
    let mut head = [0_u8; 4 + 4 + 34];
    if f.read_exact(&mut head).is_err() || &head[..4] != b"fLaC" || head[4] & 0x7F != 0 {
        return Ok(());
    }
    let info = &mut head[8..];
    let existing = u64::from(info[13] & 0x0F) << 32
        | u64::from(info[14]) << 24
        | u64::from(info[15]) << 16
        | u64::from(info[16]) << 8
        | u64::from(info[17]);
    if existing != 0 {
        return Ok(());
    }
    info[13] = (info[13] & 0xF0) | ((total_samples >> 32) & 0x0F) as u8;
    info[14] = (total_samples >> 24) as u8;
    info[15] = (total_samples >> 16) as u8;
    info[16] = (total_samples >> 8) as u8;
    info[17] = total_samples as u8;
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&head)
}

// --- driver -----------------------------------------------------------------

/// `gpu_encoder`: encode video with this NVENC encoder instead of the preset's own.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SubKind {
    /// MP4/MOV text track (`tx3g`).
    MovText,
    /// Matroska `S_TEXT/UTF8`.
    Srt,
}

/// The slice of the source that ends up in the output.
#[derive(Clone, Copy)]
struct Window {
    /// Time zero of the source (see `run_export`).
    origin_secs: f64,
    trim_start: f64,
    trim_end: Option<f64>,
}

/// A subtitle track being written next to the audio and video.
struct SubTrack {
    out_index: usize,
    out_tb: Rational,
    kind: SubKind,
    cues: Vec<crate::types::SubtitleCue>,
}

/// Read the source's text subtitle tracks and add an output stream for each.
/// Anything that cannot be carried over is explained in `notes`.
fn prepare_subtitles(
    input: &Path,
    ictx: &format::context::Input,
    octx: &mut format::context::Output,
    muxer: &str,
    window: Window,
    notes: &mut Vec<String>,
) -> Result<Vec<SubTrack>> {
    let wanted: Vec<(usize, ffmpeg::codec::Id, Option<String>, Option<String>)> = ictx
        .streams()
        .filter(|s| s.parameters().medium() == Type::Subtitle)
        .map(|s| {
            let md = s.metadata();
            (
                s.index(),
                s.parameters().id(),
                md.get("language").map(str::to_owned),
                md.get("title").map(str::to_owned),
            )
        })
        .collect();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let (kind, codec_id) = match muxer {
        "mp4" | "mov" => (SubKind::MovText, ffmpeg::codec::Id::MOV_TEXT),
        "matroska" => (SubKind::Srt, ffmpeg::codec::Id::SUBRIP),
        other => {
            notes.push(format!(
                "{other} output cannot hold these subtitles; they were left out"
            ));
            return Ok(Vec::new());
        }
    };
    let Some(codec) = ffmpeg::encoder::find(codec_id) else {
        notes.push("this FFmpeg build cannot write text subtitles; they were left out".into());
        return Ok(Vec::new());
    };

    // A second reader, so the main one keeps its position.
    let mut sub_in = format::input(&input)?;
    let mut tracks = Vec::new();
    for (index, id, language, title) in wanted {
        if !crate::subtitle::is_text_codec(id) {
            notes.push(format!(
                "subtitle stream {index} ({}) is a picture format and was skipped",
                id.name()
            ));
            continue;
        }
        let cues = match crate::subtitle::read_cues(&mut sub_in, index) {
            Ok(c) => crate::subtitle::rebase_cues(
                c,
                window.origin_secs,
                window.trim_start,
                window.trim_end,
            ),
            Err(e) => {
                notes.push(format!("subtitle stream {index} could not be read: {e}"));
                continue;
            }
        };
        if cues.is_empty() {
            notes.push(format!(
                "subtitle stream {index} has no lines in the exported range"
            ));
            continue;
        }
        let mut ost = octx.add_stream(codec)?;
        // SAFETY: `ost` is a freshly created stream owned by `octx`, so its codec
        // parameters pointer is valid. Writing the plain `codec_type` and `codec_id`
        // fields tells the muxer what this stream carries; ffmpeg-next has no setter
        // for subtitle streams.
        unsafe {
            let par = (*ost.as_mut_ptr()).codecpar;
            (*par).codec_type = ffmpeg::ffi::AVMediaType::AVMEDIA_TYPE_SUBTITLE;
            (*par).codec_id = codec_id.into();
        }
        ost.set_time_base(Rational::new(1, 1000));
        let mut md = Dictionary::new();
        if let Some(l) = language {
            md.set("language", &l);
        }
        if let Some(t) = title {
            md.set("title", &t);
        }
        ost.set_metadata(md);
        tracks.push(SubTrack {
            out_index: ost.index(),
            out_tb: Rational::new(1, 1000),
            kind,
            cues,
        });
    }
    Ok(tracks)
}

/// Write every cue of `track`. MP4/MOV text tracks need explicit empty samples
/// in the gaps, otherwise a line would stay up until the next one.
fn write_subtitle_track(octx: &mut format::context::Output, track: &SubTrack) -> Result<()> {
    let tb = f64::from(track.out_tb.numerator()) / f64::from(track.out_tb.denominator().max(1));
    let ts = |secs: f64| (secs / tb).round() as i64;
    let send = |octx: &mut format::context::Output, data: &[u8], start: i64, dur: i64| {
        let mut p = Packet::copy(data);
        p.set_pts(Some(start));
        p.set_dts(Some(start));
        p.set_duration(dur.max(1));
        p.set_stream(track.out_index);
        p.write_interleaved(octx)
    };
    let mut cursor = 0.0_f64;
    for cue in &track.cues {
        let mut start = cue.start_secs;
        if track.kind == SubKind::MovText {
            // Samples cannot overlap: a new line replaces the previous one.
            start = start.max(cursor);
            if cue.end_secs <= start {
                continue;
            }
            if start > cursor + 0.001 {
                send(octx, &[0, 0], ts(cursor), ts(start) - ts(cursor))?;
            }
        }
        let bytes = cue.text.as_bytes();
        let data = match track.kind {
            SubKind::MovText => {
                let n = bytes.len().min(usize::from(u16::MAX));
                let mut d = Vec::with_capacity(n + 2);
                d.extend_from_slice(&(n as u16).to_be_bytes());
                d.extend_from_slice(&bytes[..n]);
                d
            }
            SubKind::Srt => bytes.to_vec(),
        };
        send(octx, &data, ts(start), ts(cue.end_secs) - ts(start))?;
        cursor = cursor.max(cue.end_secs);
    }
    Ok(())
}

/// Copy chapter markers, shifted and clipped to the exported range.
/// Returns how many were written (0 when the container cannot hold chapters).
fn add_chapters(
    ictx: &format::context::Input,
    octx: &mut format::context::Output,
    trim_start: f64,
    trim_end: Option<f64>,
) -> Result<usize> {
    if !matches!(octx.format().name(), "mp4" | "mov" | "matroska" | "ipod") {
        return Ok(0);
    }
    let len = trim_end.map(|e| e - trim_start);
    let mut written = 0usize;
    for ch in ictx.chapters() {
        let tb = ch.time_base();
        let f = f64::from(tb.numerator()) / f64::from(tb.denominator().max(1));
        let mut start = ch.start() as f64 * f - trim_start;
        let mut end = ch.end() as f64 * f - trim_start;
        if end <= 0.0 || len.is_some_and(|l| start >= l) {
            continue;
        }
        start = start.max(0.0);
        if let Some(l) = len {
            end = end.min(l);
        }
        if end <= start {
            continue;
        }
        let title = ch
            .metadata()
            .get("title")
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Chapter {}", written + 1));
        octx.add_chapter(
            written as i64,
            Rational::new(1, 1000),
            (start * 1000.0).round() as i64,
            (end * 1000.0).round() as i64,
            title,
        )?;
        written += 1;
    }
    Ok(written)
}

fn run_export(
    input: &Path,
    output: &Path,
    settings: &ExportSettings,
    gpu_encoder: Option<&'static str>,
    mut on_progress: impl FnMut(&Progress) -> bool,
) -> Result<ExportReport> {
    let started = Instant::now();
    let preset = settings.preset;
    let trim = settings.trim_range()?;
    let mut ictx = format::input(&input)?;

    let pick = |ictx: &format::context::Input,
                wanted: Option<usize>,
                kind: Type,
                label: &'static str|
     -> Result<Option<usize>> {
        match wanted {
            Some(i) => match ictx.stream(i) {
                Some(s) if s.parameters().medium() == kind => Ok(Some(i)),
                Some(_) => Err(Error::InvalidArgument(format!(
                    "stream {i} is not a {label} stream"
                ))),
                None => Err(Error::InvalidArgument(format!("no stream with index {i}"))),
            },
            None => Ok(ictx.streams().best(kind).map(|s| s.index())),
        }
    };
    let video_in = match preset.video() {
        Some(_) => Some(
            pick(&ictx, settings.video_stream, Type::Video, "video")?
                .ok_or(Error::NoStream("video"))?,
        ),
        None => None,
    };
    let audio_in = match preset.audio() {
        Some(_) => pick(&ictx, settings.audio_stream, Type::Audio, "audio")?,
        None => None,
    };
    if video_in.is_none() && audio_in.is_none() {
        return Err(Error::NoStream("audio"));
    }

    // Common time origin so audio and video keep their relative offset.
    let origin_secs = [video_in, audio_in]
        .into_iter()
        .flatten()
        .filter_map(|i| ictx.stream(i))
        .map(|s| start_offset_secs(&s))
        .fold(f64::INFINITY, f64::min);
    let origin_secs = if origin_secs.is_finite() {
        origin_secs
    } else {
        0.0
    };

    let total_secs = {
        let d = ictx.duration();
        (d > 0).then(|| d as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE))
    };
    let (trim_start, trim_end) = trim.unwrap_or((0.0, None));
    let total_secs = total_secs.map(|t| (trim_end.unwrap_or(t).min(t) - trim_start).max(0.0));

    // Jump close to the start so we do not decode what will be thrown away;
    // the trim filter makes the cut exact.
    if trim_start > 0.0 {
        let us = ((origin_secs + trim_start) * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
        ictx.seek(us, ..us)?;
    }

    let mut octx = format::output_as(&output, preset.muxer())?;
    let global_header = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);

    let mut vpipe = match (video_in, preset.video()) {
        (Some(i), Some(spec)) => Some(VideoPipe::new(
            &ictx,
            &mut octx,
            i,
            settings,
            spec,
            gpu_encoder,
            origin_secs,
            global_header,
        )?),
        _ => None,
    };
    let mut apipe = match (audio_in, preset.audio()) {
        (Some(i), Some(spec)) => Some(AudioPipe::new(
            &ictx,
            &mut octx,
            i,
            settings,
            spec,
            origin_secs,
            global_header,
        )?),
        _ => None,
    };

    let mut notes = Vec::new();
    let mut sub_tracks = if settings.subtitles {
        prepare_subtitles(
            input,
            &ictx,
            &mut octx,
            preset.muxer(),
            Window {
                origin_secs,
                trim_start,
                trim_end,
            },
            &mut notes,
        )?
    } else {
        Vec::new()
    };
    let chapter_count = if settings.chapters {
        add_chapters(&ictx, &mut octx, trim_start, trim_end)?
    } else {
        0
    };

    octx.set_metadata(ictx.metadata().to_owned());
    let mut header_opts = Dictionary::new();
    if settings.fast_start && matches!(preset.muxer(), "mp4" | "mov" | "ipod") {
        header_opts.set("movflags", "+faststart");
    }
    octx.write_header_with(header_opts)?;

    // Muxers may adjust the stream time base when writing the header.
    if let Some(v) = vpipe.as_mut() {
        v.out_tb = octx
            .stream(v.out_index)
            .map(|s| s.time_base())
            .unwrap_or(v.enc_tb);
    }
    if let Some(a) = apipe.as_mut() {
        a.out_tb = octx
            .stream(a.out_index)
            .map(|s| s.time_base())
            .unwrap_or(a.enc_tb);
    }

    for t in &mut sub_tracks {
        t.out_tb = octx
            .stream(t.out_index)
            .map(|s| s.time_base())
            .unwrap_or(Rational::new(1, 1000));
        write_subtitle_track(&mut octx, t)?;
    }

    let mut last_reported = f64::NEG_INFINITY;
    let mut report = |position: f64, frames: u64, force: bool| -> bool {
        if !force && position - last_reported < 0.2 {
            return true;
        }
        last_reported = position;
        on_progress(&Progress {
            position_secs: position,
            total_secs,
            frames,
        })
    };

    for (stream, packet) in ictx.packets() {
        let idx = stream.index();
        if let Some(v) = vpipe.as_mut().filter(|v| v.in_index == idx) {
            match v.decoder.send_packet(&packet) {
                Ok(()) | Err(ffmpeg::Error::InvalidData) => {}
                Err(e) => return Err(e.into()),
            }
            v.decode_drain(&mut octx)?;
            let pos = (v.last_pts_secs - origin_secs - trim_start).max(0.0);
            if !report(pos, v.frames, false) {
                return Err(Error::Cancelled);
            }
        } else if let Some(a) = apipe.as_mut().filter(|a| a.in_index == idx) {
            match a.decoder.send_packet(&packet) {
                Ok(()) | Err(ffmpeg::Error::InvalidData) => {}
                Err(e) => return Err(e.into()),
            }
            a.decode_drain(&mut octx)?;
            if vpipe.is_none() {
                let pos = (a.last_pts_secs - origin_secs - trim_start).max(0.0);
                if !report(pos, 0, false) {
                    return Err(Error::Cancelled);
                }
            }
        }
        // Everything we still need is already decoded once every active
        // stream has passed the trim end.
        if let Some(end) = trim_end {
            let limit = origin_secs + end + 1.0;
            let done = vpipe.as_ref().is_none_or(|v| v.last_pts_secs > limit)
                && apipe.as_ref().is_none_or(|a| a.last_pts_secs > limit);
            if done {
                break;
            }
        }
    }

    if let Some(v) = vpipe.as_mut() {
        v.finish(&mut octx)?;
    }
    if let Some(a) = apipe.as_mut() {
        a.finish(&mut octx)?;
    }
    octx.write_trailer()?;
    if preset == ExportPreset::Flac {
        if let Some(a) = apipe.as_ref() {
            // Best effort: the audio is complete either way.
            let _ = patch_flac_length(output, a.samples_sent);
        }
    }

    let frames = vpipe.as_ref().map_or(0, |v| v.frames);
    let end_secs = [
        vpipe.as_ref().map(|v| v.last_pts_secs),
        apipe.as_ref().map(|a| a.last_pts_secs),
    ]
    .into_iter()
    .flatten()
    .fold(0.0_f64, f64::max);
    let end_rel = trim_end.map_or(end_secs - origin_secs, |e| (end_secs - origin_secs).min(e));
    let out_secs = (end_rel - trim_start).max(0.0);
    report(out_secs, frames, true);

    Ok(ExportReport {
        frames,
        video_encoder: vpipe.as_ref().map(|v| v.encoder_name.to_string()),
        audio_encoder: apipe.as_ref().map(|a| a.encoder_name.to_string()),
        duration_secs: out_secs,
        output_bytes: std::fs::metadata(output).map(|m| m.len()).unwrap_or(0),
        elapsed: started.elapsed(),
        hw_fallback_reason: None,
        subtitle_tracks: sub_tracks.len(),
        chapters: chapter_count,
        notes,
    })
}
