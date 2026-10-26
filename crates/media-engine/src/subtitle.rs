//! Subtitle text and chapter helpers.
//!
//! Text subtitles (SRT, ASS/SSA, WebVTT, mov_text and friends) are read as plain
//! timed cues. Styling and positioning are dropped. Picture subtitles (PGS, DVD,
//! DVB) are not supported.

use std::path::Path;

use ffmpeg::codec::Id;
use ffmpeg::media::Type;
use ffmpeg_next as ffmpeg;

use crate::decode::start_offset_secs;
use crate::error::{Error, Result};
use crate::types::SubtitleCue;

/// Cue length used when the file does not say how long a line stays on screen.
const DEFAULT_CUE_SECS: f64 = 3.0;

/// True for subtitle codecs that carry text.
pub(crate) fn is_text_codec(id: Id) -> bool {
    matches!(
        id,
        Id::SUBRIP
            | Id::SRT
            | Id::ASS
            | Id::SSA
            | Id::WEBVTT
            | Id::MOV_TEXT
            | Id::TEXT
            | Id::SUBVIEWER
            | Id::SUBVIEWER1
            | Id::MICRODVD
            | Id::SAMI
            | Id::REALTEXT
            | Id::STL
            | Id::JACOSUB
            | Id::VPLAYER
            | Id::PJS
            | Id::MPL2
    )
}

/// Reduce one ASS "Dialogue" payload to plain text: drop the leading
/// `ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,` fields, the `{...}`
/// override tags, and turn `\N` / `\n` into line breaks.
pub(crate) fn ass_to_text(line: &str) -> String {
    let body = if line.matches(',').count() >= 8 {
        line.splitn(9, ',').nth(8).unwrap_or(line)
    } else {
        line
    };
    plain_text(body)
}

/// Strip `{...}` tags and `<...>` markup and expand ASS escapes.
fn plain_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0u32;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            '\\' => match chars.peek() {
                Some('N') | Some('n') => {
                    chars.next();
                    out.push('\n');
                }
                Some('h') => {
                    chars.next();
                    out.push(' ');
                }
                _ => out.push('\\'),
            },
            '<' => {
                // Skip simple HTML-like tags such as <i>, </b>, <font ...>.
                let mut tag = String::new();
                let mut closed = false;
                for t in chars.by_ref() {
                    if t == '>' {
                        closed = true;
                        break;
                    }
                    tag.push(t);
                    if tag.len() > 40 {
                        break;
                    }
                }
                if !closed {
                    out.push('<');
                    out.push_str(&tag);
                }
            }
            _ => out.push(c),
        }
    }
    out.trim().to_string()
}

/// Read all cues of subtitle stream `stream_index` (a container stream index, see
/// [`crate::probe`]). Times are seconds from the start of the media, the same time
/// zero the video and audio decoders use.
pub fn read_subtitles(path: impl AsRef<Path>, stream_index: usize) -> Result<Vec<SubtitleCue>> {
    let path = path.as_ref();
    crate::init();
    if !path.exists() {
        return Err(Error::NotFound(path.to_path_buf()));
    }
    let mut ictx = ffmpeg::format::input(&path)?;
    let origin = media_origin_secs(&ictx);
    let mut cues = read_cues(&mut ictx, stream_index)?;
    for c in &mut cues {
        c.start_secs -= origin;
        c.end_secs -= origin;
    }
    Ok(cues)
}

/// Time zero of the media: the earliest start of any video or audio stream.
pub(crate) fn media_origin_secs(ictx: &ffmpeg::format::context::Input) -> f64 {
    let o = ictx
        .streams()
        .filter(|s| matches!(s.parameters().medium(), Type::Video | Type::Audio))
        .map(|s| start_offset_secs(&s))
        .fold(f64::INFINITY, f64::min);
    if o.is_finite() {
        o
    } else {
        0.0
    }
}

