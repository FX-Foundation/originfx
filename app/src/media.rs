// SPDX-License-Identifier: GPL-3.0-or-later
//! Section B: import, media inspector, proxies and cache, export, interchange.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use slint::{ComponentHandle, Image, Model as _, SharedString};

use media_engine as me;
use render_engine as re;
use timeline_engine as tl;
use tl::{AssetId, Time, TimeRange};

use crate::session::{Peaks, Session};
use crate::settings::{expand_home, shorten_home, AppSettings};
use crate::util;
use crate::{later, strings, vec_model, with, App, Backend, ExportJob, FileItem, Model, Row4, Screen, State, KV};

#[derive(Default)]
pub struct Jobs {
    pub thumbs_pending: HashSet<PathBuf>,
    pub file_thumbs: HashMap<PathBuf, Image>,
    pub peaks_pending: HashSet<AssetId>,
    pub proxies: HashMap<PathBuf, ProxyState>,
    pub proxy_running: bool,
    pub export_cancel: Option<Arc<AtomicBool>>,
    pub export_queue: Vec<QueuedExport>,
    pub export_log: Vec<ExportJob>,
    pub browse_dir: PathBuf,
    pub browse_entries: Vec<(PathBuf, i32, u64)>,
    pub browse_selected: HashSet<PathBuf>,
    pub browse_filter: String,
    pub inspector_frame_pending: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ProxyState {
    Queued,
    Working(f32),
    Ready,
    Failed,
}

#[derive(Clone)]
pub struct QueuedExport {
    pub name: String,
    pub output: PathBuf,
    pub settings: me::ExportSettings,
    pub range: Option<TimeRange>,
    pub project: tl::Project,
    pub seq: tl::SequenceId,
}

pub fn poll_jobs(_st: &mut State, _ui: &App) {}

// ---------------------------------------------------------------- session media

/// Start waveform and thumbnail work for every asset of the open project.
pub fn scan_session_media(st: &mut State) {
    let Some(s) = &st.session else { return };
    let assets: Vec<tl::Asset> = s.project().assets().values().cloned().collect();
    for a in assets {
        request_asset_media(st, &a);
    }
}

pub fn request_asset_media(st: &mut State, a: &tl::Asset) {
    if a.is_generated() || !a.path.exists() {
        return;
    }
    let id = a.id;
    if a.has_audio() && !st.jobs.peaks_pending.contains(&id) && st.session.as_ref().is_some_and(|s| !s.peaks.contains_key(&id)) {
        st.jobs.peaks_pending.insert(id);
        let path = a.path.clone();
        thread::spawn(move || {
            let peaks = compute_peaks(&path);
            later(move |st, ui| {
                st.jobs.peaks_pending.remove(&id);
                if let (Some(p), Some(s)) = (peaks, st.session.as_mut()) {
                    s.peaks.insert(id, Arc::new(p));
                    s.waveforms.clear();
                    st.sync(ui);
                }
            });
        });
    }
    if a.has_video() && st.session.as_ref().is_some_and(|s| !s.thumbs.contains_key(&id)) {
        let path = a.path.clone();
        let at = a.duration.map(|d| (d.as_secs_f64() * 0.2).min(2.0)).unwrap_or(0.0);
        thread::spawn(move || {
            if let Some((w, h, px)) = thumbnail(&path, at, 240) {
                later(move |st, ui| {
                    if let Some(s) = st.session.as_mut() {
                        s.thumbs.insert(id, util::rgba_image(w, h, &px, 240));
                        st.sync(ui);
                    }
                });
            }
        });
    }
}

pub fn compute_peaks(path: &Path) -> Option<Peaks> {
    let opts = me::AudioOptions { sample_rate: Some(8000), channels: Some(1) };
    let mut dec = me::AudioDecoder::open(path, opts).ok()?;
    let per_bucket = 80usize; // 100 buckets per second at 8 kHz
    let mut data = Vec::new();
    let mut carry: Vec<f32> = Vec::new();
    while let Ok(Some(buf)) = dec.next_buffer() {
        carry.extend_from_slice(&buf.samples);
        let full = carry.len() / per_bucket * per_bucket;
        for chunk in carry[..full].chunks(per_bucket) {
            let (mut lo, mut hi) = (0.0f32, 0.0f32);
            for &v in chunk {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            data.push((lo, hi));
        }
        carry.drain(..full);
        if data.len() > 100 * 60 * 60 * 6 {
            break;
        }
    }
    Some(Peaks { per_second: 100.0, data })
}

/// A small RGBA thumbnail of a picture at `secs`.
pub fn thumbnail(path: &Path, secs: f64, max_w: u32) -> Option<(u32, u32, Vec<u8>)> {
    let mut dec = me::VideoDecoder::open(path).ok()?;
    let frame = match dec.frame_at(secs) {
        Ok(Some(f)) => f,
        _ => {
            let mut d2 = me::VideoDecoder::open(path).ok()?;
            d2.next_frame().ok()??
        }
    };
    let (w, h) = (frame.width, frame.height);
    if w == 0 || h == 0 {
        return None;
    }
    let nw = max_w.min(w);
    let nh = ((h as f64 * nw as f64 / w as f64).round() as u32).max(1);
    Some((nw, nh, util::resize_rgba(w, h, &frame.data, nw, nh)))
}

pub fn proxy_map(st: &State) -> HashMap<PathBuf, PathBuf> {
    let mut map = HashMap::new();
    if st.settings.proxy_mode == 2 {
        return map;
    }
    let pm = proxy_manager(&st.settings);
    if let Some(s) = &st.session {
        for a in s.project().assets().values() {
            if a.has_video() && !a.is_generated() {
                if let Some(p) = pm.existing(&a.path) {
                    map.insert(a.path.clone(), p);
                }
            }
        }
    }
    map
}

fn proxy_manager(s: &AppSettings) -> me::ProxyManager {
    let height = if s.proxy_res == 0 { 540 } else { 720 };
    me::ProxyManager::new(s.proxy_dir()).with_settings(me::ProxySettings { height, keyint: 12 })
}

// ---------------------------------------------------------------- import

fn sources() -> Vec<(String, PathBuf)> {
    let mut v = Vec::new();
    if let Some(h) = dirs::home_dir() {
        v.push(("This computer".to_string(), h.clone()));
    }
    // removable drives: camera cards, phones, USB sticks
    let disks = sysinfo::Disks::new_with_refreshed_list();
    for d in disks.list() {
        if d.is_removable() {
            let name = d.name().to_string_lossy().to_string();
            let label = if name.is_empty() { d.mount_point().to_string_lossy().to_string() } else { name };
            v.push((format!("Drive · {label}"), d.mount_point().to_path_buf()));
        }
    }
    for (label, dir) in [
        ("Videos", dirs::video_dir()),
        ("Pictures", dirs::picture_dir()),
        ("Music", dirs::audio_dir()),
        ("Downloads", dirs::download_dir()),
        ("Desktop", dirs::desktop_dir()),
    ] {
        if let Some(d) = dir {
            if d.exists() {
                v.push((label.to_string(), d));
            }
        }
    }
    v
}

pub fn enter_import(st: &mut State, ui: &App) {
    let src = sources();
    let m = ui.global::<Model>();
    m.set_import_sources(strings(src.iter().map(|s| s.0.clone())));
    m.set_import_source_paths(strings(src.iter().map(|s| s.1.to_string_lossy().to_string())));
    let start = if !st.settings.last_import_dir.is_empty() && Path::new(&st.settings.last_import_dir).is_dir() {
        PathBuf::from(&st.settings.last_import_dir)
    } else {
        dirs::video_dir().filter(|d| d.exists()).or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("/"))
    };
    let idx = src.iter().position(|s| start.starts_with(&s.1) && s.0 != "This computer").unwrap_or(0);
    m.set_import_source(idx as i32);
    m.set_import_method(if st.session.is_some() { 0 } else { 1 });
    m.set_import_proxies(if st.settings.proxy_mode == 2 { 1 } else { 0 });
    open_dir(st, ui, start);
}

fn open_dir(st: &mut State, ui: &App, dir: PathBuf) {
    let mut entries = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let md = match e.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if md.is_dir() {
                entries.push((p, 3, 0));
            } else {
                let k = util::file_kind(&p);
                if k <= 2 {
                    entries.push((p, k, md.len()));
                }
            }
        }
    }
    entries.sort_by(|a, b| {
        (a.1 != 3)
            .cmp(&(b.1 != 3))
            .then_with(|| a.0.file_name().unwrap_or_default().to_ascii_lowercase().cmp(&b.0.file_name().unwrap_or_default().to_ascii_lowercase()))
    });
    st.jobs.browse_dir = dir.clone();
    st.jobs.browse_entries = entries;
    st.jobs.browse_selected.clear();
    st.settings.last_import_dir = dir.to_string_lossy().to_string();
    let m = ui.global::<Model>();
    m.set_import_path(dir.to_string_lossy().to_string().into());
    let crumbs = shorten_home(&dir).replace('/', "  /  ").replace('\\', "  /  ");
    m.set_import_crumbs(crumbs.into());
    refresh_files(st, ui);
    // thumbnails for the first screenfuls
    let todo: Vec<PathBuf> = st
        .jobs
        .browse_entries
        .iter()
        .filter(|e| e.1 == 0 || e.1 == 2)
        .map(|e| e.0.clone())
        .filter(|p| !st.jobs.file_thumbs.contains_key(p) && !st.jobs.thumbs_pending.contains(p))
        .take(200)
        .collect();
    if todo.is_empty() {
        return;
    }
    for p in &todo {
        st.jobs.thumbs_pending.insert(p.clone());
    }
    thread::spawn(move || {
        for p in todo {
            let t = thumbnail(&p, 1.0, 200);
            let dur = if util::file_kind(&p) == 0 { me::probe(&p).ok().and_then(|i| i.duration_secs) } else { None };
            later(move |st, ui| {
                st.jobs.thumbs_pending.remove(&p);
                if let Some((w, h, px)) = t {
                    st.jobs.file_thumbs.insert(p.clone(), util::rgba_image(w, h, &px, 200));
                }
                if let Some(d) = dur {
                    st.sys.durations.insert(p.clone(), d);
                }
                if ui.global::<Model>().get_screen() == Screen::Import {
                    refresh_files(st, ui);
                }
            });
        }
    });
}

