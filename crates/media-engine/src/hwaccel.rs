// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardware acceleration: discovery and GPU video decoding.
//!
//! [`probe_backends`] reports which FFmpeg hardware device types exist and
//! whether a device can actually be opened on this machine.
//!
//! GPU *decoding* is available through [`HwPolicy`] (see
//! [`DecodeOptions`](crate::DecodeOptions)): NVIDIA via CUDA/NVDEC and
//! Intel/AMD via VA-API. Each decoded frame is downloaded to system memory
//! (NV12, or P010 for 10-bit) and converted to RGBA by the same code as
//! software decoding, so callers see identical frames. GPU *encoding* is not
//! wired in yet.

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

// ---------------------------------------------------------------------------
// GPU decoding
// ---------------------------------------------------------------------------

/// A GPU decoding API the engine can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HwApi {
    /// NVIDIA NVDEC through CUDA.
    Cuda,
    /// VA-API (Intel and AMD on Linux).
    Vaapi,
}

impl HwApi {
    /// Every API, in the order [`HwPolicy::Auto`] tries them.
    pub const ALL: [HwApi; 2] = [HwApi::Cuda, HwApi::Vaapi];

    /// Short name, as FFmpeg and the CLI spell it: `cuda`, `vaapi`.
    pub fn name(self) -> &'static str {
        match self {
            HwApi::Cuda => "cuda",
            HwApi::Vaapi => "vaapi",
        }
    }

    /// True when a device for this API can be opened on this machine right now.
    pub fn is_usable(self) -> bool {
        crate::init();
        let mut ctx: *mut ffmpeg::ffi::AVBufferRef = ptr::null_mut();
        // SAFETY: on success `ctx` holds a new reference, released straight away.
        unsafe {
            let ret = ffmpeg::ffi::av_hwdevice_ctx_create(
                &mut ctx,
                self.device_type(),
                ptr::null(),
                ptr::null_mut(),
                0,
            );
            if ret >= 0 {
                ffmpeg::ffi::av_buffer_unref(&mut ctx);
            }
            ret >= 0
        }
    }

    /// Parse a name as returned by [`name`](Self::name).
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.name() == name)
    }

    fn device_type(self) -> ffmpeg::ffi::AVHWDeviceType {
        match self {
            HwApi::Cuda => ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
            HwApi::Vaapi => ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
        }
    }
}

impl std::fmt::Display for HwApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Whether, and how, to decode video on the GPU.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum HwPolicy {
    /// Software decoding only (the default; identical to earlier releases).
    #[default]
    Off,
    /// Try CUDA, then VA-API. If no device opens, the codec is not supported,
    /// or a frame fails to come back from the GPU, quietly continue in software.
    Auto,
    /// Use exactly this API. Opening fails if it cannot decode the stream, and
    /// a GPU failure while decoding is returned as an error instead of falling back.
    Require(HwApi),
}

/// A GPU decode setup ready to attach to a codec context.
pub(crate) struct HwSetup {
    /// Decoder implementation that supports this device (e.g. native `av1`, not `libdav1d`).
    pub codec: ffmpeg::Codec,
    /// Pixel format the decoder outputs when it decodes on the GPU.
    pub hw_format: ffmpeg::ffi::AVPixelFormat,
    /// Owned reference to the device; handed to the codec context by [`attach`](Self::attach).
    device: *mut ffmpeg::ffi::AVBufferRef,
}

impl HwSetup {
    /// Find a decoder for `codec_id` that can use `api`, then open the device.
    /// The codec check comes first so no device is created for a codec it cannot decode.
    pub fn new(api: HwApi, codec_id: ffmpeg::codec::Id) -> std::result::Result<Self, String> {
        let (codec, hw_format) = find_hw_decoder(codec_id, api.device_type())
            .ok_or_else(|| format!("{api} cannot decode {}", codec_id.name()))?;
        let mut device: *mut ffmpeg::ffi::AVBufferRef = ptr::null_mut();
        // SAFETY: on success `av_hwdevice_ctx_create` stores a new reference in
        // `device`, which `HwSetup` owns until `attach` or `Drop`.
        let ret = unsafe {
            ffmpeg::ffi::av_hwdevice_ctx_create(
                &mut device,
                api.device_type(),
                ptr::null(),
                ptr::null_mut(),
                0,
            )
        };
        if ret < 0 {
            return Err(format!("{api} device: {}", error_text(ret)));
        }
        Ok(Self {
            codec,
            hw_format,
            device,
        })
    }

