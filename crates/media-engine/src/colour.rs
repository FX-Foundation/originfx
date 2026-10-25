//! Colour handling: which YUV matrix a clip uses, and how to tag exports.
//!
//! Video is stored as YUV, and the numbers only mean something together with
//! the matrix (BT.601 for SD, BT.709 for HD, BT.2020 for UHD/HDR) and the range
//! (limited "TV" 16-235 or full "PC" 0-255) they were encoded with. Many files
//! do not say, so we follow the same rule players use: HD-sized video is
//! BT.709, anything smaller is BT.601.

use ffmpeg::color::{Primaries, Range, Space, TransferCharacteristic};
use ffmpeg_next as ffmpeg;

/// Colour description of a video stream as the container reports it.
/// `None` means the file does not say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColourInfo {
    /// Matrix, e.g. `bt709`, `smpte170m`, `bt2020nc`.
    pub space: Option<String>,
    /// `tv` (limited) or `pc` (full).
    pub range: Option<String>,
    /// Primaries, e.g. `bt709`, `bt2020`.
    pub primaries: Option<String>,
    /// Transfer curve, e.g. `bt709`, `smpte2084` (HDR10), `arib-std-b67` (HLG).
    pub transfer: Option<String>,
}

impl ColourInfo {
    pub(crate) fn from_decoder(dec: &ffmpeg::decoder::Video) -> Self {
        Self::from_tags(
            dec.color_space(),
            dec.color_range(),
            dec.color_primaries(),
            dec.color_transfer_characteristic(),
        )
    }

    fn from_tags(
        space: Space,
        range: Range,
        primaries: Primaries,
        transfer: TransferCharacteristic,
    ) -> Self {
        Self {
            space: space.name().map(str::to_owned),
            range: range.name().map(str::to_owned),
            primaries: primaries.name().map(str::to_owned),
            transfer: transfer.name().map(str::to_owned),
        }
    }

    /// True for HDR transfer curves (PQ or HLG). Exports are 8-bit, so these
    /// keep their tags but lose dynamic range precision.
    pub fn is_hdr(&self) -> bool {
        matches!(
            self.transfer.as_deref(),
            Some("smpte2084") | Some("arib-std-b67")
        )
    }
}

/// Fully decided colour parameters (nothing unspecified).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub space: Space,
    pub full_range: bool,
    pub primaries: Primaries,
    pub transfer: TransferCharacteristic,
}

/// Fill in whatever the file leaves unspecified.
pub(crate) fn resolve(
    space: Space,
    range: Range,
    primaries: Primaries,
    transfer: TransferCharacteristic,
    width: u32,
    height: u32,
    full_range_format: bool,
) -> Resolved {
    let space = match space {
        Space::Unspecified | Space::Reserved => {
            if height >= 720 || width >= 1280 {
                Space::BT709
            } else {
                Space::SMPTE170M
            }
        }
        s => s,
    };
    let full_range = match range {
        Range::JPEG => true,
        Range::MPEG => false,
        Range::Unspecified => full_range_format,
    };
    let primaries = match primaries {
        Primaries::Unspecified | Primaries::Reserved | Primaries::Reserved0 => match space {
            Space::BT709 => Primaries::BT709,
            Space::BT2020NCL | Space::BT2020CL => Primaries::BT2020,
            _ => Primaries::SMPTE170M,
        },
        p => p,
    };
    let transfer = match transfer {
        TransferCharacteristic::Unspecified
        | TransferCharacteristic::Reserved
        | TransferCharacteristic::Reserved0 => match space {
            Space::BT709 => TransferCharacteristic::BT709,
            Space::BT2020NCL | Space::BT2020CL => TransferCharacteristic::BT2020_10,
            _ => TransferCharacteristic::SMPTE170M,
        },
        t => t,
    };
    Resolved {
        space,
        full_range,
        primaries,
        transfer,
    }
}

/// swscale coefficient-table id for a matrix.
pub(crate) fn sws_table_id(space: Space) -> i32 {
    match space {
        Space::BT709 => ffmpeg::ffi::SWS_CS_ITU709,
        Space::BT2020NCL | Space::BT2020CL => ffmpeg::ffi::SWS_CS_BT2020,
        Space::SMPTE240M => ffmpeg::ffi::SWS_CS_SMPTE240M,
        Space::FCC => ffmpeg::ffi::SWS_CS_FCC,
        _ => ffmpeg::ffi::SWS_CS_ITU601,
    }
}

/// Name of the matrix as the `scale` filter spells it.
pub(crate) fn filter_matrix_name(space: Space) -> &'static str {
    match space {
        Space::BT709 => "bt709",
        Space::BT2020NCL | Space::BT2020CL => "bt2020",
        Space::SMPTE240M => "smpte240m",
        Space::FCC => "fcc",
        _ => "bt601",
    }
}

/// Does this pixel format store luma/chroma (so a matrix applies), as opposed to RGB?
pub(crate) fn is_yuv_format(fmt: ffmpeg::format::Pixel) -> bool {
    let name = format!("{fmt:?}").to_lowercase();
    ["yuv", "nv", "p01", "p21", "yvu"]
        .iter()
        .any(|p| name.starts_with(p))
}

/// Is this a full-range ("JPEG") YUV pixel format?
pub(crate) fn is_full_range_format(fmt: ffmpeg::format::Pixel) -> bool {
    use ffmpeg::format::Pixel::*;
    matches!(fmt, YUVJ420P | YUVJ422P | YUVJ444P | YUVJ440P | YUVJ411P)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(space: Space, h: u32) -> Resolved {
        resolve(
            space,
            Range::Unspecified,
            Primaries::Unspecified,
            TransferCharacteristic::Unspecified,
            h * 16 / 9,
            h,
            false,
        )
    }

    #[test]
    fn untagged_hd_is_709_and_sd_is_601() {
        assert_eq!(r(Space::Unspecified, 1080).space, Space::BT709);
        assert_eq!(r(Space::Unspecified, 720).space, Space::BT709);
        assert_eq!(r(Space::Unspecified, 480).space, Space::SMPTE170M);
    }

    #[test]
    fn explicit_tags_win_over_the_size_guess() {
        assert_eq!(r(Space::SMPTE170M, 1080).space, Space::SMPTE170M);
        assert_eq!(r(Space::BT709, 480).space, Space::BT709);
        assert_eq!(r(Space::BT2020NCL, 2160).primaries, Primaries::BT2020);
    }

    #[test]
    fn range_defaults_to_limited_unless_the_format_is_full() {
        let limited = resolve(
            Space::BT709,
            Range::Unspecified,
            Primaries::BT709,
            TransferCharacteristic::BT709,
            1920,
            1080,
            false,
        );
        assert!(!limited.full_range);
        let full = resolve(
            Space::BT709,
            Range::Unspecified,
            Primaries::BT709,
            TransferCharacteristic::BT709,
            1920,
            1080,
            true,
        );
        assert!(full.full_range);
        let tagged = resolve(
            Space::BT709,
            Range::JPEG,
            Primaries::BT709,
            TransferCharacteristic::BT709,
            1920,
            1080,
            false,
        );
        assert!(tagged.full_range);
    }

    #[test]
    fn hdr_curves_are_detected() {
        let hdr = ColourInfo {
            transfer: Some("smpte2084".into()),
            ..Default::default()
        };
        assert!(hdr.is_hdr());
        assert!(!ColourInfo::default().is_hdr());
    }
}
