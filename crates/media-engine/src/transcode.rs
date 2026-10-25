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

    let result = run_export(input, output, settings, on_progress);
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
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

fn pixel_from_name(name: &str) -> Result<Pixel> {
    Ok(match name {
        "yuv420p" => Pixel::YUV420P,
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
}

impl VideoPipe {
    fn new(
        ictx: &format::context::Input,
        octx: &mut format::context::Output,
        in_index: usize,
        settings: &ExportSettings,
        spec: VideoSpec,
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

        let (codec_found, name) = find_encoder(spec.encoders)?;
        let pix = pixel_from_name(spec.pix_fmt)?;

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
        let mut spec_str = format!(
            "setpts=PTS-{ticks},fps={}/{}{rotate},scale={ow}:{oh}",
            fps.numerator(),
            fps.denominator()
        );
        if settings.preset == ExportPreset::Gif {
            spec_str.push_str(
                ",split[s0][s1];[s0]palettegen=stats_mode=diff[p];[s1][p]paletteuse=dither=bayer:bayer_scale=5",
            );
        } else {
            spec_str.push_str(&format!(",format={}", spec.pix_fmt));
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
        let out_layout = if decoder.channels() == 1 {
            ffmpeg::ChannelLayout::MONO
        } else {
            ffmpeg::ChannelLayout::STEREO
        };

        let (codec_found, name) = find_encoder(spec.encoders)?;
        let audio_codec = codec_found.audio()?;
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

        let mut ost = octx.add_stream(codec_found)?;
        let out_index = ost.index();
        let mut enc = new_encoder_context(&codec_found)?.encoder().audio()?;
        if global_header {
            enc.set_flags(codec::Flags::GLOBAL_HEADER);
        }
        enc.set_rate(rate as i32);
        enc.set_channel_layout(out_layout);
        enc.set_channels(out_layout.channels());
        enc.set_format(sample_fmt);
        if spec.lossy {
            let default = if out_layout.channels() == 1 {
                128_000
            } else {
                192_000
            };
            enc.set_bit_rate(settings.audio_bitrate.unwrap_or(default) as usize);
        }
        enc.set_time_base((1, rate as i32));
        ost.set_time_base((1, rate as i32));
        let opened = enc.open_as(codec_found)?;
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
        graph
            .output("in", 0)?
            .input("out", 0)?
            .parse(&format!("asetpts=PTS-{ticks}"))?;
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
            encoder_name: name,
        })
    }

    fn encode_drain(&mut self, octx: &mut format::context::Output) -> Result<()> {
        let mut pkt = Packet::empty();
        while self.encoder.receive_packet(&mut pkt).is_ok() {
            // FLAC emits a trailing zero-length packet that only carries updated
            // header metadata; the muxer rejects it when fed through this API,
            // so skip it. The audio data itself is complete.
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

// --- driver -----------------------------------------------------------------

fn run_export(
    input: &Path,
    output: &Path,
    settings: &ExportSettings,
    mut on_progress: impl FnMut(&Progress) -> bool,
) -> Result<ExportReport> {
    let started = Instant::now();
    let preset = settings.preset;
    let mut ictx = format::input(&input)?;

    let video_in = match preset.video() {
        Some(_) => Some(
            ictx.streams()
                .best(Type::Video)
                .ok_or(Error::NoStream("video"))?
                .index(),
        ),
        None => None,
    };
    let audio_in = match preset.audio() {
        Some(_) => ictx.streams().best(Type::Audio).map(|s| s.index()),
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

    let mut octx = format::output_as(&output, preset.muxer())?;
    let global_header = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);

    let mut vpipe = match (video_in, preset.video()) {
        (Some(i), Some(spec)) => Some(VideoPipe::new(
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
            let pos = (v.last_pts_secs - origin_secs).max(0.0);
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
                let pos = (a.last_pts_secs - origin_secs).max(0.0);
                if !report(pos, 0, false) {
                    return Err(Error::Cancelled);
                }
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

    let frames = vpipe.as_ref().map_or(0, |v| v.frames);
    let end_secs = [
        vpipe.as_ref().map(|v| v.last_pts_secs),
        apipe.as_ref().map(|a| a.last_pts_secs),
    ]
    .into_iter()
    .flatten()
    .fold(0.0_f64, f64::max);
    report((end_secs - origin_secs).max(0.0), frames, true);

    Ok(ExportReport {
        frames,
        video_encoder: vpipe.as_ref().map(|v| v.encoder_name.to_string()),
        audio_encoder: apipe.as_ref().map(|a| a.encoder_name.to_string()),
        duration_secs: (end_secs - origin_secs).max(0.0),
        output_bytes: std::fs::metadata(output).map(|m| m.len()).unwrap_or(0),
        elapsed: started.elapsed(),
    })
}
