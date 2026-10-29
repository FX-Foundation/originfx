// SPDX-License-Identifier: GPL-3.0-or-later
//! Lossless working files: a way to hand finished frames and sound *to* the Media Engine.
//!
//! The rest of this crate reads a file and writes a file. A renderer, though, makes frames
//! out of thin air (a composite of many clips) and has no file to read them from. The
//! [`WorkingFileWriter`] takes those frames and samples and writes them into a lossless
//! Matroska file (Ut Video or FFV1 for the picture, 32-bit float PCM for the sound).
//! That file is then exported like any other with [`crate::export`], so rendered timelines
//! get every preset, quality setting, GPU encoder and progress report for free.
//!
//! The picture is stored as RGB (no YUV conversion, so nothing is lost), and the colour
//! rules of the exporter apply to it afterwards (HD sizes are treated as BT.709).
//!
//! On Unix the same writer can feed an in-memory pipe instead of a file
//! ([`WorkingFileWriter::create_pipe`]): the frames go uncompressed straight to
//! [`crate::export_stream_with_progress`], which encodes them while the next frames are still being
//! made. Nothing touches the disk and no encode/decode time is spent on the middle step.

use std::path::Path;

use ffmpeg::format::{Pixel, Sample};
use ffmpeg::{codec, format, frame, software, Packet, Rational};
use ffmpeg_next as ffmpeg;

use crate::error::{Error, Result};
use crate::transcode::{dictionary, new_encoder_context};
use crate::types::VideoFrame;

/// Samples sent to the encoder per audio frame.
const AUDIO_CHUNK: usize = 4096;

/// Sound layout of a working file: interleaved `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkingAudio {
    pub sample_rate: u32,
    /// 1 or 2.
    pub channels: u16,
}

/// What a working file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkingSpec {
    pub width: u32,
    pub height: u32,
    /// Frame rate as a fraction, e.g. 24000/1001.
    pub fps_num: u32,
    pub fps_den: u32,
    /// `None` for a silent file.
    pub audio: Option<WorkingAudio>,
}

struct VideoOut {
    encoder: ffmpeg::encoder::Video,
    index: usize,
    enc_tb: Rational,
    out_tb: Rational,
    /// `None` when the picture is stored as RGBA exactly as given.
    scaler: Option<software::scaling::Context>,
    frames: i64,
}

struct AudioOut {
    encoder: ffmpeg::encoder::Audio,
    index: usize,
    enc_tb: Rational,
    out_tb: Rational,
    spec: WorkingAudio,
    layout: ffmpeg::ChannelLayout,
    samples: i64,
}

/// The read end of a pipe made by [`WorkingFileWriter::create_pipe`].
#[cfg(unix)]
pub struct WorkingPipe {
    url: String,
    /// The frame rate of what is written, which a pipe cannot carry reliably on its own.
    pub(crate) fps: (u32, u32),
    _read_end: std::os::fd::OwnedFd,
}

#[cfg(unix)]
impl WorkingPipe {
    /// Where to read the stream from: pass it to [`crate::export_stream_with_progress`].
    pub fn url(&self) -> &str {
        &self.url
    }
}

/// Writes frames and sound into a lossless working file. Call [`finish`](Self::finish) at the end.
pub struct WorkingFileWriter {
    octx: format::context::Output,
    spec: WorkingSpec,
    video: VideoOut,
    audio: Option<AudioOut>,
}

/// Pixel layouts we accept, best first. All are RGB, so the conversion from RGBA is exact.
const PIXEL_PREFERENCE: [Pixel; 5] = [
    Pixel::RGBA,
    Pixel::BGRA,
    Pixel::GBRAP,
    Pixel::GBRP,
    Pixel::RGB24,
];

fn pick_video_encoder() -> Result<(ffmpeg::Codec, Pixel)> {
    for name in ["utvideo", "ffv1"] {
        let Some(codec) = ffmpeg::encoder::find_by_name(name) else {
            continue;
        };
        let Ok(video) = codec.video() else { continue };
        let Some(formats) = video.formats() else {
            continue;
        };
        let formats: Vec<Pixel> = formats.collect();
        if let Some(p) = PIXEL_PREFERENCE.iter().find(|p| formats.contains(p)) {
            return Ok((codec, *p));
        }
    }
    Err(Error::Unsupported(
        "this FFmpeg build has no lossless RGB video encoder (utvideo or ffv1)".into(),
    ))
}

impl WorkingFileWriter {
    /// Start a working file at `path` (use a `.nut` name).
    pub fn create(path: impl AsRef<Path>, spec: WorkingSpec) -> Result<Self> {
        Self::open(path.as_ref(), spec, false)
    }

