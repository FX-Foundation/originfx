//! Hardware acceleration discovery.
//!
//! **Status:** probing works and is tested. GPU *decoding* is implemented
//! ([`HwMode`], used through `VideoDecoder::open_with`) with an automatic
//! fallback to software, but could only be exercised without a GPU in the
//! cloud; run `originfx hwcheck <file>` on a machine with a GPU to verify it.
//! GPU *encoding* (NVENC etc.) is not implemented.

use std::ffi::{c_int, CStr, CString};
use std::ptr;

use ffmpeg_next as ffmpeg;

use crate::error::{Error, Result};

/// Whether (and how) to use the GPU for decoding.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HwMode {
    /// Software only. The default, and always works.
    #[default]
    Off,
    /// Use the first working GPU backend that supports the clip's codec;
    /// silently fall back to software when none does.
    Auto,
    /// Require this backend (`cuda`, `vaapi`, `videotoolbox`, `d3d11va`, ...).
    /// Opening fails if it cannot be used.
    Backend(String),
}

impl HwMode {
    /// Parse `off`, `auto` or a backend name (as used by the command line).
    pub fn parse(text: &str) -> HwMode {
        match text.trim().to_ascii_lowercase().as_str() {
            "" | "off" | "none" | "software" | "cpu" => HwMode::Off,
            "auto" | "on" | "gpu" => HwMode::Auto,
            other => HwMode::Backend(other.to_string()),
        }
    }
}

/// Backends tried by [`HwMode::Auto`], best first. Intel QSV and Vulkan are
/// left out on purpose: they need extra setup and are not reliable through
/// the generic device API.
const AUTO_ORDER: [&str; 5] = ["cuda", "vaapi", "videotoolbox", "d3d11va", "dxva2"];

/// A GPU decode session attached to one decoder.
pub(crate) struct HwSession {
    pub name: String,
    /// Pixel format the decoder must pick; the `get_format` callback reads it
    /// through `AVCodecContext::opaque`, so the box must outlive the decoder.
    _wanted: Box<ffmpeg::ffi::AVPixelFormat>,
}

unsafe extern "C" fn pick_hw_format(
    ctx: *mut ffmpeg::ffi::AVCodecContext,
    formats: *const ffmpeg::ffi::AVPixelFormat,
) -> ffmpeg::ffi::AVPixelFormat {
    // SAFETY: `opaque` points at the boxed pixel format owned by `HwSession`, which
    // outlives the codec context; `formats` is a NONE-terminated array from FFmpeg.
    let wanted = *((*ctx).opaque as *const ffmpeg::ffi::AVPixelFormat);
    let mut p = formats;
    while *p != ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_NONE {
        if *p == wanted {
            return wanted;
        }
        p = p.add(1);
    }
    ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_NONE
}

