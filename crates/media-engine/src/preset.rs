//! Export presets: which container, codecs and defaults each output format uses.

use ffmpeg_next as ffmpeg;

use crate::error::{Error, Result};

/// Overall quality/speed trade-off, mapped to each encoder's own knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    /// Fast and small; for previews and proxies.
    Draft,
    /// Good default for sharing.
    #[default]
    Standard,
    /// Slow and large; for final masters.
    High,
}

/// How an encoder is configured for one preset.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VideoSpec {
    /// Encoder names to try in order (first one present in this FFmpeg build wins).
    pub encoders: &'static [&'static str],
    pub pix_fmt: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AudioSpec {
    pub encoders: &'static [&'static str],
    /// Fixed sample rate required by the codec, if any.
    pub fixed_rate: Option<u32>,
    /// Lossless / PCM codecs ignore bitrate.
    pub lossy: bool,
    /// Most channels the format can carry (2 = stereo only).
    pub max_channels: u16,
}

/// A ready-made output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportPreset {
    // --- video + audio ---
    Mp4H264,
    Mp4H265,
    MovH264,
    MkvH264,
    MkvH265,
    WebmVp9,
    WebmVp8,
    WebmAv1,
    MovProRes422,
    MovProRes422Hq,
    MovProRes4444,
    MovDnxhrHq,
    AviMjpeg,
    // --- video only ---
    Gif,
    // --- audio only ---
    Mp3,
    AacM4a,
    Wav,
    Flac,
    OpusOgg,
}

impl ExportPreset {
    /// Every preset, in display order.
    pub const ALL: [ExportPreset; 19] = [
        Self::Mp4H264,
        Self::Mp4H265,
        Self::MovH264,
        Self::MkvH264,
        Self::MkvH265,
        Self::WebmVp9,
        Self::WebmVp8,
        Self::WebmAv1,
        Self::MovProRes422,
        Self::MovProRes422Hq,
        Self::MovProRes4444,
        Self::MovDnxhrHq,
        Self::AviMjpeg,
        Self::Gif,
        Self::Mp3,
        Self::AacM4a,
        Self::Wav,
        Self::Flac,
        Self::OpusOgg,
    ];