    /// Start writing into an in-memory pipe. Hand the returned [`WorkingPipe`] to
    /// [`crate::export_stream_with_progress`] on another thread; frames are exported as they
    /// are written. Unix only.
    ///
    /// Uncompressed frames cross the pipe, so the reading side must keep up: if it stops (an
    /// error, say), writing fails with an error instead of blocking forever.
    #[cfg(unix)]
    pub fn create_pipe(spec: WorkingSpec) -> Result<(Self, WorkingPipe)> {
        use std::os::fd::{AsRawFd, OwnedFd};
        let (r, w) = std::io::pipe()?;
        let (r, w): (OwnedFd, OwnedFd) = (r.into(), w.into());
        let url = format!("/dev/fd/{}", w.as_raw_fd());
        // FFmpeg opens its own descriptor on the pipe; ours is closed straight away, so the
        // reader sees the end of the data as soon as the writer is done.
        let writer = Self::open(Path::new(&url), spec, true)?;
        drop(w);
        let pipe = WorkingPipe {
            fps: (spec.fps_num, spec.fps_den),
            url: format!("/dev/fd/{}", r.as_raw_fd()),
            _read_end: r,
        };
        Ok((writer, pipe))
    }

    fn open(path: &Path, spec: WorkingSpec, raw: bool) -> Result<Self> {
        crate::init();
        if spec.width == 0 || spec.height == 0 || spec.fps_num == 0 || spec.fps_den == 0 {
            return Err(Error::InvalidArgument(
                "working file needs a size and a frame rate".into(),
            ));
        }
        if let Some(a) = spec.audio {
            if !(1..=2).contains(&a.channels) || a.sample_rate == 0 {
                return Err(Error::InvalidArgument(
                    "working file audio must be mono or stereo with a sample rate".into(),
                ));
            }
        }
        let mut octx = format::output_as(&path, "nut")?;
        let global_header = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);

        // Video
        let (codec_found, pix) = if raw {
            let c = ffmpeg::encoder::find_by_name("rawvideo")
                .ok_or_else(|| Error::Unsupported("encoder rawvideo".into()))?;
            (c, Pixel::RGBA)
        } else {
            pick_video_encoder()?
        };
        let fps = Rational::new(spec.fps_num as i32, spec.fps_den as i32);
        let mut ost = octx.add_stream(codec_found)?;
        let v_index = ost.index();
        let mut ctx = new_encoder_context(&codec_found)?;
        // Both lossless encoders can split a picture into slices and encode them in parallel.
        ctx.set_threading(ffmpeg::threading::Config {
            kind: ffmpeg::threading::Type::Slice,
            count: 0,
        });
        let mut enc = ctx.encoder().video()?;
        enc.set_width(spec.width);
        enc.set_height(spec.height);
        enc.set_format(pix);
        enc.set_time_base(fps.invert());
        enc.set_frame_rate(Some(fps));
        if global_header {
            enc.set_flags(codec::Flags::GLOBAL_HEADER);
        }
        // More slices than cores keeps every core busy (the cost in file size is tiny).
        let slices = if codec_found.name() == "ffv1" {
            "24"
        } else {
            "16"
        };
        let opts = dictionary(&[("slices", slices.to_string())]);
        let opened = enc.open_as_with(codec_found, opts)?;
        ost.set_parameters(&opened);
        ost.set_time_base(fps.invert());
        ost.set_avg_frame_rate(fps);
        let scaler = if pix == Pixel::RGBA {
            None
        } else {
            Some(software::scaling::Context::get(
                Pixel::RGBA,
                spec.width,
                spec.height,
                pix,
                spec.width,
                spec.height,
                software::scaling::Flags::POINT,
            )?)
        };
        let video = VideoOut {
            encoder: opened,
            index: v_index,
            enc_tb: fps.invert(),
            out_tb: fps.invert(),
            scaler,
            frames: 0,
        };

        // Audio
        let audio = match spec.audio {
            None => None,
            Some(a) => {
                let codec_found = ffmpeg::encoder::find_by_name("pcm_f32le")
                    .ok_or_else(|| Error::Unsupported("encoder pcm_f32le".into()))?;
                let mut ost = octx.add_stream(codec_found)?;
                let index = ost.index();
                let layout = ffmpeg::ChannelLayout::default(i32::from(a.channels));
                let mut enc = new_encoder_context(&codec_found)?.encoder().audio()?;
                enc.set_rate(a.sample_rate as i32);
                enc.set_channel_layout(layout);
                enc.set_format(Sample::F32(ffmpeg::format::sample::Type::Packed));
                enc.set_time_base(Rational::new(1, a.sample_rate as i32));
                if global_header {
                    enc.set_flags(codec::Flags::GLOBAL_HEADER);
                }
                let opened = enc.open_as_with(codec_found, dictionary(&[]))?;
                ost.set_parameters(&opened);
                let tb = Rational::new(1, a.sample_rate as i32);
                ost.set_time_base(tb);
                Some(AudioOut {
                    encoder: opened,
                    index,
                    enc_tb: tb,
                    out_tb: tb,
                    spec: a,
                    layout,
                    samples: 0,
                })
            }
        };

