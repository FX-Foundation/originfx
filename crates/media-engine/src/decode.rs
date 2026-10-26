//! Video and audio decoding with frame-accurate seeking.
//!
//! Video decodes in software by default; [`DecodeOptions`] can move it to the
//! GPU (see [`HwPolicy`]). Either way frames come out as the same RGBA.

use std::collections::VecDeque;
use std::path::Path;

use ffmpeg::format::Pixel;
use ffmpeg::media::Type;
use ffmpeg::software::scaling::{Context as Scaler, Flags};
use ffmpeg_next as ffmpeg;

use crate::error::{Error, Result};
use crate::hwaccel::{HwApi, HwPolicy, HwSetup};
use crate::probe::{stream_duration_secs, stream_rotation};
use crate::types::{AudioBuffer, Rational, VideoFrame, VideoStreamInfo};

/// `AVERROR(EAGAIN)`: "feed me more input" from `receive_frame`.
fn is_again(e: &ffmpeg::Error) -> bool {
    matches!(e, ffmpeg::Error::Other { errno } if *errno == ffmpeg::util::error::EAGAIN)
}

fn open_input(path: &Path) -> Result<ffmpeg::format::context::Input> {
    crate::init();
    if !path.exists() {
        return Err(Error::NotFound(path.to_path_buf()));
    }
    Ok(ffmpeg::format::input(&path)?)
}

/// Tell swscale which YUV matrix and range the source uses; output is full-range RGB.
/// Ignored (returns -1 inside FFmpeg) for sources where it does not apply, such as RGB.
fn set_scaler_colour(scaler: &mut Scaler, table_id: i32, src_full_range: bool) {
    // SAFETY: `scaler` owns a valid SwsContext for the duration of the call, and
    // `sws_getCoefficients` returns a pointer to a static table that FFmpeg only reads.
    unsafe {
        let table = ffmpeg::ffi::sws_getCoefficients(table_id);
        ffmpeg::ffi::sws_setColorspaceDetails(
            scaler.as_mut_ptr(),
            table,
            i32::from(src_full_range),
            table,
            1,
            0,
            1 << 16,
            1 << 16,
        );
    }
}

/// Stream start offset in seconds; many transport streams do not start at 0.
pub(crate) fn start_offset_secs(stream: &ffmpeg::format::stream::Stream) -> f64 {
    let start = stream.start_time();
    let tb = stream.time_base();
    if start > 0 && tb.denominator() != 0 {
        start as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator())
    } else {
        0.0
    }
}

