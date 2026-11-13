// SPDX-License-Identifier: GPL-3.0-or-later
//! Small helpers: timecode, byte sizes, image conversion, relative dates.

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

pub fn frame_size(resolution: i32, aspect: i32) -> (u32, u32) {
    let short: u32 = match resolution {
        0 => 720,
        2 => 2160,
        _ => 1080,
    };
    match aspect {
        1 => (short, short * 16 / 9),
        2 => (short, short),
        3 => (short * 16 / 9, (short as f64 * 16.0 / 9.0 / 2.39).round() as u32 / 2 * 2),
        _ => (short * 16 / 9, short),
    }
}

pub fn aspect_name(w: u32, h: u32) -> &'static str {
    if h == 0 {
        return "16:9";
    }
    let r = w as f64 / h as f64;
    if (r - 16.0 / 9.0).abs() < 0.02 {
        "16:9"
    } else if (r - 9.0 / 16.0).abs() < 0.02 {
        "9:16"
    } else if (r - 1.0).abs() < 0.02 {
        "1:1"
    } else if r > 2.2 {
        "21:9"
    } else if (r - 4.0 / 3.0).abs() < 0.02 {
        "4:3"
    } else {
        "custom"
    }
}

pub fn aspect_index(w: u32, h: u32) -> i32 {
    match aspect_name(w, h) {
        "9:16" => 1,
        "1:1" => 2,
        "21:9" => 3,
        _ => 0,
    }
}

/// hh:mm:ss:ff at the given rate (or hh:mm:ss.mmm when `seconds` is set).
pub fn timecode(secs: f64, fps: f64, seconds: bool) -> String {
    let secs = secs.max(0.0);
    if seconds {
        let total_ms = (secs * 1000.0).round() as i64;
        return format!(
            "{:02}:{:02}:{:02}.{:03}",
            total_ms / 3_600_000,
            total_ms / 60_000 % 60,
            total_ms / 1000 % 60,
            total_ms % 1000
        );
    }
    let f = fps.round().max(1.0) as i64;
    let total = (secs * fps).round() as i64;
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        total / f / 3600,
        total / f / 60 % 60,
        total / f % 60,
        total % f
    )
}

/// m:ss or h:mm:ss, for lists.
pub fn short_duration(secs: f64) -> String {
    let s = secs.round().max(0.0) as i64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub fn bytes(n: u64) -> String {
    let f = n as f64;
    if f >= 1e12 {
        format!("{:.1} TB", f / 1e12)
    } else if f >= 1e9 {
        format!("{:.1} GB", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.0} MB", f / 1e6)
    } else if f >= 1e3 {
        format!("{:.0} KB", f / 1e3)
    } else {
        format!("{n} B")
    }
}

pub fn relative_date(unix: i64) -> String {
    let now = chrono::Local::now().timestamp();
    let d = (now - unix).max(0);
    if d < 60 * 60 * 24 {
        "Edited today".into()
    } else if d < 2 * 60 * 60 * 24 {
        "Edited yesterday".into()
    } else if d < 7 * 60 * 60 * 24 {
        format!("Edited {} days ago", d / (60 * 60 * 24))
    } else if d < 14 * 60 * 60 * 24 {
        "Edited last week".into()
    } else if d < 60 * 60 * 60 * 24 {
        format!("Edited {} weeks ago", d / (7 * 60 * 60 * 24))
    } else {
        format!("Edited {} months ago", d / (30 * 60 * 60 * 24))
    }
}

pub fn clock(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

pub fn date_time(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%d %b %Y, %H:%M").to_string())
        .unwrap_or_default()
}

/// RGBA bytes -> slint image, optionally downscaled to at most `max_w` wide (nearest neighbour).
pub fn rgba_image(w: u32, h: u32, data: &[u8], max_w: u32) -> Image {
    if w == 0 || h == 0 || data.len() < (w * h * 4) as usize {
        return Image::default();
    }
    if w <= max_w {
        let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
        buf.make_mut_bytes().copy_from_slice(&data[..(w * h * 4) as usize]);
        return Image::from_rgba8(buf);
    }
    let scale = max_w as f64 / w as f64;
    let nw = max_w;
    let nh = ((h as f64 * scale).round() as u32).max(1);
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(nw, nh);
    let out = buf.make_mut_bytes();
    for y in 0..nh {
        let sy = ((y as f64 / scale) as u32).min(h - 1);
        for x in 0..nw {
            let sx = ((x as f64 / scale) as u32).min(w - 1);
            let si = ((sy * w + sx) * 4) as usize;
            let di = ((y * nw + x) * 4) as usize;
            out[di..di + 4].copy_from_slice(&data[si..si + 4]);
        }
    }
    Image::from_rgba8(buf)
}

/// Box-filter downscale to exactly (nw, nh), returned as RGBA bytes.
pub fn resize_rgba(w: u32, h: u32, data: &[u8], nw: u32, nh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    if w == 0 || h == 0 || nw == 0 || nh == 0 {
        return out;
    }
    for y in 0..nh {
        let y0 = (y as u64 * h as u64 / nh as u64) as u32;
        let y1 = (((y + 1) as u64 * h as u64 / nh as u64) as u32).max(y0 + 1).min(h);
        for x in 0..nw {
            let x0 = (x as u64 * w as u64 / nw as u64) as u32;
            let x1 = (((x + 1) as u64 * w as u64 / nw as u64) as u32).max(x0 + 1).min(w);
            let mut acc = [0u64; 4];
            let mut n = 0u64;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = ((sy * w + sx) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += data[i + c] as u64;
                    }
                    n += 1;
                }
            }
            let o = ((y * nw + x) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    out
}

pub fn file_kind(path: &std::path::Path) -> i32 {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" | "mts" | "m2ts" | "mxf" | "ts" | "wmv" | "flv" | "3gp"
        | "mpg" | "mpeg" | "ogv" | "vob" | "dv" | "gif" => 0,
        "wav" | "mp3" | "aac" | "m4a" | "flac" | "ogg" | "opus" | "aif" | "aiff" | "wma" | "ac3" => 1,
        "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" | "webp" | "exr" | "dpx" | "tga" | "heic" => 2,
        "originfx" => 5,
        _ => 4,
    }
}

pub fn is_media(path: &std::path::Path) -> bool {
    matches!(file_kind(path), 0 | 1 | 2)
}

pub fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(path) {
        for e in rd.flatten() {
            if let Ok(md) = e.metadata() {
                if md.is_dir() {
                    total += dir_size(&e.path());
                } else {
                    total += md.len();
                }
            }
        }
    }
    total
}

pub fn free_space(path: &std::path::Path) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    disks
        .list()
        .iter()
        .filter(|d| target.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}