    /// Give the device to a codec context that has not been opened yet and
    /// install the callback that picks the GPU output format.
    pub fn attach(mut self, ctx: &mut ffmpeg::codec::context::Context) {
        // SAFETY: `ctx` is a valid, not yet opened codec context. Ownership of our
        // device reference moves into `hw_device_ctx`; `avcodec_free_context`
        // releases it. We null our pointer so `Drop` does not release it twice.
        unsafe {
            let raw = ctx.as_mut_ptr();
            (*raw).hw_device_ctx = self.device;
            (*raw).get_format = Some(pick_hw_format);
        }
        self.device = ptr::null_mut();
    }
}

impl Drop for HwSetup {
    fn drop(&mut self) {
        if !self.device.is_null() {
            // SAFETY: we still own this reference (it was never attached).
            unsafe { ffmpeg::ffi::av_buffer_unref(&mut self.device) };
        }
    }
}

/// Hardware pixel format of a `HW_DEVICE_CTX` config for `device` on `codec`, if any.
///
/// # Safety
/// `codec` must point to a valid, registered `AVCodec`.
unsafe fn hw_config_format(
    codec: *const ffmpeg::ffi::AVCodec,
    device: ffmpeg::ffi::AVHWDeviceType,
) -> Option<ffmpeg::ffi::AVPixelFormat> {
    let method = ffmpeg::ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32;
    for i in 0.. {
        // SAFETY: the caller guarantees `codec` is valid; FFmpeg returns null past the last config.
        let config = unsafe { ffmpeg::ffi::avcodec_get_hw_config(codec, i) };
        if config.is_null() {
            return None;
        }
        // SAFETY: non-null configs point into the codec's static table.
        let config = unsafe { &*config };
        if config.device_type == device && config.methods & method != 0 {
            return Some(config.pix_fmt);
        }
    }
    None
}

/// First registered decoder for `id` that has a GPU config for `device`.
fn find_hw_decoder(
    id: ffmpeg::codec::Id,
    device: ffmpeg::ffi::AVHWDeviceType,
) -> Option<(ffmpeg::Codec, ffmpeg::ffi::AVPixelFormat)> {
    let want: ffmpeg::ffi::AVCodecID = id.into();
    let mut opaque: *mut std::ffi::c_void = ptr::null_mut();
    loop {
        // SAFETY: `av_codec_iterate` walks FFmpeg's static codec list; the
        // returned codecs live for the whole program.
        unsafe {
            let codec = ffmpeg::ffi::av_codec_iterate(&mut opaque);
            if codec.is_null() {
                return None;
            }
            if (*codec).id != want || ffmpeg::ffi::av_codec_is_decoder(codec) == 0 {
                continue;
            }
            if let Some(fmt) = hw_config_format(codec, device) {
                return Some((ffmpeg::Codec::wrap(codec as *mut _), fmt));
            }
        }
    }
}

/// `get_format` callback: choose the GPU format when the decoder offers it,
/// otherwise FFmpeg's default (software) choice. A software pick is noticed by
/// the decoder, which then reopens itself as a plain software decoder.
unsafe extern "C" fn pick_hw_format(
    ctx: *mut ffmpeg::ffi::AVCodecContext,
    formats: *const ffmpeg::ffi::AVPixelFormat,
) -> ffmpeg::ffi::AVPixelFormat {
    // SAFETY: FFmpeg calls this with a valid context whose `hw_device_ctx` we set
    // (a buffer holding an `AVHWDeviceContext`) and a list ending in AV_PIX_FMT_NONE.
    unsafe {
        let device = (*ctx).hw_device_ctx;
        if !device.is_null() {
            let kind = (*((*device).data as *const ffmpeg::ffi::AVHWDeviceContext)).type_;
            if let Some(want) = hw_config_format((*ctx).codec, kind) {
                let mut p = formats;
                while *p != ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_NONE {
                    if *p == want {
                        return want;
                    }
                    p = p.add(1);
                }
            }
        }
        ffmpeg::ffi::avcodec_default_get_format(ctx, formats)
    }
}