    /// Stable machine name, used by the CLI (`--preset mp4-h264`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Mp4H264 => "mp4-h264",
            Self::Mp4H265 => "mp4-h265",
            Self::MovH264 => "mov-h264",
            Self::MkvH264 => "mkv-h264",
            Self::MkvH265 => "mkv-h265",
            Self::WebmVp9 => "webm-vp9",
            Self::WebmVp8 => "webm-vp8",
            Self::WebmAv1 => "webm-av1",
            Self::MovProRes422 => "mov-prores-422",
            Self::MovProRes422Hq => "mov-prores-422hq",
            Self::MovProRes4444 => "mov-prores-4444",
            Self::MovDnxhrHq => "mov-dnxhr-hq",
            Self::AviMjpeg => "avi-mjpeg",
            Self::Gif => "gif",
            Self::Mp3 => "mp3",
            Self::AacM4a => "m4a-aac",
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::OpusOgg => "ogg-opus",
        }
    }

    /// Human readable name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Mp4H264 => "MP4 (H.264 + AAC)",
            Self::Mp4H265 => "MP4 (H.265/HEVC + AAC)",
            Self::MovH264 => "MOV (H.264 + AAC)",
            Self::MkvH264 => "MKV (H.264 + AAC)",
            Self::MkvH265 => "MKV (H.265/HEVC + AAC)",
            Self::WebmVp9 => "WebM (VP9 + Opus)",
            Self::WebmVp8 => "WebM (VP8 + Opus)",
            Self::WebmAv1 => "WebM (AV1 + Opus)",
            Self::MovProRes422 => "MOV (ProRes 422 + PCM)",
            Self::MovProRes422Hq => "MOV (ProRes 422 HQ + PCM)",
            Self::MovProRes4444 => "MOV (ProRes 4444 + PCM)",
            Self::MovDnxhrHq => "MOV (DNxHR HQ + PCM)",
            Self::AviMjpeg => "AVI (Motion JPEG + PCM)",
            Self::Gif => "Animated GIF",
            Self::Mp3 => "MP3 audio",
            Self::AacM4a => "M4A audio (AAC)",
            Self::Wav => "WAV audio (PCM 16-bit)",
            Self::Flac => "FLAC audio (lossless)",
            Self::OpusOgg => "Ogg audio (Opus)",
        }
    }

    /// Look a preset up by its [`id`](Self::id).
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|p| p.id().eq_ignore_ascii_case(id))
    }

    /// File extension (without the dot) outputs of this preset should use.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4H264 | Self::Mp4H265 => "mp4",
            Self::MovH264
            | Self::MovProRes422
            | Self::MovProRes422Hq
            | Self::MovProRes4444
            | Self::MovDnxhrHq => "mov",
            Self::MkvH264 | Self::MkvH265 => "mkv",
            Self::WebmVp9 | Self::WebmVp8 | Self::WebmAv1 => "webm",
            Self::AviMjpeg => "avi",
            Self::Gif => "gif",
            Self::Mp3 => "mp3",
            Self::AacM4a => "m4a",
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::OpusOgg => "ogg",
        }
    }

    /// FFmpeg muxer name.
    pub(crate) fn muxer(self) -> &'static str {
        match self {
            Self::Mp4H264 | Self::Mp4H265 => "mp4",
            Self::MovH264
            | Self::MovProRes422
            | Self::MovProRes422Hq
            | Self::MovProRes4444
            | Self::MovDnxhrHq => "mov",
            Self::MkvH264 | Self::MkvH265 => "matroska",
            Self::WebmVp9 | Self::WebmVp8 | Self::WebmAv1 => "webm",
            Self::AviMjpeg => "avi",
            Self::Gif => "gif",
            Self::Mp3 => "mp3",
            Self::AacM4a => "ipod",
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::OpusOgg => "ogg",
        }
    }

    pub(crate) fn video(self) -> Option<VideoSpec> {
        let (encoders, pix_fmt): (&'static [&'static str], &'static str) = match self {
            Self::Mp4H264 | Self::MovH264 | Self::MkvH264 => (&["libx264"], "yuv420p"),
            Self::Mp4H265 | Self::MkvH265 => (&["libx265"], "yuv420p"),
            Self::WebmVp9 => (&["libvpx-vp9"], "yuv420p"),
            Self::WebmVp8 => (&["libvpx"], "yuv420p"),
            Self::WebmAv1 => (&["libsvtav1", "libaom-av1"], "yuv420p"),
            Self::MovProRes422 | Self::MovProRes422Hq => (&["prores_ks"], "yuv422p10le"),
            Self::MovProRes4444 => (&["prores_ks"], "yuv444p10le"),
            Self::MovDnxhrHq => (&["dnxhd"], "yuv422p"),
            Self::AviMjpeg => (&["mjpeg"], "yuvj420p"),
            Self::Gif => (&["gif"], "pal8"),
            Self::Mp3 | Self::AacM4a | Self::Wav | Self::Flac | Self::OpusOgg => return None,
        };
        Some(VideoSpec { encoders, pix_fmt })
    }

    pub(crate) fn audio(self) -> Option<AudioSpec> {
        let (encoders, fixed_rate, lossy): (&'static [&'static str], Option<u32>, bool) = match self
        {
            Self::Mp4H264 | Self::Mp4H265 | Self::MovH264 | Self::MkvH264 | Self::MkvH265 => {
                (&["aac"], None, true)
            }
            Self::WebmVp9 | Self::WebmVp8 | Self::WebmAv1 | Self::OpusOgg => {
                (&["libopus"], Some(48_000), true)
            }
            Self::MovProRes422
            | Self::MovProRes422Hq
            | Self::MovProRes4444
            | Self::MovDnxhrHq
            | Self::AviMjpeg
            | Self::Wav => (&["pcm_s16le"], None, false),
            Self::Mp3 => (&["libmp3lame"], None, true),
            Self::AacM4a => (&["aac"], None, true),
            Self::Flac => (&["flac"], None, false),
            Self::Gif => return None,
        };
        let max_channels = match self {
            Self::Mp3 => 2,
            _ => 8,
        };
        Some(AudioSpec {
            encoders,
            fixed_rate,
            lossy,
            max_channels,
        })
    }

    /// True for presets that need a video stream in the input.
    pub fn needs_video(self) -> bool {
        self.video().is_some()
    }

    /// True for presets that produce no video at all.
    pub fn audio_only(self) -> bool {
        self.video().is_none()
    }

    /// Which of this preset's encoders are present in the linked FFmpeg.
    /// Returns `Ok(())` when everything the preset needs is available.
    pub fn check_available(self) -> Result<()> {
        crate::init();
        if let Some(v) = self.video() {
            find_encoder(v.encoders)?;
        }
        if let Some(a) = self.audio() {
            find_encoder(a.encoders)?;
        }
        Ok(())
    }

    /// Convenience wrapper around [`check_available`](Self::check_available).
    pub fn is_available(self) -> bool {
        self.check_available().is_ok()
    }
}

/// First encoder from `names` that this FFmpeg build contains.
pub(crate) fn find_encoder(
    names: &'static [&'static str],
) -> Result<(ffmpeg::Codec, &'static str)> {
    for &name in names {
        if let Some(codec) = ffmpeg::encoder::find_by_name(name) {
            return Ok((codec, name));
        }
    }
    Err(Error::Unsupported(format!(
        "none of the encoders [{}] is available in this FFmpeg build",
        names.join(", ")
    )))
}
