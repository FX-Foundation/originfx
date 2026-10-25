//! Hardware acceleration discovery.
//!
//! **Status: probing only.** This module reports which FFmpeg hardware device
//! types exist and whether a device can actually be opened on this machine.
//! Decoding and encoding on the GPU (NVDEC/NVENC, VAAPI, QSV, VideoToolbox,
//! AMF) are *not* wired into the decoder or encoder yet; they need real
//! hardware to develop against and are tracked in `docs/ROADMAP.md`.

use std::ffi::CStr;
use std::ptr;

use ffmpeg_next as ffmpeg;

/// A hardware acceleration API FFmpeg knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HwBackend {
    /// FFmpeg's name for it: `cuda`, `vaapi`, `qsv`, `videotoolbox`, `d3d11va`, ...
    pub name: String,
    /// Which vendor/platform this is typically used for.
    pub vendor: &'static str,
    /// True when a device of this type could be opened right now.
    pub usable: bool,
    /// Why it is not usable, when it is not.
    pub error: Option<String>,
}

fn vendor_of(name: &str) -> &'static str {
    match name {
        "cuda" => "NVIDIA (NVDEC/NVENC)",
        "vaapi" => "Intel / AMD on Linux (VA-API)",
        "qsv" => "Intel Quick Sync",
        "videotoolbox" => "Apple (VideoToolbox)",
        "d3d11va" | "dxva2" => "Windows (Direct3D)",
        "vulkan" => "Vulkan Video (cross-vendor)",
        "opencl" => "OpenCL",
        "vdpau" => "NVIDIA on Linux (VDPAU)",
        "drm" => "Linux DRM",
        "mediacodec" => "Android MediaCodec",
        _ => "other",
    }
}

fn error_text(code: i32) -> String {
    ffmpeg::Error::from(code).to_string()
}

/// List every hardware device type compiled into this FFmpeg and try to open each.
///
/// Opening a device is cheap and is the only reliable way to know whether the
/// driver, permissions and hardware are all in place.
pub fn probe_backends() -> Vec<HwBackend> {
    crate::init();
    let mut out = Vec::new();
    // SAFETY: `av_hwdevice_iterate_types` and `av_hwdevice_get_type_name` are
    // pure lookups into static tables. `av_hwdevice_ctx_create` fills `ctx`
    // with a new reference on success, which we release with `av_buffer_unref`.
    unsafe {
        let mut kind = ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE;
        loop {
            kind = ffmpeg::ffi::av_hwdevice_iterate_types(kind);
            if kind == ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
                break;
            }
            let name_ptr = ffmpeg::ffi::av_hwdevice_get_type_name(kind);
            if name_ptr.is_null() {
                continue;
            }
            let name = CStr::from_ptr(name_ptr).to_string_lossy().into_owned();

            let mut ctx: *mut ffmpeg::ffi::AVBufferRef = ptr::null_mut();
            let ret = ffmpeg::ffi::av_hwdevice_ctx_create(&mut ctx, kind, ptr::null(), ptr::null_mut(), 0);
            let (usable, error) = if ret >= 0 {
                ffmpeg::ffi::av_buffer_unref(&mut ctx);
                (true, None)
            } else {
                (false, Some(error_text(ret)))
            };
            out.push(HwBackend { vendor: vendor_of(&name), name, usable, error });
        }
    }
    out
}

/// Backends that can be opened on this machine.
pub fn usable_backends() -> Vec<HwBackend> {
    probe_backends().into_iter().filter(|b| b.usable).collect()
}

/// Hardware-capable encoders/decoders present in this FFmpeg build, by name
/// (`h264_nvenc`, `hevc_vaapi`, ...). Presence does not mean a device works.
pub fn hardware_codecs() -> Vec<String> {
    crate::init();
    const SUFFIXES: [&str; 6] = ["_nvenc", "_vaapi", "_qsv", "_videotoolbox", "_amf", "_cuvid"];
    const CODECS: [&str; 5] = ["h264", "hevc", "av1", "vp9", "mpeg2video"];
    let mut found = Vec::new();
    for codec in CODECS {
        for suffix in SUFFIXES {
            let name = format!("{codec}{suffix}");
            if ffmpeg::encoder::find_by_name(&name).is_some() || ffmpeg::decoder::find_by_name(&name).is_some() {
                found.push(name);
            }
        }
    }
    found
}
