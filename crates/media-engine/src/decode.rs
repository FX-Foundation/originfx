//! Software video and audio decoding with frame-accurate seeking.

use std::collections::VecDeque;
use std::path::Path;

use ffmpeg::format::Pixel;
use ffmpeg::software::scaling::{Context as Scaler, Flags};
use ffmpeg::media::Type;
use ffmpeg_next as ffmpeg;

use crate::error::{Error, Result};
use crate::probe::stream_duration_secs;
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

/// Stream start offset in seconds; many transport streams do not start at 0.
fn start_offset_secs(stream: &ffmpeg::format::stream::Stream) -> f64 {
    let start = stream.start_time();
    let tb = stream.time_base();
    if start > 0 && tb.denominator() != 0 {
        start as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator())
    } else {
        0.0
    }
}

fn to_av_time_base(secs: f64) -> i64 {
    (secs.max(0.0) * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64
}

// ---------------------------------------------------------------------------
// Video
// ---------------------------------------------------------------------------

/// Sequential video decoder that yields RGBA frames and can seek.
///
/// Frames come out in presentation order. [`VideoDecoder::seek`] is
/// frame-accurate: after it, the next frame returned is the one that is on
/// screen at the requested time.
pub struct VideoDecoder {
    ictx: ffmpeg::format::context::Input,
    stream_index: usize,
    time_base: ffmpeg::Rational,
    origin_secs: f64,
    decoder: ffmpeg::decoder::Video,
    scaler: Option<(Pixel, u32, u32, Scaler)>,
    info: VideoStreamInfo,
    eof_sent: bool,
    /// A decoded frame that was read ahead and must be returned first.
    held: Option<ffmpeg::frame::Video>,
    /// After a seek: the time whose frame we are looking for.
    target: Option<f64>,
}

impl VideoDecoder {
    /// Open the best video stream of `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_stream(path, None)
    }

    /// Open a specific video stream (`None` picks the best one).
    pub fn open_stream(path: impl AsRef<Path>, stream_index: Option<usize>) -> Result<Self> {
        let path = path.as_ref();
        let ictx = open_input(path)?;

        let stream = match stream_index {
            Some(i) => ictx
                .stream(i)
                .filter(|s| s.parameters().medium() == Type::Video)
                .ok_or(Error::NoStream("video"))?,
            None => ictx.streams().best(Type::Video).ok_or(Error::NoStream("video"))?,
        };
        let index = stream.index();
        let time_base = stream.time_base();
        let origin_secs = start_offset_secs(&stream);

        let mut ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        ctx.set_threading(ffmpeg::threading::Config {
            kind: ffmpeg::threading::Type::Frame,
            count: 0, // 0 = pick automatically from CPU count
        });
        let decoder = ctx.decoder().video()?;

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
        };
        drop(stream);

        Ok(Self {
            ictx,
            stream_index: index,
            time_base,
            origin_secs,
            decoder,
            scaler: None,
            info,
            eof_sent: false,
            held: None,
            target: None,
        })
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
                Ok(()) => return Ok(Some(frame)),
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(e) if is_again(&e) => {}
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

    /// Convert a raw decoder frame to tightly packed RGBA.
    fn convert(&mut self, raw: &ffmpeg::frame::Video) -> Result<VideoFrame> {
        let (w, h, fmt) = (raw.width(), raw.height(), raw.format());
        let needs_new = !matches!(&self.scaler, Some((f, sw, sh, _)) if *f == fmt && *sw == w && *sh == h);
        if needs_new {
            let scaler = Scaler::get(fmt, w, h, Pixel::RGBA, w, h, Flags::BILINEAR)?;
            self.scaler = Some((fmt, w, h, scaler));
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
        Ok(VideoFrame {
            width: w,
            height: h,
            pts_secs: self.pts_secs(raw),
            data,
        })
    }

    /// Decode the next frame, or `None` at the end of the stream.
    pub fn next_frame(&mut self) -> Result<Option<VideoFrame>> {
        let Some(target) = self.target.take() else {
            return match self.decode_raw()? {
                Some(raw) => Ok(Some(self.convert(&raw)?)),
                None => Ok(None),
            };
        };

        // Just after a seek: we landed on a keyframe at or before `target`.
        // Decode forward and return the last frame whose start time is <= target.
        let mut previous: Option<ffmpeg::frame::Video> = None;
        loop {
            match self.decode_raw()? {
                None => {
                    return match previous {
                        Some(raw) => Ok(Some(self.convert(&raw)?)),
                        None => Ok(None),
                    }
                }
                Some(raw) => {
                    if self.pts_secs(&raw) > target + 1e-6 {
                        // `raw` is already after the target; `previous` is on screen.
                        let chosen = match previous {
                            Some(prev) => {
                                self.held = Some(raw);
                                prev
                            }
                            None => raw,
                        };
                        return Ok(Some(self.convert(&chosen)?));
                    }
                    previous = Some(raw);
                }
            }
        }
    }

    /// Seek so that the next [`next_frame`](Self::next_frame) returns the
    /// frame displayed at `secs`.
    pub fn seek(&mut self, secs: f64) -> Result<()> {
        let secs = secs.max(0.0);
        let ts = to_av_time_base(secs + self.origin_secs);
        self.ictx.seek(ts, ..ts)?;
        self.decoder.flush();
        self.eof_sent = false;
        self.held = None;
        self.target = Some(secs);
        Ok(())
    }

    /// Convenience: seek to `secs` and return the frame shown there.
    pub fn frame_at(&mut self, secs: f64) -> Result<Option<VideoFrame>> {
        self.seek(secs)?;
        self.next_frame()
    }
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
        let stream = ictx.streams().best(Type::Audio).ok_or(Error::NoStream("audio"))?;
        let index = stream.index();
        let time_base = stream.time_base();
        let origin_secs = start_offset_secs(&stream);

        let ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = ctx.decoder().audio()?;
        drop(stream);

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
            decoder,
            resampler,
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

    fn push_resampled(&mut self, pts: f64, out: &ffmpeg::frame::Audio) {
        if out.samples() == 0 {
            return;
        }
        // Packed f32: `plane::<f32>` only exposes one value per sample frame,
        // so read the raw bytes to get every channel.
        let wanted = out.samples() * usize::from(self.out_channels) * 4;
        let bytes = out.data(0);
        let samples: Vec<f32> = bytes[..wanted.min(bytes.len())]
            .chunks_exact(4)
            .map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
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
                    let mut out = ffmpeg::frame::Audio::empty();
                    self.resampler.run(&frame, &mut out)?;
                    self.push_resampled(pts, &out);
                    continue;
                }
                Err(ffmpeg::Error::Eof) => {
                    let mut out = ffmpeg::frame::Audio::empty();
                    if self.resampler.flush(&mut out).is_ok() {
                        let pts = self.queue.back().map_or(0.0, |b| b.pts_secs + b.duration_secs());
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
        let ts = to_av_time_base(secs + self.origin_secs);
        self.ictx.seek(ts, ..ts)?;
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
