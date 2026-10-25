//! Plain data types that cross the Media Engine boundary.
//!
//! None of these types expose FFmpeg, so the rest of OriginFX never has to
//! depend on FFmpeg directly.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// A rational number (frame rates, time bases).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational {
    pub num: i32,
    pub den: i32,
}

impl Rational {
    pub const fn new(num: i32, den: i32) -> Self {
        Self { num, den }
    }

    /// Value as a float; `0.0` when the denominator is zero.
    pub fn as_f64(self) -> f64 {
        if self.den == 0 {
            0.0
        } else {
            f64::from(self.num) / f64::from(self.den)
        }
    }

    /// True when this is a usable positive rate.
    pub fn is_valid(self) -> bool {
        self.num > 0 && self.den > 0
    }
}

impl std::fmt::Display for Rational {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.num, self.den)
    }
}

impl From<ffmpeg_next::Rational> for Rational {
    fn from(r: ffmpeg_next::Rational) -> Self {
        Self::new(r.numerator(), r.denominator())
    }
}

/// Description of one video stream.
#[derive(Debug, Clone)]
pub struct VideoStreamInfo {
    /// Stream index inside the container.
    pub index: usize,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub pixel_format: String,
    /// Average frame rate, if the container knows it.
    pub frame_rate: Option<Rational>,
    pub time_base: Rational,
    /// Frame count if the container stores it (not all do).
    pub frames: Option<u64>,
    pub duration_secs: Option<f64>,
    pub bit_rate: Option<u64>,
}

/// Description of one audio stream.
#[derive(Debug, Clone)]
pub struct AudioStreamInfo {
    pub index: usize,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: String,
    pub duration_secs: Option<f64>,
    pub bit_rate: Option<u64>,
    pub language: Option<String>,
}

/// Description of one subtitle stream.
#[derive(Debug, Clone)]
pub struct SubtitleStreamInfo {
    pub index: usize,
    pub codec: String,
    pub language: Option<String>,
}

/// Everything [`crate::probe`] learns about a file.
#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub path: PathBuf,
    pub format_name: String,
    pub format_long_name: String,
    pub size_bytes: u64,
    pub duration_secs: Option<f64>,
    pub bit_rate: Option<u64>,
    pub video: Vec<VideoStreamInfo>,
    pub audio: Vec<AudioStreamInfo>,
    pub subtitles: Vec<SubtitleStreamInfo>,
    pub metadata: BTreeMap<String, String>,
}

impl MediaInfo {
    /// The first video stream, which is what the editor shows by default.
    pub fn primary_video(&self) -> Option<&VideoStreamInfo> {
        self.video.first()
    }

    /// The first audio stream.
    pub fn primary_audio(&self) -> Option<&AudioStreamInfo> {
        self.audio.first()
    }
}

/// One decoded video frame in tightly packed RGBA8 (`width * height * 4` bytes).
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    /// Presentation time in seconds from the start of the stream.
    pub pts_secs: f64,
    /// RGBA8, row-major, no padding between rows.
    pub data: Vec<u8>,
}

impl VideoFrame {
    /// Size of the pixel buffer in bytes.
    pub fn byte_len(&self) -> usize {
        self.data.len()
    }
}

/// A block of decoded audio: interleaved 32-bit float samples.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    pub sample_rate: u32,
    pub channels: u16,
    /// Presentation time of the first sample, in seconds.
    pub pts_secs: f64,
    /// Interleaved samples, `frames * channels` values.
    pub samples: Vec<f32>,
}

impl AudioBuffer {
    /// Number of sample frames (one frame = one sample per channel).
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / usize::from(self.channels)
        }
    }

    /// Duration of this block in seconds.
    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames() as f64 / f64::from(self.sample_rate)
        }
    }
}