fn refresh_files(st: &mut State, ui: &App) {
    let filter = st.jobs.browse_filter.to_lowercase();
    let mut items = Vec::new();
    let mut total_bytes = 0u64;
    let mut total_secs = 0.0;
    for (p, k, size) in &st.jobs.browse_entries {
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
            continue;
        }
        let sel = st.jobs.browse_selected.contains(p);
        let dur = st.sys.durations.get(p).copied();
        if sel {
            total_bytes += size;
            total_secs += dur.unwrap_or(0.0);
        }
        let thumb = st.jobs.file_thumbs.get(p).cloned();
        items.push(FileItem {
            name: name.into(),
            path: p.to_string_lossy().to_string().into(),
            kind: *k,
            size: if *k == 3 { "".into() } else { util::bytes(*size).into() },
            duration: match (k, dur) {
                (2, _) => "Image".into(),
                (_, Some(d)) => util::short_duration(d).into(),
                _ => "".into(),
            },
            has_thumb: thumb.is_some(),
            thumb: thumb.unwrap_or_default(),
            selected: sel,
        });
    }
    let m = ui.global::<Model>();
    m.set_files(vec_model(items));
    let n = st.jobs.browse_selected.len();
    m.set_files_selected(n as i32);
    m.set_files_selected_text(
        format!(
            "{} · Total {}",
            util::bytes(total_bytes),
            util::timecode(total_secs, 30.0, false)
        )
        .into(),
    );
}