/// Seek `ictx` to just before `abs_secs` (absolute container time) using the
/// given stream's own timeline.
///
/// `Input::seek` always seeks on FFmpeg's "default stream", which may be the
/// audio stream; landing on an audio packet drops us in the middle of a video
/// GOP and the next decodable video frame is a whole keyframe interval late.
fn seek_on_stream(
    ictx: &mut ffmpeg::format::context::Input,
    stream_index: usize,
    time_base: ffmpeg::Rational,
    abs_secs: f64,
) -> Result<()> {
    let tb = f64::from(time_base.numerator()) / f64::from(time_base.denominator().max(1));
    let ts = (abs_secs.max(0.0) / tb.max(1e-12)).round() as i64;
    // SAFETY: `ictx` is a valid, open input context for the whole call, and
    // `stream_index` came from one of its streams. Passing `i64::MIN` as the
    // lower bound and `ts` as both target and upper bound requests the nearest
    // seek point at or before `ts`.
    let ret = unsafe {
        ffmpeg::ffi::avformat_seek_file(ictx.as_mut_ptr(), stream_index as i32, i64::MIN, ts, ts, 0)
    };
    if ret < 0 {
        return Err(ffmpeg::Error::from(ret).into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Video
// ---------------------------------------------------------------------------

/// Rotate tightly packed RGBA clockwise by `degrees` (0, 90, 180 or 270).
/// Returns the new buffer and its width and height.
pub(crate) fn rotate_rgba(data: Vec<u8>, w: u32, h: u32, degrees: u32) -> (Vec<u8>, u32, u32) {
    let (wu, hu) = (w as usize, h as usize);
    match degrees % 360 {
        90 => {
            let mut out = vec![0u8; data.len()];
            // (x, y) -> (h - 1 - y, x); the new image is h wide and w tall.
            for y in 0..hu {
                for x in 0..wu {
                    let src = (y * wu + x) * 4;
                    let dst = (x * hu + (hu - 1 - y)) * 4;
                    out[dst..dst + 4].copy_from_slice(&data[src..src + 4]);
                }
            }
            (out, h, w)
        }
        180 => {
            let mut out = vec![0u8; data.len()];
            let total = wu * hu;
            for i in 0..total {
                let dst = (total - 1 - i) * 4;
                out[dst..dst + 4].copy_from_slice(&data[i * 4..i * 4 + 4]);
            }
            (out, w, h)
        }
        270 => {
            let mut out = vec![0u8; data.len()];
            // (x, y) -> (y, w - 1 - x); the new image is h wide and w tall.
            for y in 0..hu {
                for x in 0..wu {
                    let src = (y * wu + x) * 4;
                    let dst = ((wu - 1 - x) * hu + y) * 4;
                    out[dst..dst + 4].copy_from_slice(&data[src..src + 4]);
                }
            }
            (out, h, w)
        }
        _ => (data, w, h),
    }
}

/// How far (seconds) before a seek target full decoding resumes. It has to cover the
/// decoder's reordering, so the frame on screen at the target is never one that
/// was skipped.
const CATCH_UP_MARGIN_SECS: f64 = 1.0;

/// How a [`VideoDecoder`] (or [`FrameReader`](crate::FrameReader)) decodes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct DecodeOptions {
    /// GPU decoding. Defaults to [`HwPolicy::Off`] (software).
    pub hw: HwPolicy,
}

/// The plain software decoder, exactly as used when GPU decoding is off.
fn software_decoder(params: ffmpeg::codec::Parameters) -> Result<ffmpeg::decoder::Video> {
    let mut ctx = ffmpeg::codec::context::Context::from_parameters(params)?;
    ctx.set_threading(ffmpeg::threading::Config {
        kind: ffmpeg::threading::Type::Frame,
        count: 0, // 0 = pick automatically from CPU count
    });
    Ok(ctx.decoder().video()?)
}

/// A decoder that decodes on the GPU described by `setup`.
fn hardware_decoder(
    params: ffmpeg::codec::Parameters,
    setup: HwSetup,
) -> Result<ffmpeg::decoder::Video> {
    let mut ctx = ffmpeg::codec::context::Context::from_parameters(params)?;
    // One thread: the GPU does the decoding. Extra frame threads only wait on
    // the GPU, and on this laptop their waiting cost more CPU than software
    // decoding (4K HEVC: 16 fps with them, 27 fps without).
    ctx.set_threading(ffmpeg::threading::Config {
        kind: ffmpeg::threading::Type::Frame,
        count: 1,
    });
    // While seeking we keep up to two decoded frames on the GPU (the candidate
    // and the read-ahead frame); reserve surfaces for them.
    // SAFETY: `ctx` is a valid codec context that has not been opened yet.
    unsafe { (*ctx.as_mut_ptr()).extra_hw_frames = 2 };
    let codec = setup.codec;
    setup.attach(&mut ctx);
    Ok(ctx.decoder().open_as(codec)?.video()?)
}

/// GPU decoding in use: which API, and the pixel format its frames arrive in.
#[derive(Clone, Copy)]
struct GpuState {
    api: HwApi,
    format: ffmpeg::ffi::AVPixelFormat,
    /// The stream is full-range `yuvj420p`; label downloaded frames the same way.
    full_range: bool,
}

/// After switching from GPU to software mid-stream: skip frames we already returned.
struct Resume {
    /// Pts of the last frame returned before the switch.
    after: f64,
    /// Where we seeked to resume.
    from: f64,
    /// How far to step back if that seek landed too late.
    step: f64,
}

/// Sequential video decoder that yields RGBA frames and can seek.
///
/// Frames come out in presentation order. [`VideoDecoder::seek`] is
/// frame-accurate: after it, the next frame returned is the one that is on
/// screen at the requested time.
pub struct VideoDecoder {
    path: std::path::PathBuf,
    ictx: ffmpeg::format::context::Input,
    stream_index: usize,
    time_base: ffmpeg::Rational,
    origin_secs: f64,
    decoder: ffmpeg::decoder::Video,
    scaler: Option<(Pixel, u32, u32, Scaler)>,
    /// Matrix table id and source range the scaler was configured for.
    scaler_colour: Option<(i32, bool)>,
    info: VideoStreamInfo,
    eof_sent: bool,
    /// A decoded frame that was read ahead and must be returned first.
    held: Option<ffmpeg::frame::Video>,
    /// After a seek: the time whose frame we are looking for.
    target: Option<f64>,
    /// Where the last seek actually pointed (seconds from stream start).
    seek_pos: f64,
    /// How far to step back if a seek lands after its target.
    retry_step: f64,
    options: DecodeOptions,
    /// Set while decoding on the GPU.
    gpu: Option<GpuState>,
    /// The GPU this decoder started on, kept after a fallback so frames it
    /// decoded earlier (still in GPU memory) can be recognised and downloaded.
    hw_frames: Option<GpuState>,
    /// Why GPU decoding was asked for but is not in use.
    hw_note: Option<String>,
    /// Pts of the last frame `decode_raw` produced since the last seek.
    last_raw_pts: Option<f64>,
    resume: Option<Resume>,
    /// True once the file was reopened from its very beginning for this seek.
    rewound: bool,
    /// While fast-forwarding from the keyframe to a seek target, unreferenced
    /// frames are skipped for packets before this time (software decoding only).
    skip_until: Option<f64>,
    /// Frames downloaded from the GPU so far.
    gpu_frames: u64,
    /// Testing aid: pretend the GPU download fails after this many frames.
    fail_gpu_after: Option<u64>,
    /// System-memory frames GPU frames are downloaded into, reused.
    gpu_bufs: crate::hwaccel::DownloadBuffers,
}

impl VideoDecoder {
    /// Open the best video stream of `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_stream(path, None)
    }

    /// Open the best video stream of `path` with the given options.
    pub fn open_with(path: impl AsRef<Path>, options: DecodeOptions) -> Result<Self> {
        Self::open_stream_with(path, None, options)
    }

    /// Open a specific video stream (`None` picks the best one).
    pub fn open_stream(path: impl AsRef<Path>, stream_index: Option<usize>) -> Result<Self> {
        Self::open_stream_with(path, stream_index, DecodeOptions::default())
    }

    /// Open a specific video stream (`None` picks the best one) with the given options.
    pub fn open_stream_with(
        path: impl AsRef<Path>,
        stream_index: Option<usize>,
        options: DecodeOptions,
    ) -> Result<Self> {
        let path = path.as_ref();
        let ictx = open_input(path)?;

        let stream = match stream_index {
            Some(i) => ictx
                .stream(i)
                .filter(|s| s.parameters().medium() == Type::Video)
                .ok_or(Error::NoStream("video"))?,
            None => ictx
                .streams()
                .best(Type::Video)
                .ok_or(Error::NoStream("video"))?,
        };
        let index = stream.index();
        let time_base = stream.time_base();
        let origin_secs = start_offset_secs(&stream);

        let (decoder, gpu, hw_note) = open_video_decoder(stream.parameters(), options.hw)?;

        let avg = Rational::from(stream.avg_frame_rate());
        let info = VideoStreamInfo {
            index,
            codec: stream.parameters().id().name().to_string(),
            width: decoder.width(),
            height: decoder.height(),
            pixel_format: format!("{:?}", decoder.format()).to_lowercase(),
            frame_rate: avg.is_valid().then_some(avg),
            time_base: Rational::from(time_base),
            frames: u64::try_from(stream.frames()).ok().filter(|&n| n > 0),
            duration_secs: stream_duration_secs(stream.duration(), time_base),
            bit_rate: Some(decoder.bit_rate() as u64).filter(|&b| b > 0),
            rotation: stream_rotation(&stream),
            colour: crate::colour::ColourInfo::from_decoder(&decoder),
        };

        Ok(Self {
            path: path.to_path_buf(),
            ictx,
            stream_index: index,
            time_base,
            origin_secs,
            decoder,
            scaler: None,
            scaler_colour: None,
            info,
            eof_sent: false,
            held: None,
            target: None,
            seek_pos: 0.0,
            retry_step: 1.0,
            skip_until: None,
            rewound: false,
            options,
            gpu,
            hw_frames: gpu,
            hw_note,
            last_raw_pts: None,
            resume: None,
            gpu_frames: 0,
            fail_gpu_after: None,
            gpu_bufs: Default::default(),
        })
    }

    /// The GPU API decoding this stream, or `None` for software.
    ///
    /// Settled once the first frame is decoded: a GPU that turns out not to
    /// support the stream switches to software (with [`HwPolicy::Auto`]).
    pub fn hw_backend(&self) -> Option<HwApi> {
        self.gpu.map(|g| g.api)
    }

    /// Why GPU decoding was requested but is not being used, if it was.
    pub fn hw_fallback_reason(&self) -> Option<&str> {
        self.hw_note.as_deref()
    }

    /// Testing aid: make the GPU frame download fail after `frames` more frames,
    /// to exercise the fallback to software. No effect when decoding in software.
    #[doc(hidden)]
    pub fn simulate_gpu_failure_after(&mut self, frames: u64) {
        self.fail_gpu_after = Some(self.gpu_frames + frames);
    }

    /// Is `frame` still in GPU memory (decoded by `gpu`)?
    fn on_gpu(frame: &ffmpeg::frame::Video, gpu: GpuState) -> bool {
        // SAFETY: `frame` is a valid frame; we only read its format field.
        unsafe { (*frame.as_ptr()).format == gpu.format as i32 }
    }

    /// Copy a GPU frame into system memory. `Err` carries the reason the GPU
    /// cannot be used any more.
    fn download_gpu_frame(
        &mut self,
        frame: &ffmpeg::frame::Video,
        gpu: GpuState,
    ) -> std::result::Result<ffmpeg::frame::Video, String> {
        if self.fail_gpu_after.is_some_and(|n| self.gpu_frames >= n) {
            return Err("simulated GPU failure".to_string());
        }
        let mut sw = crate::hwaccel::download(frame, &mut self.gpu_bufs)?;
        match sw.format() {
            Pixel::YUV420P if gpu.full_range => {
                // SAFETY: yuvj420p has the same layout as yuv420p; only the label changes,
                // so the RGBA conversion picks full range exactly as for software frames.
                unsafe {
                    (*sw.as_mut_ptr()).format =
                        ffmpeg::ffi::AVPixelFormat::from(Pixel::YUVJ420P) as i32
                };
            }
            Pixel::YUV420P | Pixel::P010LE => {}
            other => {
                return Err(format!(
                    "{} returned {other:?} frames; only 4:2:0 video is decoded on the GPU",
                    gpu.api
                ))
            }
        }
        self.gpu_frames += 1;
        Ok(sw)
    }

    /// Stop using the GPU and continue in software after the frame at `after`
    /// (`None`: nothing returned since the last seek, so redo that seek).
    fn fall_back(&mut self, reason: String, after: Option<f64>) -> Result<()> {
        if let HwPolicy::Require(api) = self.options.hw {
            // Stay on the GPU so later calls report the same error.
            return Err(Error::Unsupported(format!(
                "{api} decoding failed: {reason}"
            )));
        }
        self.gpu = None;
        self.hw_note = Some(reason);
        self.fail_gpu_after = None;
        // A read-ahead frame comes after `after`; it will be decoded again.
        let after = match self.held.take() {
            Some(h) => Some(after.unwrap_or(f64::MAX).min(self.pts_secs(&h) - 1e-4)),
            None => after,
        };
        let params = self
            .ictx
            .stream(self.stream_index)
            .ok_or(Error::NoStream("video"))?
            .parameters();
        self.decoder = software_decoder(params)?;
        self.eof_sent = false;
        // Restart from the keyframe before `after` and skip what was already returned.
        let from = after.unwrap_or(self.seek_pos).max(0.0);
        self.resume = after.map(|after| Resume {
            after,
            from,
            step: 1.0,
        });
        seek_on_stream(
            &mut self.ictx,
            self.stream_index,
            self.time_base,
            from + self.origin_secs,
        )
    }

    /// Apply [`Resume`] to a frame decoded after a fallback: `false` means drop it.
    fn keep_after_resume(&mut self, pts: f64) -> Result<bool> {
        let Some(r) = self.resume.as_mut() else {
            return Ok(true);
        };
        if pts <= r.after + 1e-6 {
            return Ok(false);
        }
        // The first frame we keep must be the one right after `after`. If the
        // seek landed later (MPEG-TS and friends), step back and try again.
        let frame_gap = 1.5 / self.info.frame_rate.map_or(25.0, Rational::as_f64).max(1.0);
        if pts > r.after + frame_gap && r.from > 0.0 {
            r.from = (r.from - r.step).max(0.0);
            r.step *= 2.0;
            let from = r.from;
            seek_on_stream(
                &mut self.ictx,
                self.stream_index,
                self.time_base,
                from + self.origin_secs,
            )?;
            self.decoder.flush();
            self.eof_sent = false;
            return Ok(false);
        }
        self.resume = None;
        Ok(true)
    }

    /// Description of the stream being decoded.
    pub fn info(&self) -> &VideoStreamInfo {
        &self.info
    }

    /// Nominal frame rate, falling back to 25 when the file does not say.
    pub fn fps(&self) -> f64 {
        self.info
            .frame_rate
            .map(Rational::as_f64)
            .filter(|f| *f > 0.0)
            .unwrap_or(25.0)
    }

    fn pts_secs(&self, frame: &ffmpeg::frame::Video) -> f64 {
        let pts = frame.pts().or_else(|| frame.timestamp()).unwrap_or(0);
        pts as f64 * f64::from(self.time_base.numerator()) / f64::from(self.time_base.denominator())
            - self.origin_secs
    }

    /// Next raw decoded frame in presentation order, `None` at end of stream.
    fn decode_raw(&mut self) -> Result<Option<ffmpeg::frame::Video>> {
        if let Some(f) = self.held.take() {
            return Ok(Some(f));
        }
        loop {
            let mut frame = ffmpeg::frame::Video::empty();
            match self.decoder.receive_frame(&mut frame) {
                Ok(()) => {
                    if let Some(gpu) = self.gpu.filter(|g| !Self::on_gpu(&frame, *g)) {
                        // The decoder chose software output: this GPU cannot decode the stream.
                        let reason = format!(
                            "{} cannot decode this stream ({} {}x{})",
                            gpu.api, self.info.codec, self.info.width, self.info.height
                        );
                        self.fall_back(reason, self.last_raw_pts)?;
                        continue;
                    }
                    let pts = self.pts_secs(&frame);
                    if !self.keep_after_resume(pts)? {
                        continue;
                    }
                    self.last_raw_pts = Some(pts);
                    return Ok(Some(frame));
                }
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(e) if is_again(&e) => {}
                Err(e) if self.gpu.is_some() => {
                    self.fall_back(e.to_string(), self.last_raw_pts)?;
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
            if self.eof_sent {
                return Ok(None);
            }
            // Decoder wants more input: read packets until we find one of ours.
            loop {
                let mut packet = ffmpeg::Packet::empty();
                match packet.read(&mut self.ictx) {
                    Ok(()) => {
                        if packet.stream() != self.stream_index {
                            continue;
                        }
                        self.note_packet(&packet);
                        match self.decoder.send_packet(&packet) {
                            Ok(()) | Err(ffmpeg::Error::InvalidData) => {}
                            Err(e) if is_again(&e) => {}
                            Err(e) if self.gpu.is_some() => {
                                self.fall_back(e.to_string(), self.last_raw_pts)?
                            }
                            Err(e) => return Err(e.into()),
                        }
                        break;
                    }
                    Err(ffmpeg::Error::Eof) => {
                        self.decoder.send_eof()?;
                        self.eof_sent = true;
                        break;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }

    /// Convert a raw decoder frame to tightly packed RGBA.
    fn convert(&mut self, raw: &ffmpeg::frame::Video) -> Result<VideoFrame> {
        let (w, h, fmt) = (raw.width(), raw.height(), raw.format());
        // Decode with the matrix the clip was encoded with, not swscale's
        // default (BT.601), or HD footage comes out with shifted colours.
        let colour = crate::colour::resolve(
            raw.color_space(),
            raw.color_range(),
            raw.color_primaries(),
            raw.color_transfer_characteristic(),
            w,
            h,
            crate::colour::is_full_range_format(fmt),
        );
        let colour_key = (crate::colour::sws_table_id(colour.space), colour.full_range);
        let needs_new = self.scaler_colour != Some(colour_key)
            || !matches!(&self.scaler, Some((f, sw, sh, _)) if *f == fmt && *sw == w && *sh == h);
        if needs_new {
            let mut scaler = Scaler::get(fmt, w, h, Pixel::RGBA, w, h, Flags::BILINEAR)?;
            set_scaler_colour(&mut scaler, colour_key.0, colour_key.1);
            self.scaler = Some((fmt, w, h, scaler));
            self.scaler_colour = Some(colour_key);
        }
        let scaler = &mut self.scaler.as_mut().expect("scaler just created").3;

        let mut out = ffmpeg::frame::Video::empty();
        scaler.run(raw, &mut out)?;

        let row = w as usize * 4;
        let stride = out.stride(0);
        let src = out.data(0);
        let mut data = Vec::with_capacity(row * h as usize);
        for y in 0..h as usize {
            data.extend_from_slice(&src[y * stride..y * stride + row]);
        }
        let pts_secs = self.pts_secs(raw);
        let (data, w, h) = rotate_rgba(data, w, h, self.info.rotation);
        Ok(VideoFrame {
            width: w,
            height: h,
            pts_secs,
            data,
        })
    }

    /// Decode the next frame, or `None` at the end of the stream.
    pub fn next_frame(&mut self) -> Result<Option<VideoFrame>> {
        loop {
            let target = self.target;
            let Some(raw) = self.next_raw()? else {
                return Ok(None);
            };
            match self.bring_to_memory(raw)? {
                Some(frame) => return Ok(Some(self.convert(&frame)?)),
                // The GPU failed on this frame: it is decoded again in software.
                None => self.target = target,
            }
        }
    }

    /// Bring the frame chosen by [`next_raw`](Self::next_raw) into system memory.
    /// Only that frame is downloaded: frames skipped while seeking stay on the GPU.
    /// `None` means the GPU failed and decoding switched to software.
    fn bring_to_memory(
        &mut self,
        raw: ffmpeg::frame::Video,
    ) -> Result<Option<ffmpeg::frame::Video>> {
        let Some(gpu) = self.hw_frames.filter(|g| Self::on_gpu(&raw, *g)) else {
            return Ok(Some(raw));
        };
        match self.download_gpu_frame(&raw, gpu) {
            Ok(frame) => Ok(Some(frame)),
            Err(reason) => {
                let pts = self.pts_secs(&raw);
                self.fall_back(reason, Some(pts - 1e-4))?;
                Ok(None)
            }
        }
    }

    /// The next frame as it left the decoder (in GPU memory when decoding on the GPU).
    fn next_raw(&mut self) -> Result<Option<ffmpeg::frame::Video>> {
        let result = self.next_raw_inner();
        // Whatever happened, normal decoding resumes after the first frame.
        self.stop_catching_up();
        result
    }

    /// Software decoding only: while fast-forwarding from the keyframe to a seek
    /// target, skip frames no other frame refers to (typically many B-frames). The
    /// frames that matter are unchanged, so output is identical, only faster.
    ///
    /// The decision is made per packet, as it is handed to the decoder, because the
    /// decoder reads ahead of the frames it outputs: by the time a frame near the
    /// target comes out, the packets after it have already been decoded.
    fn start_catching_up(&mut self, target: f64) {
        if self.gpu.is_some() {
            return;
        }
        self.skip_until = Some(target - CATCH_UP_MARGIN_SECS);
        self.set_discard(ffmpeg::ffi::AVDiscard::AVDISCARD_NONREF);
    }

    fn stop_catching_up(&mut self) {
        if self.skip_until.take().is_some() {
            self.set_discard(ffmpeg::ffi::AVDiscard::AVDISCARD_DEFAULT);
        }
    }

    /// Called before each packet is decoded: resume full decoding once the
    /// packets get close to the seek target.
    fn note_packet(&mut self, packet: &ffmpeg::Packet) {
        let Some(limit) = self.skip_until else {
            return;
        };
        let near = match packet.pts().or(packet.dts()) {
            Some(p) => {
                let tb = f64::from(self.time_base.numerator())
                    / f64::from(self.time_base.denominator().max(1));
                p as f64 * tb - self.origin_secs >= limit
            }
            None => true,
        };
        if near {
            self.stop_catching_up();
        }
    }

    fn set_discard(&mut self, mode: ffmpeg::ffi::AVDiscard) {
        // SAFETY: `self.decoder` owns a valid, open codec context. `skip_frame` is a
        // plain field FFmpeg documents as changeable while decoding; frame threads
        // pick the new value up before the next frame they decode.
        unsafe {
            (*self.decoder.as_mut_ptr()).skip_frame = mode;
        }
    }

    fn next_raw_inner(&mut self) -> Result<Option<ffmpeg::frame::Video>> {
        let Some(target) = self.target.take() else {
            return self.decode_raw();
        };

        // Just after a seek: decode forward and return the last frame whose
        // start time is <= target. Some demuxers (MPEG-TS, raw streams) land
        // between keyframes; the decoder then only produces frames from the
        // *next* keyframe, i.e. after the target. When that happens we seek
        // further back, doubling the step, until we start at or before it.
        let mut previous: Option<ffmpeg::frame::Video> = None;
        loop {
            match self.decode_raw()? {
                None => {
                    if previous.is_none() && self.seek_pos > 0.0 {
                        // Landed so late that nothing decodable was left (e.g. past the
                        // last keyframe of a stream without a seek index): go back.
                        let back_to = (self.seek_pos - self.retry_step).max(0.0);
                        self.retry_step *= 2.0;
                        self.reseek(back_to)?;
                        continue;
                    }
                    if previous.is_none() && !self.rewound {
                        // Not even a seek to time zero found anything: start over.
                        self.rewind()?;
                        continue;
                    }
                    return Ok(previous);
                }
                Some(raw) => {
                    let pts = self.pts_secs(&raw);
                    if previous.is_none() && pts > target + 1e-6 && self.seek_pos > 0.0 {
                        let back_to = (self.seek_pos - self.retry_step).max(0.0);
                        self.retry_step *= 2.0;
                        self.reseek(back_to)?;
                        continue;
                    }
                    if previous.is_none() && pts > target + 1e-6 && !self.rewound {
                        // Seeking to time zero still lands after the target. This is
                        // what MPEG-TS with B-frames does: no keyframe is listed at or
                        // before the start, so the seek jumps to a later one. The
                        // stream really begins at the start of the file.
                        self.rewind()?;
                        continue;
                    }
                    if pts > target + 1e-6 {
                        // `raw` is already after the target; `previous` is on screen.
                        let chosen = match previous {
                            Some(prev) => {
                                self.held = Some(raw);
                                prev
                            }
                            None => raw,
                        };
                        return Ok(Some(chosen));
                    }
                    previous = Some(raw);
                }
            }
        }
    }

    /// Start again from the very first packet of the file, whatever the seek index
    /// says. Used when seeking to time zero cannot find the first frames.
    fn rewind(&mut self) -> Result<()> {
        self.ictx = open_input(&self.path)?;
        self.decoder.flush();
        self.eof_sent = false;
        self.held = None;
        self.seek_pos = 0.0;
        self.last_raw_pts = None;
        self.resume = None;
        self.rewound = true;
        // Everything from the start is needed up to the target, but skipping
        // unreferenced frames is still safe and unchanged.
        Ok(())
    }

    /// Low-level seek used by [`seek`](Self::seek) and its retry logic.
    fn reseek(&mut self, secs: f64) -> Result<()> {
        seek_on_stream(
            &mut self.ictx,
            self.stream_index,
            self.time_base,
            secs + self.origin_secs,
        )?;
        self.decoder.flush();
        self.eof_sent = false;
        self.held = None;
        self.seek_pos = secs;
        self.last_raw_pts = None;
        self.resume = None;
        Ok(())
    }

    /// Seek so that the next [`next_frame`](Self::next_frame) returns the
    /// frame displayed at `secs`.
    pub fn seek(&mut self, secs: f64) -> Result<()> {
        let secs = secs.max(0.0);
        self.reseek(secs)?;
        self.retry_step = 1.0;
        self.rewound = false;
        self.target = Some(secs);
        self.start_catching_up(secs);
        Ok(())
    }

    /// Convenience: seek to `secs` and return the frame shown there.
    pub fn frame_at(&mut self, secs: f64) -> Result<Option<VideoFrame>> {
        self.seek(secs)?;
        self.next_frame()
    }
}

/// Open the video decoder for `params` following `policy`.
/// Returns the decoder, the GPU state if it is on the GPU, and why not if GPU was asked for.
fn open_video_decoder(
    params: ffmpeg::codec::Parameters,
    policy: HwPolicy,
) -> Result<(ffmpeg::decoder::Video, Option<GpuState>, Option<String>)> {
    let apis: &[HwApi] = match &policy {
        HwPolicy::Off => return Ok((software_decoder(params)?, None, None)),
        HwPolicy::Auto => &HwApi::ALL,
        HwPolicy::Require(api) => std::slice::from_ref(api),
    };
    let mut reasons = Vec::new();
    // The GPU path is built (and tested) for 8/10-bit 4:2:0, by far the most common
    // case. Other layouts (4:2:2, 4:4:4, RGB) stay in software; some GPUs claim
    // them but decode them wrongly.
    // SAFETY: `params` wraps valid codec parameters; we only read the format field.
    let coded = unsafe { (*params.as_ptr()).format };
    let is = |p: Pixel| coded == ffmpeg::ffi::AVPixelFormat::from(p) as i32;
    let full_range = is(Pixel::YUVJ420P);
    let gpu_layout = coded == ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_NONE as i32
        || is(Pixel::YUV420P)
        || full_range
        || is(Pixel::YUV420P10LE);
    if !gpu_layout {
        reasons.push("GPU decoding handles 8/10-bit 4:2:0 video only".to_string());
    }
    for &api in apis.iter().filter(|_| gpu_layout) {
        let attempt = HwSetup::new(api, params.id()).and_then(|setup| {
            let format = setup.hw_format;
            hardware_decoder(params.clone(), setup)
                .map(|dec| {
                    let gpu = GpuState {
                        api,
                        format,
                        full_range,
                    };
                    (dec, gpu)
                })
                .map_err(|e| format!("{api} decoder: {e}"))
        });
        match attempt {
            Ok((dec, gpu)) => return Ok((dec, Some(gpu), None)),
            Err(reason) => reasons.push(reason),
        }
    }
    if matches!(policy, HwPolicy::Require(_)) {
        return Err(Error::Unsupported(reasons.join("; ")));
    }
    Ok((software_decoder(params)?, None, Some(reasons.join("; "))))
}

impl Iterator for VideoDecoder {
    type Item = Result<VideoFrame>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_frame().transpose()
    }
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

/// Output format requested from the [`AudioDecoder`].
#[derive(Debug, Clone, Copy, Default)]
pub struct AudioOptions {
    /// Resample to this rate; `None` keeps the file's rate.
    pub sample_rate: Option<u32>,
    /// Mix to this many channels (1 or 2 are common); `None` keeps the file's layout.
    pub channels: Option<u16>,
}

/// Sequential audio decoder producing interleaved `f32` samples.
pub struct AudioDecoder {
    ictx: ffmpeg::format::context::Input,
    stream_index: usize,
    time_base: ffmpeg::Rational,
    origin_secs: f64,
    decoder: ffmpeg::decoder::Audio,
    resampler: ffmpeg::software::resampling::Context,
    in_rate: u32,
    out_layout: ffmpeg::ChannelLayout,
    out_rate: u32,
    out_channels: u16,
    eof_sent: bool,
    flushed: bool,
    queue: VecDeque<AudioBuffer>,
    skip_before: Option<f64>,
}

impl AudioDecoder {
    /// Open the best audio stream of `path`.
    pub fn open(path: impl AsRef<Path>, options: AudioOptions) -> Result<Self> {
        let path = path.as_ref();
        let ictx = open_input(path)?;
        let stream = ictx
            .streams()
            .best(Type::Audio)
            .ok_or(Error::NoStream("audio"))?;
        let index = stream.index();
        let time_base = stream.time_base();
        let origin_secs = start_offset_secs(&stream);

        let ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = ctx.decoder().audio()?;

        let in_layout = if decoder.channel_layout().is_empty() {
            ffmpeg::ChannelLayout::default(i32::from(decoder.channels()))
        } else {
            decoder.channel_layout()
        };
        let out_rate = options.sample_rate.unwrap_or_else(|| decoder.rate());
        let out_channels = options.channels.unwrap_or_else(|| decoder.channels());
        let out_layout = ffmpeg::ChannelLayout::default(i32::from(out_channels));

        let resampler = ffmpeg::software::resampling::Context::get(
            decoder.format(),
            in_layout,
            decoder.rate(),
            ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
            out_layout,
            out_rate,
        )?;

        Ok(Self {
            ictx,
            stream_index: index,
            time_base,
            origin_secs,
            in_rate: decoder.rate(),
            decoder,
            resampler,
            out_layout,
            out_rate,
            out_channels,
            eof_sent: false,
            flushed: false,
            queue: VecDeque::new(),
            skip_before: None,
        })
    }

    /// Sample rate of the buffers this decoder returns.
    pub fn sample_rate(&self) -> u32 {
        self.out_rate
    }

    /// Channel count of the buffers this decoder returns.
    pub fn channels(&self) -> u16 {
        self.out_channels
    }

    fn pts_secs(&self, frame: &ffmpeg::frame::Audio) -> f64 {
        let pts = frame.pts().or_else(|| frame.timestamp()).unwrap_or(0);
        pts as f64 * f64::from(self.time_base.numerator()) / f64::from(self.time_base.denominator())
            - self.origin_secs
    }

    /// An output frame big enough for everything the resampler can produce now.
    ///
    /// `swr_convert_frame` writes at most `nb_samples` of an *allocated* output
    /// frame, and an empty frame defaults to the input's sample count. When the
    /// rate goes up, that silently leaves samples stuck inside the resampler.
    fn alloc_out(&self, in_samples: usize) -> ffmpeg::frame::Audio {
        let delay = self
            .resampler
            .delay()
            .map_or(0, |d| d.output.max(0) as usize);
        let scaled = in_samples as u64 * u64::from(self.out_rate) / u64::from(self.in_rate.max(1));
        let wanted = delay + scaled as usize + 64;
        let mut out = ffmpeg::frame::Audio::new(
            ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
            wanted,
            self.out_layout,
        );
        out.set_rate(self.out_rate);
        out
    }

    fn push_resampled(&mut self, pts: f64, out: &ffmpeg::frame::Audio) {
        if out.samples() == 0 {
            return;
        }
        // Packed f32: `plane::<f32>` only exposes one value per sample frame,
        // so read the raw bytes to get every channel.
        let wanted = out.samples() * usize::from(self.out_channels) * 4;
        let bytes = out.data(0);
        let samples: Vec<f32> = bytes[..wanted.min(bytes.len())]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_ne_bytes(*b))
            .collect();
        self.queue.push_back(AudioBuffer {
            sample_rate: self.out_rate,
            channels: self.out_channels,
            pts_secs: pts,
            samples,
        });
    }

    /// Next block of decoded audio, `None` at the end of the stream.
    pub fn next_buffer(&mut self) -> Result<Option<AudioBuffer>> {
        loop {
            if let Some(buf) = self.queue.pop_front() {
                if let Some(t) = self.skip_before {
                    // Drop whole blocks that end before the seek target, trim the first one.
                    let end = buf.pts_secs + buf.duration_secs();
                    if end <= t {
                        continue;
                    }
                    self.skip_before = None;
                    if buf.pts_secs < t {
                        let skip = ((t - buf.pts_secs) * f64::from(buf.sample_rate)) as usize
                            * usize::from(buf.channels);
                        let skip = skip.min(buf.samples.len());
                        return Ok(Some(AudioBuffer {
                            pts_secs: t,
                            samples: buf.samples[skip..].to_vec(),
                            ..buf
                        }));
                    }
                }
                return Ok(Some(buf));
            }
            if self.flushed {
                return Ok(None);
            }

            let mut frame = ffmpeg::frame::Audio::empty();
            match self.decoder.receive_frame(&mut frame) {
                Ok(()) => {
                    let pts = self.pts_secs(&frame);
                    if frame.channel_layout().is_empty() {
                        frame.set_channel_layout(ffmpeg::ChannelLayout::default(i32::from(
                            frame.channels(),
                        )));
                    }
                    let mut out = self.alloc_out(frame.samples());
                    self.resampler.run(&frame, &mut out)?;
                    self.push_resampled(pts, &out);
                    continue;
                }
                Err(ffmpeg::Error::Eof) => {
                    let mut out = self.alloc_out(0);
                    if self.resampler.flush(&mut out).is_ok() {
                        let pts = self
                            .queue
                            .back()
                            .map_or(0.0, |b| b.pts_secs + b.duration_secs());
                        self.push_resampled(pts, &out);
                    }
                    self.flushed = true;
                    continue;
                }
                Err(e) if is_again(&e) => {}
                Err(e) => return Err(e.into()),
            }

            loop {
                let mut packet = ffmpeg::Packet::empty();
                match packet.read(&mut self.ictx) {
                    Ok(()) => {
                        if packet.stream() != self.stream_index {
                            continue;
                        }
                        match self.decoder.send_packet(&packet) {
                            Ok(()) | Err(ffmpeg::Error::InvalidData) => {}
                            Err(e) if is_again(&e) => {}
                            Err(e) => return Err(e.into()),
                        }
                        break;
                    }
                    Err(ffmpeg::Error::Eof) => {
                        self.decoder.send_eof()?;
                        self.eof_sent = true;
                        break;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }

    /// Seek so the next buffer starts at (or just after) `secs`.
    pub fn seek(&mut self, secs: f64) -> Result<()> {
        let secs = secs.max(0.0);
        seek_on_stream(
            &mut self.ictx,
            self.stream_index,
            self.time_base,
            secs + self.origin_secs,
        )?;
        self.decoder.flush();
        self.queue.clear();
        self.eof_sent = false;
        self.flushed = false;
        self.skip_before = Some(secs);
        Ok(())
    }

    /// Decode the whole remaining stream into one buffer. Only for short clips and tests.
    pub fn read_all(&mut self) -> Result<AudioBuffer> {
        let mut samples = Vec::new();
        let mut first = None;
        while let Some(buf) = self.next_buffer()? {
            first.get_or_insert(buf.pts_secs);
            samples.extend_from_slice(&buf.samples);
        }
        Ok(AudioBuffer {
            sample_rate: self.out_rate,
            channels: self.out_channels,
            pts_secs: first.unwrap_or(0.0),
            samples,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::rotate_rgba;

    /// 3x2 image, each pixel's first byte is its index:
    ///   0 1 2
    ///   3 4 5
    fn sample() -> Vec<u8> {
        (0..6u8).flat_map(|i| [i, 0, 0, 255]).collect()
    }

    fn ids(data: &[u8]) -> Vec<u8> {
        data.chunks(4).map(|p| p[0]).collect()
    }

    #[test]
    fn rotates_clockwise() {
        let (d, w, h) = rotate_rgba(sample(), 3, 2, 90);
        assert_eq!((w, h), (2, 3));
        assert_eq!(ids(&d), vec![3, 0, 4, 1, 5, 2]);
    }

    #[test]
    fn rotates_half_turn() {
        let (d, w, h) = rotate_rgba(sample(), 3, 2, 180);
        assert_eq!((w, h), (3, 2));
        assert_eq!(ids(&d), vec![5, 4, 3, 2, 1, 0]);
    }

    #[test]
    fn rotates_counter_clockwise() {
        let (d, w, h) = rotate_rgba(sample(), 3, 2, 270);
        assert_eq!((w, h), (2, 3));
        assert_eq!(ids(&d), vec![2, 5, 1, 4, 0, 3]);
    }

    #[test]
    fn zero_is_untouched_and_four_quarter_turns_restore() {
        let (d, w, h) = rotate_rgba(sample(), 3, 2, 0);
        assert_eq!((w, h, ids(&d)), (3, 2, vec![0, 1, 2, 3, 4, 5]));
        let (mut d, mut w, mut h) = (sample(), 3, 2);
        for _ in 0..4 {
            (d, w, h) = rotate_rgba(d, w, h, 90);
        }
        assert_eq!((w, h, ids(&d)), (3, 2, vec![0, 1, 2, 3, 4, 5]));
    }
}