        octx.write_header()?;
        let mut writer = WorkingFileWriter {
            octx,
            spec,
            video,
            audio,
        };
        // Muxers may adjust the stream time bases when writing the header.
        writer.video.out_tb = writer
            .octx
            .stream(writer.video.index)
            .map(|s| s.time_base())
            .unwrap_or(writer.video.enc_tb);
        if let Some(a) = writer.audio.as_mut() {
            a.out_tb = writer
                .octx
                .stream(a.index)
                .map(|s| s.time_base())
                .unwrap_or(a.enc_tb);
        }
        Ok(writer)
    }

    /// Number of video frames written so far.
    pub fn frames_written(&self) -> u64 {
        self.video.frames as u64
    }

    /// Number of audio samples (per channel) written so far.
    pub fn samples_written(&self) -> u64 {
        self.audio.as_ref().map_or(0, |a| a.samples as u64)
    }

    fn drain_video(&mut self) -> Result<()> {
        let mut pkt = Packet::empty();
        while self.video.encoder.receive_packet(&mut pkt).is_ok() {
            pkt.set_stream(self.video.index);
            if pkt.duration() == 0 {
                pkt.set_duration(1);
            }
            pkt.rescale_ts(self.video.enc_tb, self.video.out_tb);
            pkt.write_interleaved(&mut self.octx)?;
        }
        Ok(())
    }

    fn drain_audio(&mut self) -> Result<()> {
        let Some(a) = self.audio.as_mut() else {
            return Ok(());
        };
        let mut pkt = Packet::empty();
        while a.encoder.receive_packet(&mut pkt).is_ok() {
            pkt.set_stream(a.index);
            pkt.rescale_ts(a.enc_tb, a.out_tb);
            pkt.write_interleaved(&mut self.octx)?;
        }
        Ok(())
    }

    /// Append one picture. It must be exactly the size given in the spec.
    pub fn write_video(&mut self, frame: &VideoFrame) -> Result<()> {
        let (w, h) = (self.spec.width as usize, self.spec.height as usize);
        if frame.width as usize != w || frame.height as usize != h || frame.data.len() != w * h * 4
        {
            return Err(Error::InvalidArgument(format!(
                "frame is {}x{} but the working file is {}x{}",
                frame.width, frame.height, w, h
            )));
        }
        let src = frame::Video::new(Pixel::RGBA, w as u32, h as u32);
        let mut src = src;
        let stride = src.stride(0);
        {
            let dst = src.data_mut(0);
            for y in 0..h {
                dst[y * stride..y * stride + w * 4]
                    .copy_from_slice(&frame.data[y * w * 4..(y + 1) * w * 4]);
            }
        }
        let mut converted = match self.video.scaler.as_mut() {
            Some(scaler) => {
                let mut out = frame::Video::empty();
                scaler.run(&src, &mut out)?;
                out
            }
            None => src,
        };
        converted.set_pts(Some(self.video.frames));
        self.video.encoder.send_frame(&converted)?;
        self.video.frames += 1;
        self.drain_video()
    }

    /// Append sound: interleaved `f32` samples with the channel count from the spec.
    pub fn write_audio(&mut self, samples: &[f32]) -> Result<()> {
        let Some(a) = self.audio.as_mut() else {
            return Err(Error::InvalidArgument(
                "this working file was created without audio".into(),
            ));
        };
        let channels = usize::from(a.spec.channels);
        if !samples.len().is_multiple_of(channels) {
            return Err(Error::InvalidArgument(
                "audio length is not a whole number of sample frames".into(),
            ));
        }
        for chunk in samples.chunks(AUDIO_CHUNK * channels) {
            let n = chunk.len() / channels;
            let a = self.audio.as_mut().expect("checked above");
            let mut f = frame::Audio::new(
                Sample::F32(ffmpeg::format::sample::Type::Packed),
                n,
                a.layout,
            );
            f.set_rate(a.spec.sample_rate);
            {
                let dst = f.data_mut(0);
                for (i, s) in chunk.iter().enumerate() {
                    dst[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
                }
            }
            f.set_pts(Some(a.samples));
            a.samples += n as i64;
            a.encoder.send_frame(&f)?;
            self.drain_audio()?;
        }
        Ok(())
    }

    /// Flush the encoders and close the file.
    pub fn finish(mut self) -> Result<()> {
        self.video.encoder.send_eof()?;
        self.drain_video()?;
        if let Some(a) = self.audio.as_mut() {
            a.encoder.send_eof()?;
        }
        self.drain_audio()?;
        self.octx.write_trailer()?;
        Ok(())
    }
}