/// Read cues with raw container times (no origin removed).
pub(crate) fn read_cues(
    ictx: &mut ffmpeg::format::context::Input,
    stream_index: usize,
) -> Result<Vec<SubtitleCue>> {
    let (params, tb) = {
        let s = ictx.stream(stream_index).ok_or_else(|| {
            Error::InvalidArgument(format!("no stream with index {stream_index}"))
        })?;
        if s.parameters().medium() != Type::Subtitle {
            return Err(Error::InvalidArgument(format!(
                "stream {stream_index} is not a subtitle stream"
            )));
        }
        (s.parameters(), s.time_base())
    };
    if !is_text_codec(params.id()) {
        return Err(Error::Unsupported(format!(
            "subtitle codec {} is a picture format; only text subtitles are supported",
            params.id().name()
        )));
    }
    let tbf = f64::from(tb.numerator()) / f64::from(tb.denominator().max(1));
    let mut dec = ffmpeg::codec::context::Context::from_parameters(params)?
        .decoder()
        .subtitle()?;

    let mut cues = Vec::new();
    for (stream, packet) in ictx.packets() {
        if stream.index() != stream_index {
            continue;
        }
        let mut sub = ffmpeg::Subtitle::new();
        if !dec.decode(&packet, &mut sub).unwrap_or(false) {
            continue;
        }
        let base = match (sub.pts(), packet.pts()) {
            (Some(p), _) => p as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE),
            (None, Some(p)) => p as f64 * tbf,
            _ => continue,
        };
        let start = base + f64::from(sub.start()) / 1000.0;
        let packet_len = packet.duration();
        let mut end = if sub.end() != u32::MAX && sub.end() > sub.start() {
            base + f64::from(sub.end()) / 1000.0
        } else if packet_len > 0 {
            start + packet_len as f64 * tbf
        } else {
            start + DEFAULT_CUE_SECS
        };
        if end <= start {
            end = start + DEFAULT_CUE_SECS;
        }
        let mut lines = Vec::new();
        for rect in sub.rects() {
            let text = match rect {
                ffmpeg::subtitle::Rect::Ass(a) => ass_to_text(a.get()),
                ffmpeg::subtitle::Rect::Text(t) => plain_text(t.get()),
                _ => String::new(),
            };
            if !text.is_empty() {
                lines.push(text);
            }
        }
        if !lines.is_empty() {
            cues.push(SubtitleCue {
                start_secs: start,
                end_secs: end,
                text: lines.join("\n"),
            });
        }
    }
    cues.sort_by(|a, b| a.start_secs.total_cmp(&b.start_secs));
    Ok(cues)
}

/// Keep the cues that overlap `[trim_start, trim_end]` and move them so the trim
/// start becomes time zero. `origin` is removed first.
pub(crate) fn rebase_cues(
    cues: Vec<SubtitleCue>,
    origin: f64,
    trim_start: f64,
    trim_end: Option<f64>,
) -> Vec<SubtitleCue> {
    let len = trim_end.map(|e| e - trim_start);
    cues.into_iter()
        .filter_map(|c| {
            let mut start = c.start_secs - origin - trim_start;
            let mut end = c.end_secs - origin - trim_start;
            if end <= 0.0 || len.is_some_and(|l| start >= l) {
                return None;
            }
            start = start.max(0.0);
            if let Some(l) = len {
                end = end.min(l);
            }
            (end > start).then_some(SubtitleCue {
                start_secs: start,
                end_secs: end,
                text: c.text,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ass_line_becomes_plain_text() {
        let line = r"12,0,Default,,0,0,0,,{\i1}Hello{\i0}\Nsecond \hline";
        assert_eq!(ass_to_text(line), "Hello\nsecond  line");
    }

    #[test]
    fn plain_text_strips_markup_but_keeps_commas() {
        assert_eq!(plain_text("<i>Well, hello</i>"), "Well, hello");
        assert_eq!(plain_text("a < b"), "a < b");
    }

    #[test]
    fn ass_text_with_commas_is_kept() {
        let line = "1,0,Default,,0,0,0,,one, two, three";
        assert_eq!(ass_to_text(line), "one, two, three");
    }

    #[test]
    fn rebase_trims_and_shifts() {
        let c = |s: f64, e: f64| SubtitleCue {
            start_secs: s,
            end_secs: e,
            text: "x".into(),
        };
        let out = rebase_cues(
            vec![c(0.0, 1.0), c(2.0, 4.0), c(9.0, 10.0), c(20.0, 21.0)],
            0.0,
            3.0,
            Some(10.0),
        );
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].start_secs, out[0].end_secs), (0.0, 1.0));
        assert_eq!((out[1].start_secs, out[1].end_secs), (6.0, 7.0));
    }
}
