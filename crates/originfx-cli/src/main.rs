//! `originfx`: command line front-end for the OriginFX engines.

mod png;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand, ValueEnum};
use media_engine::{
    hwaccel, probe, DecodeOptions, Error, ExportPreset, ExportSettings, FrameReader, HwApi,
    HwPolicy, MediaInfo, ProxyManager, ProxySettings, Quality, VideoDecoder,
};

#[derive(Parser)]
#[command(
    name = "originfx",
    version,
    about = "OriginFX engines on the command line"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show what is inside a media file.
    Probe {
        file: PathBuf,
        /// Print machine-readable JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Convert a file using an export preset (guessed from the output extension if omitted).
    Convert {
        input: PathBuf,
        output: PathBuf,
        /// Preset id, see `originfx presets`.
        #[arg(short, long)]
        preset: Option<String>,
        #[arg(short, long, value_enum, default_value = "standard")]
        quality: QualityArg,
        /// Scale down to this height (keeps aspect ratio, never upscales).
        #[arg(long)]
        height: Option<u32>,
        /// Output frame rate.
        #[arg(long)]
        fps: Option<f64>,
        /// Video bitrate in kbit/s (overrides quality).
        #[arg(long)]
        video_kbps: Option<u64>,
        /// Audio bitrate in kbit/s.
        #[arg(long)]
        audio_kbps: Option<u64>,
        /// Keep only the clip from this time (seconds).
        #[arg(long)]
        start: Option<f64>,
        /// Keep only the clip up to this time (seconds).
        #[arg(long)]
        end: Option<f64>,
        /// Use this video stream (index from `probe`).
        #[arg(long)]
        video_stream: Option<usize>,
        /// Use this audio track (index from `probe`).
        #[arg(long)]
        audio_stream: Option<usize>,
        /// GPU video encoding (NVENC, H.264/H.265 presets): off, auto (NVENC, else
        /// software) or cuda (NVENC or an error). vaapi is not supported yet.
        #[arg(long, value_enum, default_value = "off")]
        hw: HwArg,
    },
    /// List export presets and whether this machine's FFmpeg can produce them.
    Presets,
    /// Save one frame as a PNG.
    Frame {
        file: PathBuf,
        /// Time in seconds.
        #[arg(long, default_value_t = 0.0)]
        at: f64,
        #[arg(short, long, default_value = "frame.png")]
        out: PathBuf,
        /// GPU decoding: off, auto (try CUDA then VA-API, else software), cuda or vaapi.
        #[arg(long, value_enum, default_value = "off")]
        hw: HwArg,
    },
    /// Create (or find) the low-resolution proxy for a file.
    Proxy {
        file: PathBuf,
        /// Directory that holds proxies.
        #[arg(long, default_value = "proxies")]
        dir: PathBuf,
        #[arg(long, default_value_t = 540)]
        height: u32,
    },
    /// Report which GPU acceleration APIs this machine exposes. Paste the output when reporting GPU issues.
    Hw,
    /// Measure decode speed.
    Bench {
        file: PathBuf,
        /// Stop after this many frames.
        #[arg(long, default_value_t = 600)]
        frames: u64,
        /// Also test random access: seek this many times.
        #[arg(long, default_value_t = 20)]
        seeks: u32,
        /// GPU decoding: off, auto (try CUDA then VA-API, else software), cuda or vaapi.
        #[arg(long, value_enum, default_value = "off")]
        hw: HwArg,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum QualityArg {
    Draft,
    Standard,
    High,
}

#[derive(Clone, Copy, ValueEnum)]
enum HwArg {
    Off,
    Auto,
    Cuda,
    Vaapi,
}

impl From<HwArg> for HwPolicy {
    fn from(hw: HwArg) -> Self {
        match hw {
            HwArg::Off => HwPolicy::Off,
            HwArg::Auto => HwPolicy::Auto,
            HwArg::Cuda => HwPolicy::Require(HwApi::Cuda),
            HwArg::Vaapi => HwPolicy::Require(HwApi::Vaapi),
        }
    }
}

impl From<HwArg> for DecodeOptions {
    fn from(hw: HwArg) -> Self {
        DecodeOptions { hw: hw.into() }
    }
}

/// "cuda", or "software" plus the reason the GPU was not used when it was asked for.
fn backend_label(dec: &VideoDecoder) -> String {
    match (dec.hw_backend(), dec.hw_fallback_reason()) {
        (Some(api), _) => api.to_string(),
        (None, Some(why)) => format!("software (GPU not used: {why})"),
        (None, None) => "software".to_string(),
    }
}

impl From<QualityArg> for Quality {
    fn from(q: QualityArg) -> Self {
        match q {
            QualityArg::Draft => Quality::Draft,
            QualityArg::Standard => Quality::Standard,
            QualityArg::High => Quality::High,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

type CliResult = Result<(), Box<dyn std::error::Error>>;

fn run(cmd: Command) -> CliResult {
    match cmd {
        Command::Probe { file, json } => {
            let info = probe(&file)?;
            if json {
                println!("{}", info_json(&info));
            } else {
                print_info(&info);
            }
        }
        Command::Convert {
            input,
            output,
            preset,
            quality,
            height,
            fps,
            video_kbps,
            audio_kbps,
            start,
            end,
            video_stream,
            audio_stream,
            hw,
        } => {
            let preset = match preset {
                Some(id) => ExportPreset::from_id(&id)
                    .ok_or_else(|| format!("unknown preset '{id}', try `originfx presets`"))?,
                None => preset_from_extension(&output)?,
            };
            let mut s = ExportSettings::new(preset).quality(quality.into());
            s.height = height;
            s.fps = fps;
            s.video_bitrate = video_kbps.map(|k| k * 1000);
            s.audio_bitrate = audio_kbps.map(|k| k * 1000);
            s.start_secs = start;
            s.end_secs = end;
            s.video_stream = video_stream;
            s.audio_stream = audio_stream;
            s.hw_encode = hw.into();

            let tty = std::io::stderr().is_terminal();
            let report = media_engine::export_with_progress(&input, &output, &s, |p| {
                if tty {
                    match p.fraction() {
                        Some(f) => eprint!(
                            "\r{:>5.1}%  {:>7.1}s  {} frames   ",
                            f * 100.0,
                            p.position_secs,
                            p.frames
                        ),
                        None => eprint!("\r{:>7.1}s  {} frames   ", p.position_secs, p.frames),
                    }
                    let _ = std::io::stderr().flush();
                }
                true
            })?;
            if tty {
                eprintln!();
            }
            println!(
                "{} -> {}  [{}]  {} frames, {:.1}s of media, {} in {:.2}s",
                input.display(),
                output.display(),
                preset.label(),
                report.frames,
                report.duration_secs,
                human_bytes(report.output_bytes),
                report.elapsed.as_secs_f64()
            );
            if let Some(enc) = &report.video_encoder {
                match &report.hw_fallback_reason {
                    Some(why) => println!("video encoder: {enc} (GPU not used: {why})"),
                    None => println!("video encoder: {enc}"),
                }
            }
        }
        Command::Presets => {
            println!("{:<18} {:<5} {:<30} available", "id", "ext", "description");
            for p in ExportPreset::ALL {
                println!(
                    "{:<18} {:<5} {:<30} {}",
                    p.id(),
                    p.extension(),
                    p.label(),
                    if p.is_available() {
                        "yes"
                    } else {
                        "no (encoder missing)"
                    }
                );
            }
        }
        Command::Frame { file, at, out, hw } => {
            let mut reader = FrameReader::open_with(&file, 256 * 1024 * 1024, hw.into())?;
            let frame = reader.frame_at(at)?;
            png::write_rgba(&out, frame.width, frame.height, &frame.data)?;
            println!(
                "saved {}x{} frame at {:.3}s to {}  [decode: {}]",
                frame.width,
                frame.height,
                frame.pts_secs,
                out.display(),
                backend_label(reader.decoder())
            );
        }
        Command::Proxy { file, dir, height } => {
            let pm = ProxyManager::new(dir).with_settings(ProxySettings {
                height,
                ..ProxySettings::default()
            });
            let existed = pm.existing(&file).is_some();
            let path = pm.ensure(&file)?;
            println!(
                "{} proxy: {}",
                if existed { "existing" } else { "created" },
                path.display()
            );
        }
        Command::Hw => print_hw(),
        Command::Bench {
            file,
            frames,
            seeks,
            hw,
        } => bench(&file, frames, seeks, hw.into())?,
    }
    Ok(())
}

fn preset_from_extension(path: &Path) -> Result<ExportPreset, Error> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    Ok(match ext.as_str() {
        "mp4" | "m4v" => ExportPreset::Mp4H264,
        "mov" => ExportPreset::MovH264,
        "mkv" => ExportPreset::MkvH264,
        "webm" => ExportPreset::WebmVp9,
        "avi" => ExportPreset::AviMjpeg,
        "gif" => ExportPreset::Gif,
        "mp3" => ExportPreset::Mp3,
        "m4a" => ExportPreset::AacM4a,
        "wav" => ExportPreset::Wav,
        "flac" => ExportPreset::Flac,
        "ogg" | "opus" => ExportPreset::OpusOgg,
        _ => {
            return Err(Error::InvalidArgument(format!(
                "cannot guess a preset from extension '.{ext}'; pass --preset"
            )))
        }
    })
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

fn fmt_duration(secs: f64) -> String {
    let total = secs.round() as u64;
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total / 60) % 60,
        total % 60
    )
}

fn print_info(info: &MediaInfo) {
    println!("{}", info.path.display());
    println!(
        "  container : {} ({})",
        info.format_name, info.format_long_name
    );
    println!("  size      : {}", human_bytes(info.size_bytes));
    if let Some(d) = info.duration_secs {
        println!("  duration  : {} ({d:.3}s)", fmt_duration(d));
    }
    if let Some(b) = info.bit_rate {
        println!("  bit rate  : {} kbit/s", b / 1000);
    }
    for v in &info.video {
        let fps = v
            .frame_rate
            .map_or("?".to_string(), |r| format!("{:.3}", r.as_f64()));
        println!(
            "  video #{:<2} : {} {}x{} {} fps, {}{}{}",
            v.index,
            v.codec,
            v.width,
            v.height,
            fps,
            v.pixel_format,
            v.frames.map_or(String::new(), |n| format!(", {n} frames")),
            if v.rotation != 0 {
                format!(
                    ", rotated {}° (shows as {}x{})",
                    v.rotation,
                    v.display_width(),
                    v.display_height()
                )
            } else {
                String::new()
            }
        );
        let c = &v.colour;
        if c.space.is_some() || c.transfer.is_some() {
            println!(
                "              colour: {} / {} / {} / {} range{}",
                c.space.as_deref().unwrap_or("?"),
                c.primaries.as_deref().unwrap_or("?"),
                c.transfer.as_deref().unwrap_or("?"),
                c.range.as_deref().unwrap_or("?"),
                if c.is_hdr() { "  (HDR)" } else { "" }
            );
        }
    }
    for a in &info.audio {
        println!(
            "  audio #{:<2} : {} {} Hz, {} ch{}",
            a.index,
            a.codec,
            a.sample_rate,
            a.channels,
            a.language
                .as_deref()
                .map_or(String::new(), |l| format!(", {l}"))
        );
    }
    for s in &info.subtitles {
        println!(
            "  subs  #{:<2} : {}{}",
            s.index,
            s.codec,
            s.language
                .as_deref()
                .map_or(String::new(), |l| format!(", {l}"))
        );
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map_or("null".to_string(), |v| v.to_string())
}

fn info_json(i: &MediaInfo) -> String {
    let video: Vec<String> = i
        .video
        .iter()
        .map(|v| {
            format!(
                "{{\"index\":{},\"codec\":{},\"width\":{},\"height\":{},\"pixel_format\":{},\"frame_rate\":{},\"frames\":{},\"duration_secs\":{},\"bit_rate\":{},\"rotation\":{}}}",
                v.index,
                json_str(&v.codec),
                v.width,
                v.height,
                json_str(&v.pixel_format),
                opt(v.frame_rate.map(|r| r.as_f64())),
                opt(v.frames),
                opt(v.duration_secs),
                opt(v.bit_rate),
                v.rotation
            )
        })
        .collect();
    let audio: Vec<String> = i
        .audio
        .iter()
        .map(|a| {
            format!(
                "{{\"index\":{},\"codec\":{},\"sample_rate\":{},\"channels\":{},\"sample_format\":{},\"duration_secs\":{},\"bit_rate\":{},\"language\":{}}}",
                a.index,
                json_str(&a.codec),
                a.sample_rate,
                a.channels,
                json_str(&a.sample_format),
                opt(a.duration_secs),
                opt(a.bit_rate),
                a.language.as_deref().map_or("null".to_string(), json_str)
            )
        })
        .collect();
    let subs: Vec<String> = i
        .subtitles
        .iter()
        .map(|s| {
            format!(
                "{{\"index\":{},\"codec\":{},\"language\":{}}}",
                s.index,
                json_str(&s.codec),
                s.language.as_deref().map_or("null".to_string(), json_str)
            )
        })
        .collect();
    let meta: Vec<String> = i
        .metadata
        .iter()
        .map(|(k, v)| format!("{}:{}", json_str(k), json_str(v)))
        .collect();
    format!(
        "{{\"path\":{},\"format\":{},\"format_long\":{},\"size_bytes\":{},\"duration_secs\":{},\"bit_rate\":{},\"video\":[{}],\"audio\":[{}],\"subtitles\":[{}],\"metadata\":{{{}}}}}",
        json_str(&i.path.to_string_lossy()),
        json_str(&i.format_name),
        json_str(&i.format_long_name),
        i.size_bytes,
        opt(i.duration_secs),
        opt(i.bit_rate),
        video.join(","),
        audio.join(","),
        subs.join(","),
        meta.join(",")
    )
}

fn print_hw() {
    println!("OriginFX hardware acceleration report");
    println!("os: {} / {}", std::env::consts::OS, std::env::consts::ARCH);
    let backends = hwaccel::probe_backends();
    if backends.is_empty() {
        println!("no hardware backends compiled into this FFmpeg");
    }
    for b in &backends {
        match &b.error {
            None => println!("  [usable]  {:<14} {}", b.name, b.vendor),
            Some(e) => println!("  [no]      {:<14} {} -- {e}", b.name, b.vendor),
        }
    }
    let codecs = hwaccel::hardware_codecs();
    println!(
        "hardware codecs in this FFmpeg: {}",
        if codecs.is_empty() {
            "none".to_string()
        } else {
            codecs.join(", ")
        }
    );
    println!(
        "note: GPU decoding (cuda, vaapi) is used with --hw on `frame` and `bench`; NVENC encoding with --hw on `convert`."
    );
}

fn bench(file: &Path, max_frames: u64, seeks: u32, options: DecodeOptions) -> CliResult {
    let info = probe(file)?;
    let v = info.primary_video().ok_or(Error::NoStream("video"))?;
    println!("{}  {}x{} {}", file.display(), v.width, v.height, v.codec);

    let mut dec = VideoDecoder::open_with(file, options)?;
    let start = Instant::now();
    let mut n = 0u64;
    while n < max_frames {
        if dec.next_frame()?.is_none() {
            break;
        }
        n += 1;
    }
    let secs = start.elapsed().as_secs_f64().max(1e-9);
    println!("decode backend: {}", backend_label(&dec));
    println!(
        "sequential decode (incl. RGBA convert): {n} frames in {secs:.2}s = {:.1} fps",
        n as f64 / secs
    );

    if seeks > 0 {
        let dur = info.duration_secs.unwrap_or(0.0);
        if dur > 0.0 {
            let mut reader = FrameReader::open_with(file, 64 * 1024 * 1024, options)?;
            let start = Instant::now();
            for i in 0..seeks {
                let t = dur * (f64::from((i * 7919) % seeks) / f64::from(seeks));
                reader.frame_at(t)?;
            }
            let secs = start.elapsed().as_secs_f64();
            println!(
                "random access: {seeks} seeks in {secs:.2}s = {:.1} ms per frame  [decode: {}]",
                secs * 1000.0 / f64::from(seeks),
                backend_label(reader.decoder())
            );
        }
    }
    Ok(())
}
