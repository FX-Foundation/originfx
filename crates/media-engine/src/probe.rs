//! Read container and stream metadata without decoding any frames.

use std::path::Path;

use ffmpeg::media::Type;
use ffmpeg_next as ffmpeg;

use crate::error::{Error, Result};
use crate::types::{AudioStreamInfo, MediaInfo, Rational, SubtitleStreamInfo, VideoStreamInfo};

/// Open `path` and describe it.
///
/// This only reads headers, so it is fast even on very large files.
pub fn probe(path: impl AsRef<Path>) -> Result<MediaInfo> {
    let path = path.as_ref();
    crate::init();

    let meta = std::fs::metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Error::NotFound(path.to_path_buf()),
        _ => Error::Io(e),
    })?;

    let ictx = ffmpeg::format::input(&path)?;

    let mut video = Vec::new();
    let mut audio = Vec::new();
    let mut subtitles = Vec::new();

    for stream in ictx.streams() {
        let params = stream.parameters();
        let codec = params.id().name().to_string();
        let tb = stream.time_base();
        let duration_secs = stream_duration_secs(stream.duration(), tb);
        let language = stream.metadata().get("language").map(str::to_owned);

        match params.medium() {
            Type::Video => {
                // Attached pictures (cover art) show up as video streams; skip them.
                if stream
                    .disposition()
                    .contains(ffmpeg::format::stream::Disposition::ATTACHED_PIC)
                {
                    continue;
                }
                let dec = ffmpeg::codec::context::Context::from_parameters(params)?
                    .decoder()
                    .video()?;
                let avg = Rational::from(stream.avg_frame_rate());
                let rotation = stream_rotation(&stream);
                video.push(VideoStreamInfo {
                    index: stream.index(),
                    codec,
                    width: dec.width(),
                    height: dec.height(),
                    pixel_format: format!("{:?}", dec.format()).to_lowercase(),
                    frame_rate: avg.is_valid().then_some(avg),
                    time_base: Rational::from(tb),
                    frames: u64::try_from(stream.frames()).ok().filter(|&n| n > 0),
                    duration_secs,
                    bit_rate: Some(dec.bit_rate() as u64).filter(|&b| b > 0),
                    rotation,
                    colour: crate::colour::ColourInfo::from_decoder(&dec),
                });
            }
            Type::Audio => {
                let dec = ffmpeg::codec::context::Context::from_parameters(params)?
                    .decoder()
                    .audio()?;
                audio.push(AudioStreamInfo {
                    index: stream.index(),
                    codec,
                    sample_rate: dec.rate(),
                    channels: dec.channels(),
                    sample_format: format!("{:?}", dec.format()).to_lowercase(),
                    duration_secs,
                    bit_rate: Some(dec.bit_rate() as u64).filter(|&b| b > 0),
                    language,
                });
            }
            Type::Subtitle => subtitles.push(SubtitleStreamInfo {
                index: stream.index(),
                codec,
                language,
            }),
            _ => {}
        }
    }

    let container_duration = ictx.duration();
    let duration_secs = (container_duration > 0)
        .then(|| container_duration as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE));

    let metadata = ictx
        .metadata()
        .iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();

    Ok(MediaInfo {
        path: path.to_path_buf(),
        format_name: ictx.format().name().to_owned(),
        format_long_name: ictx.format().description().to_owned(),
        size_bytes: meta.len(),
        duration_secs,
        bit_rate: u64::try_from(ictx.bit_rate()).ok().filter(|&b| b > 0),
        video,
        audio,
        subtitles,
        metadata,
    })
}

/// Clockwise rotation (0, 90, 180, 270) a stream's display matrix asks for.
pub(crate) fn stream_rotation(stream: &ffmpeg::format::stream::Stream) -> u32 {
    let params = stream.parameters();
    // SAFETY: `params` owns a valid AVCodecParameters for the duration of this
    // call. `coded_side_data` is an array of `nb_coded_side_data` entries owned
    // by it, and we only read entries inside that range. A display matrix is 9
    // i32 values (36 bytes), which we check before handing the pointer to
    // `av_display_rotation_get`.
    unsafe {
        let p = params.as_ptr();
        let n = usize::try_from((*p).nb_coded_side_data).unwrap_or(0);
        let data = (*p).coded_side_data;
        if data.is_null() {
            return 0;
        }
        for i in 0..n {
            let sd = &*data.add(i);
            if sd.type_ == ffmpeg::ffi::AVPacketSideDataType::AV_PKT_DATA_DISPLAYMATRIX
                && sd.size >= 36
                && !sd.data.is_null()
            {
                let angle = ffmpeg::ffi::av_display_rotation_get(sd.data as *const i32);
                if angle.is_nan() {
                    return 0;
                }
                // FFmpeg reports counter-clockwise degrees; flip to clockwise and snap to 90s.
                let cw = (-angle).rem_euclid(360.0);
                return (((cw / 90.0).round() as u32) % 4) * 90;
            }
        }
    }
    0
}

/// Convert a stream duration (in `time_base` units) to seconds.
/// FFmpeg uses `i64::MIN` for "unknown", which we map to `None`.
pub(crate) fn stream_duration_secs(duration: i64, tb: ffmpeg::Rational) -> Option<f64> {
    if duration <= 0 || tb.denominator() == 0 {
        return None;
    }
    Some(duration as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator()))
}