/// Copy a GPU frame into system memory, keeping its timestamps and colour tags.
///
/// GPUs hand back NV12 (or P010 for 10-bit), where the two chroma planes are
/// interleaved. The decoded values are identical to software decoding, but
/// swscale converts 8-bit NV12 to RGB along a different path than the planar
/// `yuv420p` a software decoder produces, so sharp colour edges come out
/// differently. Repacking NV12 to planar (a lossless copy) makes GPU and
/// software frames pixel-identical. P010 already converts identically to
/// `yuv420p10le`, so it is not repacked.
///
/// `bufs` keeps the system-memory frames between calls, so a 4K stream does
/// not allocate (and page in) fresh buffers for every frame. A buffer is only
/// reused once nothing holds the frame handed out from it any more.
pub(crate) fn download(
    hw: &ffmpeg::frame::Video,
    bufs: &mut DownloadBuffers,
) -> std::result::Result<ffmpeg::frame::Video, String> {
    let size = (hw.width(), hw.height());
    if !reusable(&bufs.staging, None, size) {
        bufs.staging = ffmpeg::frame::Video::empty();
    }
    // SAFETY: both frames are valid and nobody else references `staging`'s
    // buffers. When it is empty FFmpeg allocates it in the device's download
    // format; otherwise it is overwritten in place (same size and format).
    let ret =
        unsafe { ffmpeg::ffi::av_hwframe_transfer_data(bufs.staging.as_mut_ptr(), hw.as_ptr(), 0) };
    if ret < 0 {
        bufs.staging = ffmpeg::frame::Video::empty();
        return Err(format!("GPU frame download failed: {}", error_text(ret)));
    }
    let source = if bufs.staging.format() == ffmpeg::format::Pixel::NV12 {
        let planar = ffmpeg::format::Pixel::YUV420P;
        if !reusable(&bufs.planar, Some(planar), size) {
            bufs.planar = ffmpeg::frame::Video::new(planar, size.0, size.1);
        }
        planar_from_nv12(&bufs.staging, &mut bufs.planar);
        &bufs.planar
    } else {
        // 10-bit P010 converts to RGBA exactly like planar yuv420p10le: hand it on as is.
        &bufs.staging
    };
    // SAFETY: `source` is a valid frame. `av_frame_clone` makes a new frame that
    // shares its buffers (no pixel copy); null means out of memory.
    let shared = unsafe { ffmpeg::ffi::av_frame_clone(source.as_ptr()) };
    if shared.is_null() {
        return Err("GPU frame download failed: out of memory".to_string());
    }
    // SAFETY: `shared` is a new, owned frame.
    let mut out = unsafe { ffmpeg::frame::Video::wrap(shared) };
    // SAFETY: both frames are valid; copies pts, colour and other metadata, not pixels.
    let ret = unsafe { ffmpeg::ffi::av_frame_copy_props(out.as_mut_ptr(), hw.as_ptr()) };
    if ret < 0 {
        return Err(format!("GPU frame download failed: {}", error_text(ret)));
    }
    Ok(out)
}

/// System-memory frames reused by [`download`].
pub(crate) struct DownloadBuffers {
    staging: ffmpeg::frame::Video,
    planar: ffmpeg::frame::Video,
}

impl Default for DownloadBuffers {
    fn default() -> Self {
        Self {
            staging: ffmpeg::frame::Video::empty(),
            planar: ffmpeg::frame::Video::empty(),
        }
    }
}

/// Can `f` be written again for a frame of `size` (and `format`, if given)?
/// False when it is empty or a frame handed out earlier still shares its buffers.
fn reusable(
    f: &ffmpeg::frame::Video,
    format: Option<ffmpeg::format::Pixel>,
    size: (u32, u32),
) -> bool {
    // SAFETY: `f` is a valid frame; `av_frame_is_writable` only inspects its
    // buffer reference counts (it takes `*mut` but does not modify the frame).
    let writable = unsafe { ffmpeg::ffi::av_frame_is_writable(f.as_ptr() as *mut _) != 0 };
    writable && (f.width(), f.height()) == size && format.is_none_or(|p| f.format() == p)
}

/// Rows of plane `p` of `f`, each trimmed to `len` bytes.
fn rows(f: &ffmpeg::frame::Video, p: usize, len: usize) -> impl Iterator<Item = &[u8]> {
    let stride = f.stride(p);
    f.data(p).chunks(stride).map(move |r| &r[..len])
}

/// Like [`rows`], writable.
fn rows_mut(f: &mut ffmpeg::frame::Video, p: usize, len: usize) -> impl Iterator<Item = &mut [u8]> {
    let stride = f.stride(p);
    f.data_mut(p).chunks_mut(stride).map(move |r| &mut r[..len])
}

/// NV12 -> yuv420p (into `dst`, same size): copy luma, split the interleaved UV plane.
fn planar_from_nv12(src: &ffmpeg::frame::Video, dst: &mut ffmpeg::frame::Video) {
    let (w, cw) = (src.width() as usize, (src.width() as usize).div_ceil(2));
    for (d, s) in rows_mut(dst, 0, w).zip(rows(src, 0, w)) {
        d.copy_from_slice(s);
    }
    for (plane, k) in [(1, 0), (2, 1)] {
        for (d, s) in rows_mut(dst, plane, cw).zip(rows(src, 1, cw * 2)) {
            for (d, uv) in d.iter_mut().zip(s.as_chunks::<2>().0) {
                *d = uv[k];
            }
        }
    }
}