/// Try to attach a GPU device to `ctx` (not yet opened) for `codec`.
///
/// Returns `Ok(None)` when `mode` is `Auto` and nothing suitable exists,
/// and an error when a specific backend was demanded but cannot be used.
pub(crate) fn attach(
    ctx: *mut ffmpeg::ffi::AVCodecContext,
    codec: *const ffmpeg::ffi::AVCodec,
    mode: &HwMode,
) -> Result<Option<HwSession>> {
    let names: Vec<String> = match mode {
        HwMode::Off => return Ok(None),
        HwMode::Auto => AUTO_ORDER.iter().map(|s| (*s).to_string()).collect(),
        HwMode::Backend(n) => vec![n.clone()],
    };
    let demanded = matches!(mode, HwMode::Backend(_));
    let mut last_problem = String::new();

    for name in names {
        let cname = CString::new(name.clone())
            .map_err(|_| Error::InvalidArgument(format!("bad backend name {name:?}")))?;
        // SAFETY: plain table lookups and buffer-reference calls; every pointer we
        // create is either handed to the codec context (which owns it) or released here.
        unsafe {
            let kind = ffmpeg::ffi::av_hwdevice_find_type_by_name(cname.as_ptr());
            if kind == ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
                if demanded {
                    return Err(Error::InvalidArgument(format!(
                        "unknown hardware backend '{name}'"
                    )));
                }
                continue;
            }
            // Which pixel format does this codec use with that device type?
            let mut wanted = None;
            let mut i = 0;
            loop {
                let cfg = ffmpeg::ffi::avcodec_get_hw_config(codec, i);
                if cfg.is_null() {
                    break;
                }
                if (*cfg).methods & ffmpeg::ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as c_int
                    != 0
                    && (*cfg).device_type == kind
                {
                    wanted = Some((*cfg).pix_fmt);
                    break;
                }
                i += 1;
            }
            let Some(wanted) = wanted else {
                last_problem = format!("this codec has no {name} decoder");
                continue;
            };
            let mut dev: *mut ffmpeg::ffi::AVBufferRef = ptr::null_mut();
            let ret = ffmpeg::ffi::av_hwdevice_ctx_create(
                &mut dev,
                kind,
                ptr::null(),
                ptr::null_mut(),
                0,
            );
            if ret < 0 {
                last_problem = format!("{name}: {}", error_text(ret));
                continue;
            }
            let wanted = Box::new(wanted);
            (*ctx).hw_device_ctx = ffmpeg::ffi::av_buffer_ref(dev);
            ffmpeg::ffi::av_buffer_unref(&mut dev);
            (*ctx).opaque = &*wanted as *const _ as *mut std::ffi::c_void;
            (*ctx).get_format = Some(pick_hw_format);
            return Ok(Some(HwSession {
                name,
                _wanted: wanted,
            }));
        }
    }
    if demanded {
        Err(Error::Unsupported(format!(
            "hardware decoding unavailable: {last_problem}"
        )))
    } else {
        Ok(None)
    }
}

/// Does this decoded frame live in GPU memory?
pub(crate) fn is_hw_frame(frame: &ffmpeg::frame::Video) -> bool {
    // SAFETY: reading a pointer field of a valid AVFrame.
    unsafe { !(*frame.as_ptr()).hw_frames_ctx.is_null() }
}

/// Copy a GPU frame into system memory (NV12, P010, ...), keeping its timestamps.
pub(crate) fn download(hw: &ffmpeg::frame::Video) -> Result<ffmpeg::frame::Video> {
    let mut sw = ffmpeg::frame::Video::empty();
    // SAFETY: both frames are valid; `sw` is empty so FFmpeg allocates its buffers.
    unsafe {
        let ret = ffmpeg::ffi::av_hwframe_transfer_data(sw.as_mut_ptr(), hw.as_ptr(), 0);
        if ret < 0 {
            return Err(ffmpeg::Error::from(ret).into());
        }
        let ret = ffmpeg::ffi::av_frame_copy_props(sw.as_mut_ptr(), hw.as_ptr());
        if ret < 0 {
            return Err(ffmpeg::Error::from(ret).into());
        }
    }
    Ok(sw)
}

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
            let ret = ffmpeg::ffi::av_hwdevice_ctx_create(
                &mut ctx,
                kind,
                ptr::null(),
                ptr::null_mut(),
                0,
            );
            let (usable, error) = if ret >= 0 {
                ffmpeg::ffi::av_buffer_unref(&mut ctx);
                (true, None)
            } else {
                (false, Some(error_text(ret)))
            };
            out.push(HwBackend {
                vendor: vendor_of(&name),
                name,
                usable,
                error,
            });
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
    const SUFFIXES: [&str; 6] = [
        "_nvenc",
        "_vaapi",
        "_qsv",
        "_videotoolbox",
        "_amf",
        "_cuvid",
    ];
    const CODECS: [&str; 5] = ["h264", "hevc", "av1", "vp9", "mpeg2video"];
    let mut found = Vec::new();
    for codec in CODECS {
        for suffix in SUFFIXES {
            let name = format!("{codec}{suffix}");
            if ffmpeg::encoder::find_by_name(&name).is_some()
                || ffmpeg::decoder::find_by_name(&name).is_some()
            {
                found.push(name);
            }
        }
    }
    found
}
