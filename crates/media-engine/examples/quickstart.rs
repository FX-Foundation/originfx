//! Probe a file, grab a frame, and export a 720p copy.
//!
//!     cargo run --release -p originfx-media-engine --example quickstart -- clip.mp4

use media_engine::{
    export_with_progress, probe, ExportPreset, ExportSettings, FrameReader, Quality,
};

fn main() -> media_engine::Result<()> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: quickstart <video file>");

    // 1. What is in the file? (headers only, so this is instant)
    let info = probe(&path)?;
    println!(
        "{} ({}, {:.1}s)",
        path,
        info.format_name,
        info.duration_secs.unwrap_or(0.0)
    );
    let video = info.primary_video().expect("file has no video");
    println!(
        "  video: {} {}x{}",
        video.codec,
        video.display_width(),
        video.display_height()
    );

    // 2. Random access: the frame on screen at the halfway point.
    let mut reader = FrameReader::open(&path, 256 * 1024 * 1024)?;
    let middle = info.duration_secs.unwrap_or(0.0) / 2.0;
    let frame = reader.frame_at(middle)?;
    println!(
        "  frame at {middle:.2}s: {}x{} RGBA, {} bytes",
        frame.width,
        frame.height,
        frame.data.len()
    );

    // 3. Export a 720p H.264 copy, with progress.
    let settings = ExportSettings::new(ExportPreset::Mp4H264)
        .quality(Quality::Standard)
        .height(720);
    let out = std::env::temp_dir().join("originfx-quickstart.mp4");
    let report = export_with_progress(&path, &out, &settings, |p| {
        if let Some(f) = p.fraction() {
            eprint!("\r  exporting {:>5.1}%", f * 100.0);
        }
        true // return false to cancel
    })?;
    eprintln!();
    println!(
        "  wrote {} ({} frames, {} bytes, {:.2}s)",
        out.display(),
        report.frames,
        report.output_bytes,
        report.elapsed.as_secs_f64()
    );
    Ok(())
}
