//! Software video and audio decoding with frame-accurate seeking.

use std::collections::VecDeque;
use std::path::Path;

use ffmpeg::format::Pixel;
use ffmpeg::software::scaling::{Context as Scaler, Flags};
use ffmpeg::media::Type;
use ffmpeg_next as ffmpeg;

use crate::error::{Error, Result};
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
    /// Where the last seek actually pointed (seconds from stream start).
    seek_pos: f64,
    /// How far to step back if a seek lands after its target.
    retry_step: f64,
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
            rotation: stream_rotation(&stream),
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
            seek_pos: 0.0,
            retry_step: 1.0,
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
        let pts_secs = self.pts_secs(raw);
        let (data, w, h) = rotate_rgba(data, w, h, self.info.rotation);
        Ok(VideoFrame { width: w, height: h, pts_secs, data })
    }

    /// Decode the next frame, or `None` at the end of the stream.
    pub fn next_frame(&mut self) -> Result<Option<VideoFrame>> {
        let Some(target) = self.target.take() else {
            return match self.decode_raw()? {
                Some(raw) => Ok(Some(self.convert(&raw)?)),
                None => Ok(None),
            };
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
                    return match previous {
                        Some(raw) => Ok(Some(self.convert(&raw)?)),
                        None => Ok(None),
                    };
                }
                Some(raw) => {
                    let pts = self.pts_secs(&raw);
                    if previous.is_none() && pts > target + 1e-6 && self.seek_pos > 0.0 {
                        let back_to = (self.seek_pos - self.retry_step).max(0.0);
                        self.retry_step *= 2.0;
                        self.reseek(back_to)?;
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
                        return Ok(Some(self.convert(&chosen)?));
                    }
                    previous = Some(raw);
                }
            }
        }
    }

    /// Low-level seek used by [`seek`](Self::seek) and its retry logic.
    fn reseek(&mut self, secs: f64) -> Result<()> {
        seek_on_stream(&mut self.ictx, self.stream_index, self.time_base, secs + self.origin_secs)?;
        self.decoder.flush();
        self.eof_sent = false;
        self.held = None;
        self.seek_pos = secs;
        Ok(())
    }

    /// Seek so that the next [`next_frame`](Self::next_frame) returns the
    /// frame displayed at `secs`.
    pub fn seek(&mut self, secs: f64) -> Result<()> {
        let secs = secs.max(0.0);
        self.reseek(secs)?;
        self.retry_step = 1.0;
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
        let delay = self.resampler.delay().map_or(0, |d| d.output.max(0) as usize);
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
                    let mut out = self.alloc_out(frame.samples());
                    self.resampler.run(&frame, &mut out)?;
                    self.push_resampled(pts, &out);
                    continue;
                }
                Err(ffmpeg::Error::Eof) => {
                    let mut out = self.alloc_out(0);
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
        seek_on_stream(&mut self.ictx, self.stream_index, self.time_base, secs + self.origin_secs)?;
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