fn run_import(st: &mut State, ui: &App, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        st.toast(ui, "Pick some files first.");
        return;
    }
    if st.session.is_none() {
        // Import from Home: start a project with the defaults, then import.
        let name = paths[0].file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled project".into());
        let code = st.settings.resolution * 10 + st.settings.frame_rate + st.settings.aspect * 1000 + 100;
        let s = Session::create(&name, code, st.settings.default_fps());
        st.load_session(ui, s);
        let path = crate::system::new_project_path(st, &name);
        if let Some(s) = st.session.as_mut() {
            let _ = s.save(&path);
        }
        crate::system::after_save(st, ui);
    }
    let m = ui.global::<Model>();
    let copy = m.get_import_method() == 0;
    let skip_dupes = m.get_import_skip_dupes();
    let proxies = m.get_import_proxies() == 0 && st.settings.proxy_mode != 2;
    let existing: HashSet<PathBuf> = st.session.as_ref().unwrap().project().assets().values().map(|a| a.path.clone()).collect();
    let media_dir = st
        .session
        .as_ref()
        .and_then(|s| s.path.as_ref())
        .and_then(|p| p.parent().map(|d| d.join("Media")))
        .unwrap_or_else(|| st.settings.storage_dir().join("Media"));
    let count = paths.len();
    m.set_busy(true);
    m.set_busy_text(format!("Importing {count} files").into());
    m.set_busy_progress(0.0);
    thread::spawn(move || {
        let mut done = 0;
        for src in paths {
            done += 1;
            let target = if copy {
                let _ = std::fs::create_dir_all(&media_dir);
                let dst = media_dir.join(src.file_name().unwrap_or_default());
                let same = dst.exists() && std::fs::metadata(&dst).map(|m| m.len()).ok() == std::fs::metadata(&src).map(|m| m.len()).ok();
                if !same {
                    if let Err(e) = std::fs::copy(&src, &dst) {
                        let msg = format!("Could not copy {}: {e}", src.display());
                        later(move |st, ui| st.toast(ui, msg));
                        continue;
                    }
                }
                dst
            } else {
                src.clone()
            };
            let abs = std::path::absolute(&target).unwrap_or(target.clone());
            if skip_dupes && existing.contains(&abs) {
                continue;
            }
            let probed = tl::media::probe_asset(&abs);
            let frac = done as f32 / count as f32;
            let label = format!("Importing {done} of {count}");
            later(move |st, ui| {
                let m = ui.global::<Model>();
                m.set_busy_progress(frac);
                m.set_busy_text(label.into());
                match probed {
                    Ok(asset) => {
                        let r = st.session.as_mut().map(|s| s.import_asset(asset));
                        if let Some(Ok(id)) = r {
                            let a = st.session.as_ref().unwrap().project().asset(id).cloned();
                            if let Some(a) = a {
                                crate::media::request_asset_media(st, &a);
                                if proxies {
                                    if let Some(v) = a.video.first() {
                                        if v.height > 1080 && v.frame_rate.is_some() {
                                            queue_proxy(st, a.path.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => st.toast(ui, format!("Could not import: {e}")),
                }
            });
        }
        later(move |st, ui| {
            let m = ui.global::<Model>();
            m.set_busy(false);
            st.sync(ui);
            st.toast(ui, format!("Imported {count} files."));
            if m.get_screen() == Screen::Import {
                st.go(ui, Screen::Editor);
            }
        });
    });
}

// ---------------------------------------------------------------- inspector

pub fn enter_inspector(st: &mut State, ui: &App) {
    let m = ui.global::<Model>();
    let mut id = m.get_inspector_asset();
    if let Some(s) = &st.session {
        if s.project().asset(AssetId(id.max(0) as u64)).is_none() || id < 0 {
            id = s.project().assets().keys().next().map(|a| a.0 as i32).unwrap_or(-1);
        }
    }
    inspect(st, ui, id);
}

fn kv(k: &str, v: impl Into<String>) -> KV {
    KV { k: k.into(), v: v.into().into() }
}

pub fn inspect(st: &mut State, ui: &App, id: i32) {
    let m = ui.global::<Model>();
    m.set_inspector_asset(id);
    let Some(s) = &st.session else { return };
    let Some(a) = s.project().asset(AssetId(id.max(0) as u64)).cloned() else {
        m.set_inspector_name("".into());
        return;
    };
    m.set_inspector_name(a.name.clone().into());
    m.set_inspector_offline(!a.is_generated() && !a.path.exists());
    let info = if a.is_generated() { None } else { me::probe(&a.path).ok() };
    let md = std::fs::metadata(&a.path).ok();
    let created = md.as_ref().and_then(|m| m.created().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| util::date_time(d.as_secs() as i64));
    let modified = md.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| util::date_time(d.as_secs() as i64));
    let dur = a.duration.map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let mut general = vec![
        kv("File name", a.name.clone()),
        kv("Container", info.as_ref().map(|i| i.format_long_name.clone()).unwrap_or_else(|| "Graphic".into())),
        kv("Duration", if a.duration.is_some() { util::timecode(dur, 30.0, true) } else { "Still".into() }),
        kv("File size", md.as_ref().map(|m| util::bytes(m.len())).unwrap_or_default()),
        kv("Created", created.unwrap_or_default()),
        kv("Modified", modified.unwrap_or_default()),
    ];
    if let Some(i) = &info {
        if let Some(cam) = i.metadata.get("com.apple.quicktime.model").or(i.metadata.get("model")).or(i.metadata.get("encoder")) {
            general.push(kv("Camera / encoder", cam.clone()));
        }
    }
    general.push(kv("Location", shorten_home(a.path.parent().unwrap_or(Path::new("")))));
    let mut video = Vec::new();
    let mut audio = Vec::new();
    let mut colour = Vec::new();
    let mut streams = Vec::new();
    if let Some(i) = &info {
        for (n, v) in i.video.iter().enumerate() {
            streams.push(format!(
                "Video {}|{} · {} × {} · {}",
                n + 1,
                v.codec.to_uppercase(),
                v.width,
                v.height,
                v.frame_rate.map(|r| format!("{:.3} fps", r.as_f64()).replace(".000", "")).unwrap_or_else(|| "still".into())
            ));
            video.push(kv("Codec", v.codec.to_uppercase()));
            video.push(kv("Frame size", format!("{} × {}", v.width, v.height)));
            video.push(kv("Frame rate", v.frame_rate.map(|r| format!("{:.3}", r.as_f64())).unwrap_or_else(|| "—".into())));
            video.push(kv("Pixel format", v.pixel_format.clone()));
            video.push(kv("Bit rate", v.bit_rate.map(|b| format!("{:.1} Mb/s", b as f64 / 1e6)).unwrap_or_else(|| "—".into())));
            video.push(kv("Rotation", format!("{}°", v.rotation)));
            colour.push(kv("Colour", format!("{:?}", v.colour)));
        }
        for (n, au) in i.audio.iter().enumerate() {
            streams.push(format!(
                "Audio {}|{} · {} kHz · {}",
                n + 1,
                au.codec.to_uppercase(),
                au.sample_rate / 1000,
                match au.channels {
                    1 => "Mono".into(),
                    2 => "Stereo".into(),
                    c => format!("{c} channels"),
                }
            ));
            audio.push(kv(&format!("Stream {}", n + 1), format!("{} · {} Hz · {} ch", au.codec.to_uppercase(), au.sample_rate, au.channels)));
            if let Some(l) = &au.language {
                audio.push(kv("Language", l.clone()));
            }
        }
        let meta: Vec<KV> = i.metadata.iter().take(24).map(|(k, v)| kv(k, v.clone())).collect();
        m.set_inspector_meta(vec_model(meta));
    } else {
        m.set_inspector_meta(vec_model(vec![]));
    }
    m.set_inspector_general(vec_model(general));
    m.set_inspector_video(vec_model(video));
    m.set_inspector_audio(vec_model(audio));
    m.set_inspector_colour(vec_model(colour));
    m.set_inspector_streams(strings(streams));
    m.set_inspector_len(dur as f32);
    m.set_inspector_len_text(util::timecode(dur, 30.0, false).into());
    let pos = m.get_inspector_pos().min(dur as f32).max(0.0);
    m.set_inspector_pos(pos);
    m.set_inspector_pos_text(util::timecode(pos as f64, 30.0, false).into());
    source_frame(st, ui, pos as f64);
    st.sync(ui);
}

fn source_frame(st: &mut State, ui: &App, secs: f64) {
    let m = ui.global::<Model>();
    let id = m.get_inspector_asset();
    let Some(s) = &st.session else { return };
    let Some(a) = s.project().asset(AssetId(id.max(0) as u64)).cloned() else { return };
    if !a.has_video() || a.is_generated() || st.jobs.inspector_frame_pending {
        if !a.has_video() {
            m.set_has_source_preview(false);
        }
        return;
    }
    st.jobs.inspector_frame_pending = true;
    thread::spawn(move || {
        let t = thumbnail(&a.path, secs, 960);
        later(move |st, ui| {
            st.jobs.inspector_frame_pending = false;
            let m = ui.global::<Model>();
            if let Some((w, h, px)) = t {
                m.set_source_preview(util::rgba_image(w, h, &px, 960));
                m.set_has_source_preview(true);
            }
        });
    });
}

// ---------------------------------------------------------------- proxies

pub fn enter_proxy(st: &mut State, ui: &App) {
    refresh_proxy(st, ui);
}

pub fn refresh_proxy(st: &mut State, ui: &App) {
    let m = ui.global::<Model>();
    let pdir = st.settings.proxy_dir();
    let cdir = st.settings.cache_dir();
    let proxy_bytes = util::dir_size(&pdir);
    let cache_bytes = util::dir_size(&cdir).saturating_sub(proxy_bytes);
    m.set_proxy_size(util::bytes(proxy_bytes).into());
    m.set_cache_size(util::bytes(cache_bytes).into());
    m.set_free_space(util::free_space(&cdir).map(util::bytes).unwrap_or_else(|| "—".into()).into());
    m.set_proxy_mode(st.settings.proxy_mode);
    m.set_proxy_res(st.settings.proxy_res);
    m.set_cache_limit_gb(st.settings.cache_limit_gb);
    m.set_cache_location(st.settings.cache_location.clone().into());
    let pm = proxy_manager(&st.settings);
    let mut rows = Vec::new();
    if let Some(s) = &st.session {
        for a in s.project().assets().values() {
            let Some(v) = a.video.first() else { continue };
            if v.frame_rate.is_none() || a.is_generated() {
                continue;
            }
            let state = st.jobs.proxies.get(&a.path).copied().or_else(|| pm.existing(&a.path).map(|_| ProxyState::Ready));
            let (status, text) = match state {
                Some(ProxyState::Ready) => (1, "Ready".to_string()),
                Some(ProxyState::Working(f)) => (2, format!("Generating {:.0}%", f * 100.0)),
                Some(ProxyState::Queued) => (3, "Queued".to_string()),
                Some(ProxyState::Failed) => (4, "Failed".to_string()),
                None => (0, "None".to_string()),
            };
            let ph = if st.settings.proxy_res == 0 { 540 } else { 720 }.min(v.height);
            let pw = (v.width as f64 * ph as f64 / v.height.max(1) as f64).round() as u32 / 2 * 2;
            rows.push(Row4 {
                a: a.name.clone().into(),
                b: format!("{} × {}", v.width, v.height).into(),
                c: format!("{pw} × {ph} · H.264 proxy").into(),
                d: "".into(),
                status,
                status_text: text.into(),
            });
        }
    }
    m.set_proxy_rows(vec_model(rows));
}

pub fn queue_proxy(st: &mut State, path: PathBuf) {
    if matches!(st.jobs.proxies.get(&path), Some(ProxyState::Working(_)) | Some(ProxyState::Queued)) {
        return;
    }
    st.jobs.proxies.insert(path, ProxyState::Queued);
    run_next_proxy(st);
}

fn run_next_proxy(st: &mut State) {
    if st.jobs.proxy_running {
        return;
    }
    let Some(path) = st.jobs.proxies.iter().find(|(_, s)| **s == ProxyState::Queued).map(|(p, _)| p.clone()) else { return };
    st.jobs.proxy_running = true;
    st.jobs.proxies.insert(path.clone(), ProxyState::Working(0.0));
    let pm = proxy_manager(&st.settings);
    thread::spawn(move || {
        let p2 = path.clone();
        let r = pm.ensure_with_progress(&path, |pr| {
            if let Some(f) = pr.fraction() {
                let p3 = p2.clone();
                later(move |st, ui| {
                    st.jobs.proxies.insert(p3, ProxyState::Working(f as f32));
                    if ui.global::<Model>().get_screen() == Screen::Proxy {
                        refresh_proxy(st, ui);
                    }
                });
            }
            true
        });
        later(move |st, ui| {
            st.jobs.proxy_running = false;
            st.jobs.proxies.insert(path, if r.is_ok() { ProxyState::Ready } else { ProxyState::Failed });
            if let Some(s) = st.session.as_mut() {
                s.bump(); // rebuild the preview so it picks up the proxy
            }
            if ui.global::<Model>().get_screen() == Screen::Proxy {
                refresh_proxy(st, ui);
            }
            run_next_proxy(st);
        });
    });
}

// ---------------------------------------------------------------- export

pub const PRESETS: [&str; 7] = ["YouTube", "Instagram Reels", "TikTok", "Vimeo", "Broadcast", "ProRes master", "Custom"];

fn preset_values(i: i32) -> (i32, i32, i32, i32, i32) {
    // format, res, fps, quality, audio
    match i {
        1 | 2 => (0, 0, 1, 1, 0),
        3 => (0, 0, 1, 2, 0),
        4 => (1, 0, 1, 2, 0),
        5 => (1, 0, 1, 2, 0),
        _ => (0, 0, 1, 1, 0),
    }
}

pub fn enter_export(st: &mut State, ui: &App) {
    let m = ui.global::<Model>();
    if let Some(s) = &st.session {
        m.set_export_name(s.name().into());
        let fps = s.fps();
        m.set_export_fps(if fps < 27.0 { 0 } else if fps < 45.0 { 1 } else { 2 });
        let (w, h) = s.size();
        let short = w.min(h);
        m.set_export_res(if short >= 2160 { 2 } else if short >= 1440 { 1 } else { 0 });
    }
    m.set_export_folder(st.settings.export_folder.clone().into());
    m.set_export_queue(vec_model(st.jobs.export_log.clone()));
    update_export_summary(st, ui);
}

fn export_settings(st: &State, ui: &App) -> (me::ExportSettings, String, Option<TimeRange>) {
    let m = ui.global::<Model>();
    let preset = m.get_export_preset();
    let format = m.get_export_format();
    let ep = if preset == 5 {
        me::ExportPreset::MovProRes422Hq
    } else {
        match format {
            1 => me::ExportPreset::MovH264,
            2 => me::ExportPreset::WebmVp9,
            3 => me::ExportPreset::MkvH264,
            _ => me::ExportPreset::Mp4H264,
        }
    };
    let ext = match ep {
        me::ExportPreset::MovH264 | me::ExportPreset::MovProRes422Hq => "mov",
        me::ExportPreset::WebmVp9 => "webm",
        me::ExportPreset::MkvH264 => "mkv",
        _ => "mp4",
    };
    let quality = match m.get_export_quality() {
        0 => me::Quality::Draft,
        2 => me::Quality::High,
        _ => me::Quality::Standard,
    };
    let (w, h) = st.session.as_ref().map(|s| s.size()).unwrap_or((1920, 1080));
    let short: u32 = match m.get_export_res() {
        1 => 1440,
        2 => 2160,
        _ => 1080,
    };
    let height = if h > w { (short as f64 * h as f64 / w as f64).round() as u32 / 2 * 2 } else { short.min(h.max(short)) };
    let fps = match m.get_export_fps() {
        0 => 24.0,
        2 => 60.0,
        _ => 30.0,
    };
    let mut es = me::ExportSettings::new(ep).quality(quality).height(height).fps(fps);
    es = es.audio_bitrate(if m.get_export_audio() == 0 { 320_000 } else { 192_000 });
    es = es.hw_encode(if st.settings.hw_accel { me::HwPolicy::Auto } else { me::HwPolicy::Off });
    let range = if m.get_export_range() == 1 {
        st.session.as_ref().and_then(|s| match (s.in_point, s.out_point) {
            (i, Some(o)) => TimeRange::new(Time::from_secs_f64(i.unwrap_or(0.0)), Time::from_secs_f64(o)),
            (Some(i), None) => TimeRange::new(Time::from_secs_f64(i), s.sequence().duration()),
            _ => None,
        })
    } else {
        None
    };
    (es, ext.to_string(), range)
}

pub fn update_export_summary(st: &mut State, ui: &App) {
    let m = ui.global::<Model>();
    let (es, ext, range) = export_settings(st, ui);
    let dur = match range {
        Some(r) => r.duration().as_secs_f64(),
        None => st.session.as_ref().map(|s| s.duration()).unwrap_or(0.0),
    };
    m.set_export_summary_duration(util::short_duration(dur).into());
    let codec = match es.preset {
        me::ExportPreset::MovProRes422Hq => "ProRes 422 HQ",
        me::ExportPreset::WebmVp9 => "VP9",
        _ => "H.264",
    };
    m.set_export_summary_codec(format!("{codec} · .{ext}").into());
    m.set_export_summary_encoder(
        if st.settings.hw_accel && es.preset != me::ExportPreset::MovProRes422Hq && es.preset != me::ExportPreset::WebmVp9 {
            "GPU when available"
        } else {
            "Processor"
        }
        .into(),
    );
    let h = es.height.unwrap_or(1080) as f64;
    let q = match es.quality {
        me::Quality::Draft => 0.5,
        me::Quality::High => 1.8,
        _ => 1.0,
    };
    let mbps = if es.preset == me::ExportPreset::MovProRes422Hq { 220.0 * (h / 1080.0).powi(2) } else { 8.0 * (h / 1080.0).powf(1.6) * q * es.fps.unwrap_or(30.0) / 30.0 };
    let bytes = ((mbps * 1e6 / 8.0) + 40_000.0) * dur;
    m.set_export_summary_size(util::bytes(bytes as u64).into());
}

fn start_export_job(st: &mut State, ui: &App, job: QueuedExport) {
    let m = ui.global::<Model>();
    let cancel = Arc::new(AtomicBool::new(false));
    st.jobs.export_cancel = Some(Arc::clone(&cancel));
    m.set_exporting(true);
    m.set_export_progress(0.0);
    m.set_export_status(format!("Exporting {}", job.name).into());
    st.jobs.export_log.insert(0, ExportJob { name: job.name.clone().into(), status: "Starting".into(), progress: 0.0, done: false, failed: false });
    m.set_export_queue(vec_model(st.jobs.export_log.clone()));
    let plugins = Arc::clone(&st.plugins);
    thread::spawn(move || {
        let opts = re::ExportOptions { range: job.range, plugins: Some(plugins), ..Default::default() };
        let c2 = Arc::clone(&cancel);
        let r = re::export_sequence(&job.project, job.seq, &job.output, &job.settings, &opts, |p| {
            let f = p.fraction.unwrap_or(0.0) as f32;
            let label = format!("{:?} · {} of {} frames", p.phase, p.frames, p.frames_total);
            later(move |st, ui| {
                let m = ui.global::<Model>();
                m.set_export_progress(f);
                m.set_export_status(label.clone().into());
                if let Some(j) = st.jobs.export_log.first_mut() {
                    j.progress = f;
                    j.status = format!("{:.0}%", f * 100.0).into();
                }
                m.set_export_queue(vec_model(st.jobs.export_log.clone()));
            });
            !c2.load(Ordering::Relaxed)
        });
        let out = job.output.clone();
        later(move |st, ui| {
            let m = ui.global::<Model>();
            m.set_exporting(false);
            st.jobs.export_cancel = None;
            let (msg, failed) = match &r {
                Ok(rep) => (format!("Exported {} frames to {}", rep.frames, shorten_home(&out)), false),
                Err(e) => (format!("Export stopped: {e}"), true),
            };
            if let Some(j) = st.jobs.export_log.first_mut() {
                j.done = !failed;
                j.failed = failed;
                j.progress = if failed { j.progress } else { 1.0 };
                j.status = if failed { "Failed".into() } else { "Done".into() };
            }
            m.set_export_queue(vec_model(st.jobs.export_log.clone()));
            m.set_export_status(msg.clone().into());
            st.toast(ui, msg);
            if !failed {
                if let Some(dir) = out.parent() {
                    let _ = open::that_detached(dir);
                }
            }
            if !st.jobs.export_queue.is_empty() {
                let next = st.jobs.export_queue.remove(0);
                start_export_job(st, ui, next);
            }
        });
    });
}

fn make_job(st: &mut State, ui: &App) -> Option<QueuedExport> {
    let m = ui.global::<Model>();
    let s = st.session.as_ref()?;
    if s.duration() <= 0.0 {
        st.toast(ui, "The timeline is empty.");
        return None;
    }
    let (es, ext, range) = export_settings(st, ui);
    let s = st.session.as_ref()?;
    let folder = expand_home(m.get_export_folder().as_str());
    let _ = std::fs::create_dir_all(&folder);
    let mut name = m.get_export_name().to_string();
    if name.trim().is_empty() {
        name = s.name();
    }
    let mut out = folder.join(format!("{name}.{ext}"));
    let mut n = 2;
    while out.exists() {
        out = folder.join(format!("{name} ({n}).{ext}"));
        n += 1;
    }
    Some(QueuedExport { name, output: out, settings: es, range, project: s.project().clone(), seq: s.seq })
}

// ---------------------------------------------------------------- wiring

pub fn wire(ui: &App) {
    let b = ui.global::<Backend>();

    b.on_import_select_source(|i| {
        with(|st, ui| {
            let m = ui.global::<Model>();
            m.set_import_source(i);
            let p = m.get_import_source_paths().row_data(i as usize).unwrap_or_default();
            open_dir(st, ui, PathBuf::from(p.as_str()));
        });
    });
    b.on_import_open_dir(|p| {
        with(|st, ui| open_dir(st, ui, PathBuf::from(p.as_str())));
    });
    b.on_import_up(|| {
        with(|st, ui| {
            if let Some(parent) = st.jobs.browse_dir.parent().map(|p| p.to_path_buf()) {
                open_dir(st, ui, parent);
            }
        });
    });
    b.on_import_toggle(|i| {
        with(|st, ui| {
            let files = ui.global::<Model>().get_files();
            if let Some(f) = files.row_data(i as usize) {
                let p = PathBuf::from(f.path.as_str());
                if f.kind == 3 {
                    open_dir(st, ui, p);
                    return;
                }
                if !st.jobs.browse_selected.remove(&p) {
                    st.jobs.browse_selected.insert(p);
                }
                refresh_files(st, ui);
            }
        });
    });
    b.on_import_select_all(|on| {
        with(|st, ui| {
            st.jobs.browse_selected.clear();
            if on {
                for (p, k, _) in &st.jobs.browse_entries {
                    if *k != 3 {
                        st.jobs.browse_selected.insert(p.clone());
                    }
                }
            }
            refresh_files(st, ui);
        });
    });
    b.on_import_search(|t| {
        with(|st, ui| {
            st.jobs.browse_filter = t.to_string();
            refresh_files(st, ui);
        });
    });
    b.on_import_run(|| {
        with(|st, ui| {
            let paths: Vec<PathBuf> = st.jobs.browse_entries.iter().filter(|e| st.jobs.browse_selected.contains(&e.0)).map(|e| e.0.clone()).collect();
            st.settings.save();
            run_import(st, ui, paths);
        });
    });
    b.on_import_footage_dialog(|| {
        let start = with(|st, _| st.settings.last_import_dir.clone()).unwrap_or_default();
        let mut dlg = rfd::FileDialog::new().set_title("Import footage").add_filter(
            "Media",
            &["mp4", "mov", "mkv", "webm", "avi", "m4v", "mxf", "mts", "wav", "mp3", "aac", "m4a", "flac", "ogg", "png", "jpg", "jpeg", "tif", "tiff", "webp", "gif"],
        );
        if !start.is_empty() {
            dlg = dlg.set_directory(start);
        }
        if let Some(files) = dlg.pick_files() {
            with(|st, ui| run_import(st, ui, files));
        }
    });
    b.on_inspect_asset(|id| {
        with(|st, ui| {
            let m = ui.global::<Model>();
            m.set_inspector_asset(id);
            if m.get_screen() == Screen::Inspector {
                inspect(st, ui, id);
            } else {
                m.set_inspector_pos(0.0);
                source_frame(st, ui, 0.0);
                st.sync(ui);
            }
        });
    });
    b.on_inspector_seek(|t| {
        with(|st, ui| {
            let m = ui.global::<Model>();
            let t = t.clamp(0.0, m.get_inspector_len().max(0.0));
            m.set_inspector_pos(t);
            m.set_inspector_pos_text(util::timecode(t as f64, 30.0, false).into());
            source_frame(st, ui, t as f64);
        });
    });
    b.on_inspector_play(|| {
        with(|st, ui| {
            let id = ui.global::<Model>().get_inspector_asset();
            if id >= 0 {
                st.go(ui, Screen::Editor);
                st.edit(ui, |s| {
                    let at = s.playhead;
                    s.insert_asset(AssetId(id as u64), at, None, None)
                });
            }
        });
    });
    b.on_reveal_asset(|| {
        with(|st, ui| {
            let id = ui.global::<Model>().get_inspector_asset();
            if let Some(a) = st.session.as_ref().and_then(|s| s.project().asset(AssetId(id.max(0) as u64))) {
                if let Some(dir) = a.path.parent() {
                    let _ = open::that_detached(dir);
                }
            }
        });
    });
    b.on_relink_asset(|| {
        let id = with(|_, ui| ui.global::<Model>().get_inspector_asset()).unwrap_or(-1);
        if id < 0 {
            return;
        }
        if let Some(p) = rfd::FileDialog::new().set_title("Find the file").pick_file() {
            with(|st, ui| {
                st.edit(ui, |s| s.editor.relink_asset(AssetId(id as u64), p.clone()).map_err(|e| e.to_string()).map(|_| s.bump()));
                inspect(st, ui, id);
            });
        }
    });
    b.on_remove_asset(|id| {
        with(|st, ui| {
            st.edit(ui, |s| {
                s.editor.remove_asset(AssetId(id as u64)).map_err(|e| e.to_string())?;
                s.bump();
                Ok(())
            });
            enter_inspector(st, ui);
        });
    });

    b.on_clear_cache(|| {
        with(|st, ui| {
            let cdir = st.settings.cache_dir();
            if let Ok(rd) = std::fs::read_dir(&cdir) {
                for e in rd.flatten() {
                    if e.file_name() != "Proxies" {
                        let _ = if e.path().is_dir() { std::fs::remove_dir_all(e.path()) } else { std::fs::remove_file(e.path()) };
                    }
                }
            }
            // delete proxies no longer used by the open project
            if let Some(s) = &st.session {
                let live: Vec<PathBuf> = s.project().assets().values().map(|a| a.path.clone()).collect();
                let _ = proxy_manager(&st.settings).prune(&live);
            }
            refresh_proxy(st, ui);
            st.toast(ui, "Cache cleared.");
        });
    });
    b.on_regenerate_proxies(|| {
        with(|st, ui| {
            let paths: Vec<PathBuf> = st
                .session
                .as_ref()
                .map(|s| s.project().assets().values().filter(|a| a.video.first().is_some_and(|v| v.frame_rate.is_some()) && !a.is_generated()).map(|a| a.path.clone()).collect())
                .unwrap_or_default();
            if paths.is_empty() {
                st.toast(ui, "No video in this project yet.");
                return;
            }
            let pm = proxy_manager(&st.settings);
            for p in paths {
                if let Ok(pp) = pm.proxy_path(&p) {
                    let _ = std::fs::remove_file(pp);
                }
                st.jobs.proxies.remove(&p);
                queue_proxy(st, p);
            }
            refresh_proxy(st, ui);
        });
    });
    b.on_set_proxy_mode(|i| {
        with(|st, ui| {
            st.settings.proxy_mode = i;
            st.settings.save();
            if i == 1 {
                let paths: Vec<PathBuf> = st.session.as_ref().map(|s| s.project().assets().values().filter(|a| a.video.first().is_some_and(|v| v.frame_rate.is_some())).map(|a| a.path.clone()).collect()).unwrap_or_default();
                for p in paths {
                    queue_proxy(st, p);
                }
            }
            if let Some(s) = st.session.as_mut() {
                s.bump();
            }
            refresh_proxy(st, ui);
        });
    });
    b.on_set_proxy_res(|i| {
        with(|st, ui| {
            st.settings.proxy_res = i;
            st.settings.save();
            refresh_proxy(st, ui);
        });
    });
    b.on_set_cache_limit(|gb| {
        with(|st, _| {
            st.settings.cache_limit_gb = gb.round();
            st.settings.save();
        });
    });
    b.on_choose_cache_location(|| {
        if let Some(d) = rfd::FileDialog::new().set_title("Cache location").pick_folder() {
            with(|st, ui| {
                st.settings.cache_location = shorten_home(&d);
                st.settings.save();
                refresh_proxy(st, ui);
            });
        }
    });

    b.on_export_preset(|i| {
        with(|st, ui| {
            let m = ui.global::<Model>();
            m.set_export_preset(i);
            let (f, r, fps, q, a) = preset_values(i);
            if i != 6 {
                m.set_export_format(f);
                m.set_export_res(r);
                m.set_export_fps(fps);
                m.set_export_quality(q);
                m.set_export_audio(a);
            }
            update_export_summary(st, ui);
        });
    });
    b.on_export_changed(|| {
        with(|st, ui| update_export_summary(st, ui));
    });
    b.on_export_run(|| {
        with(|st, ui| {
            if ui.global::<Model>().get_exporting() {
                st.toast(ui, "An export is running. Added to the queue instead.");
                if let Some(j) = make_job(st, ui) {
                    st.jobs.export_queue.push(j);
                }
                return;
            }
            if let Some(j) = make_job(st, ui) {
                start_export_job(st, ui, j);
            }
        });
    });
    b.on_export_queue_add(|| {
        with(|st, ui| {
            if let Some(j) = make_job(st, ui) {
                let name = j.name.clone();
                if ui.global::<Model>().get_exporting() {
                    st.jobs.export_queue.push(j);
                    st.jobs.export_log.push(ExportJob { name: name.clone().into(), status: "Queued".into(), progress: 0.0, done: false, failed: false });
                    ui.global::<Model>().set_export_queue(vec_model(st.jobs.export_log.clone()));
                    st.toast(ui, format!("{name} added to the queue."));
                } else {
                    start_export_job(st, ui, j);
                }
            }
        });
    });
    b.on_export_cancel(|| {
        with(|st, _| {
            st.jobs.export_queue.clear();
            if let Some(c) = &st.jobs.export_cancel {
                c.store(true, Ordering::Relaxed);
            }
        });
    });
    b.on_choose_export_folder(|| {
        if let Some(d) = rfd::FileDialog::new().set_title("Export folder").pick_folder() {
            with(|st, ui| {
                st.settings.export_folder = shorten_home(&d);
                st.settings.save();
                ui.global::<Model>().set_export_folder(st.settings.export_folder.clone().into());
            });
        }
    });

    b.on_xchg_import_file(|| {
        let fmt = with(|_, ui| ui.global::<Model>().get_xchg_import()).unwrap_or(0);
        let (label, exts): (&str, &[&str]) = match fmt {
            1 => ("EDL", &["edl"]),
            2 => ("OpenTimelineIO", &["otio"]),
            _ => ("Final Cut Pro XML", &["fcpxml", "xml"]),
        };
        if let Some(p) = rfd::FileDialog::new().set_title("Choose a timeline file").add_filter(label, exts).pick_file() {
            with(|st, ui| crate::system::import_timeline(st, ui, &p, fmt));
        }
    });
    b.on_xchg_export_file(|| {
        let (fmt, name) = with(|st, ui| (ui.global::<Model>().get_xchg_export(), st.session.as_ref().map(|s| s.name()).unwrap_or_default())).unwrap_or((0, String::new()));
        let ext = match fmt {
            1 => "edl",
            2 => "otio",
            _ => "fcpxml",
        };
        if let Some(p) = rfd::FileDialog::new().set_title("Export timeline").set_file_name(format!("{name}.{ext}")).save_file() {
            with(|st, ui| crate::system::export_timeline(st, ui, &p, fmt));
        }
    });
}

pub fn _unused(_: SharedString) {}
