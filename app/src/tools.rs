// SPDX-License-Identifier: GPL-3.0-or-later
//! Sections C to F: precision trim, multicam, speed, transitions, mixer, audio cleanup, colour,
//! grading, scopes, effects, titles, keyframes, masks and composition.

use std::sync::Arc;
use std::thread;
use std::time::Instant;

use slint::{ComponentHandle, Image, Model as _, SharedString};

use effects_engine as fx;
use media_engine as me;
use motion_graphics_engine as mg;
use timeline_engine as tl;
use tl::{AssetId, ClipId, EffectId, EffectKind, Param, PlaceMode, Time, TrackKind};

use crate::session::{self, Session, KF_PARAMS};
use crate::{later, strings, util, vec_model, with, App, Backend, Model, Screen, State};

#[derive(Default)]
pub struct ToolState {
    pub trim_frames: i32,
    pub trim_edits: usize,
    pub mc_offsets: Vec<f64>,
    pub mc_assets: Vec<AssetId>,
    pub mc_last: Option<Instant>,
    pub frame: Option<me::VideoFrame>,
    pub scopes_dirty: bool,
    pub thumbs_for: Option<(ClipId, u64)>,
    pub ref_stats: Option<[f64; 6]>,
    pub tgt_stats: Option<[f64; 6]>,
    pub fonts: Option<mg::Fonts>,
}

pub fn enter(st: &mut State, ui: &App, screen: Screen) {
    let m = ui.global::<Model>();
    match screen {
        Screen::Trim => {
            st.tools.trim_frames = 0;
            st.tools.trim_edits = 0;
            if let Some(s) = st.session.as_mut() {
                // a cut needs a clip with a neighbour before it
                if let Some(id) = s.ensure_selection() {
                    if s.prev_clip(id).is_none() {
                        if let Some(n) = s.next_clip(id) {
                            s.selected = Some(n);
                        }
                    }
                }
            }
            trim_frames(st, ui);
        }
        Screen::Multicam => mc_setup(st, ui),
        Screen::Transitions => {
            if let Some(s) = st.session.as_mut() {
                if let Some(id) = s.ensure_selection() {
                    if s.prev_clip(id).is_none() {
                        if let Some(n) = s.next_clip(id) {
                            s.selected = Some(n);
                        }
                    }
                }
            }
            let sel = st.session.as_ref().map(|s| s.selected_item());
            if let Some(sel) = sel {
                if sel.transition >= 0 {
                    m.set_transition_pick(sel.transition);
                    m.set_transition_secs(sel.transition_secs);
                    m.set_transition_align(sel.transition_align);
                }
            }
        }
        Screen::AudioCleanup => {
            let names = audio_clips(st).into_iter().map(|(_, n)| n).collect::<Vec<_>>();
            m.set_cleanup_clip(0);
            ui.global::<Model>().set_clip_names(strings(names));
            cleanup_refresh(st, ui);
        }
        Screen::Color | Screen::Grading | Screen::Effects | Screen::Scopes | Screen::Composition | Screen::Keyframes | Screen::Masks => {
            if let Some(s) = st.session.as_mut() {
                s.ensure_selection();
            }
            st.tools.thumbs_for = None;
            st.tools.scopes_dirty = true;
            if screen == Screen::Color || screen == Screen::Grading {
                read_grading(st, ui);
            }
            if screen == Screen::Scopes {
                m.set_scopes_ref(0);
                m.set_scopes_target(1.min(video_clips(st).len().saturating_sub(1)) as i32);
            }
        }
        Screen::Titles => {
            let sel = st.session.as_ref().map(|s| s.selected_item());
            if let Some(sel) = sel.filter(|s| s.is_text) {
                m.set_title_text(sel.text.clone());
                m.set_title_sub(sel.text_sub.clone());
                m.set_title_size(sel.text_size);
                m.set_title_colour(sel.text_colour);
            }
            title_preview(st, ui);
        }
        Screen::Mixer => {}
        _ => {}
    }
}

pub fn on_seek(st: &mut State, ui: &App) {
    let screen = ui.global::<Model>().get_screen();
    if screen == Screen::Multicam {
        mc_thumbs(st, ui, false);
    }
    if screen == Screen::Mixer {
        mixer_levels(st, ui);
    }
    if screen == Screen::Composition || screen == Screen::Masks {
        st.sync(ui);
    }
}

pub fn on_selection(st: &mut State, ui: &App) {
    let screen = ui.global::<Model>().get_screen();
    match screen {
        Screen::Trim => {
            st.tools.trim_frames = 0;
            st.tools.trim_edits = 0;
            trim_frames(st, ui);
        }
        Screen::Color | Screen::Grading => read_grading(st, ui),
        _ => {}
    }
    st.tools.thumbs_for = None;
}

pub fn on_frame(st: &mut State, ui: &App, f: &me::VideoFrame) {
    let screen = ui.global::<Model>().get_screen();
    let wants = matches!(screen, Screen::Scopes | Screen::Color | Screen::Grading | Screen::Effects) || (screen == Screen::Editor && ui.global::<Model>().get_editor_tab() == 1);
    if !wants {
        return;
    }
    // small working copy for scopes and look thumbnails
    let w = 320u32.min(f.width);
    let h = ((f.height as f64 * w as f64 / f.width.max(1) as f64).round() as u32).max(1);
    let small = me::VideoFrame { width: w, height: h, pts_secs: f.pts_secs, data: util::resize_rgba(f.width, f.height, &f.data, w, h) };
    st.tools.frame = Some(small.clone());
    if matches!(screen, Screen::Scopes | Screen::Grading) || screen == Screen::Color {
        scopes(ui, &small);
    }
    if matches!(screen, Screen::Color | Screen::Effects) {
        let key = st.session.as_ref().and_then(|s| s.selected.map(|c| (c, s.revision)));
        if st.tools.thumbs_for != key {
            st.tools.thumbs_for = key;
            look_thumbs(st, ui, screen, &small);
        }
    }
}

// ---------------------------------------------------------------- trim

fn frame_of(path: &std::path::Path, secs: f64) -> Option<Image> {
    crate::media::thumbnail(path, secs, 640).map(|(w, h, px)| util::rgba_image(w, h, &px, 640))
}

fn trim_frames(st: &mut State, ui: &App) {
    let Some(s) = &st.session else { return };
    let Some((_, c)) = s.selected_clip() else { return };
    let inc_asset = s.project().asset(c.asset).cloned();
    let inc_t = c.source_time_at(c.start).as_secs_f64();
    let prev = s.prev_clip(c.id).and_then(|p| s.find_clip(p)).map(|(_, p)| (s.project().asset(p.asset).cloned(), p.source_time_at(p.end() - s.sequence().frame_duration()).as_secs_f64()));
    let m = ui.global::<Model>();
    m.set_trim_offset(st.tools.trim_frames);
    thread::spawn(move || {
        let inc = inc_asset.filter(|a| a.has_video() && !a.is_generated()).and_then(|a| crate::media::thumbnail(&a.path, inc_t, 640));
        let out = prev.and_then(|(a, t)| a.filter(|a| a.has_video() && !a.is_generated()).and_then(|a| crate::media::thumbnail(&a.path, t, 640)));
        later(move |_, ui| {
            let m = ui.global::<Model>();
            match inc {
                Some((w, h, px)) => {
                    m.set_trim_in_img(util::rgba_image(w, h, &px, 640));
                    m.set_has_trim_in(true);
                }
                None => m.set_has_trim_in(false),
            }
            match out {
                Some((w, h, px)) => {
                    m.set_trim_out_img(util::rgba_image(w, h, &px, 640));
                    m.set_has_trim_out(true);
                }
                None => m.set_has_trim_out(false),
            }
        });
    });
    let _ = frame_of;
}

// ---------------------------------------------------------------- multicam

fn video_assets(s: &Session) -> Vec<tl::Asset> {
    s.project().assets().values().filter(|a| a.has_video() && !a.is_generated() && !a.is_still()).cloned().collect()
}

fn mc_setup(st: &mut State, ui: &App) {
    let Some(s) = &st.session else { return };
    let assets: Vec<tl::Asset> = video_assets(s).into_iter().take(4).collect();
    st.tools.mc_assets = assets.iter().map(|a| a.id).collect();
    if st.tools.mc_offsets.len() != assets.len() {
        st.tools.mc_offsets = vec![0.0; assets.len()];
    }
    let m = ui.global::<Model>();
    m.set_mc_angles(strings(assets.iter().map(|a| a.name.clone())));
    m.set_mc_synced(vec_model(vec![false; assets.len()]));
    m.set_mc_thumbs(vec_model(vec![Image::default(); assets.len()]));
    mc_thumbs(st, ui, true);
}

fn mc_thumbs(st: &mut State, ui: &App, force: bool) {
    if !force && st.tools.mc_last.is_some_and(|t| t.elapsed().as_millis() < 250) {
        return;
    }
    st.tools.mc_last = Some(Instant::now());
    let Some(s) = &st.session else { return };
    let ph = s.playhead;
    let jobs: Vec<(usize, std::path::PathBuf, f64)> = st
        .tools
        .mc_assets
        .iter()
        .enumerate()
        .filter_map(|(i, id)| s.project().asset(*id).map(|a| (i, a.path.clone(), (ph - st.tools.mc_offsets.get(i).copied().unwrap_or(0.0)).max(0.0))))
        .collect();
    let _ = ui;
    thread::spawn(move || {
        for (i, p, t) in jobs {
            if let Some((w, h, px)) = crate::media::thumbnail(&p, t, 360) {
                later(move |_, ui| {
                    let m = ui.global::<Model>();
                    let thumbs = m.get_mc_thumbs();
                    if i < thumbs.row_count() {
                        thumbs.set_row_data(i, util::rgba_image(w, h, &px, 360));
                    }
                });
            }
        }
    });
}

/// Offset of `b` relative to `a` (seconds b starts after a) by cross-correlating loudness envelopes.
fn correlate(a: &session::Peaks, b: &session::Peaks, max_shift_s: f64) -> f64 {
    let env = |p: &session::Peaks| -> Vec<f32> { p.data.iter().map(|(lo, hi)| hi.max(-lo)).collect() };
    let (ea, eb) = (env(a), env(b));
    let n = ea.len().min(eb.len()).min(100 * 120);
    if n < 50 {
        return 0.0;
    }
    let max_shift = (max_shift_s * 100.0) as i64;
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
    let (ma, mb) = (mean(&ea[..n]), mean(&eb[..n]));
    let mut best = (f32::MIN, 0i64);
    for shift in -max_shift..=max_shift {
        let mut acc = 0.0f32;
        let mut cnt = 0;
        for i in 0..n as i64 {
            let j = i - shift;
            if j < 0 || j as usize >= eb.len() || i as usize >= ea.len() {
                continue;
            }
            acc += (ea[i as usize] - ma) * (eb[j as usize] - mb);
            cnt += 1;
        }
        if cnt > 20 {
            let score = acc / cnt as f32;
            if score > best.0 {
                best = (score, shift);
            }
        }
    }
    best.1 as f64 / 100.0
}

// ---------------------------------------------------------------- mixer

fn mixer_levels(st: &mut State, ui: &App) {
    let Some(s) = &st.session else { return };
    let t = s.playhead_time();
    let mut levels = Vec::new();
    for track in s.sequence().audio_tracks().iter() {
        let mut lvl = 0.0f32;
        if track.enabled {
            if let Some(c) = track.clip_at(t) {
                if let Some(p) = s.peaks.get(&c.asset) {
                    let src = c.source_time_at(t).as_secs_f64();
                    let i = (src * p.per_second) as usize;
                    let mut amp = 0.0f32;
                    for k in i.saturating_sub(3)..(i + 3).min(p.data.len()) {
                        amp = amp.max(p.data[k].1.max(-p.data[k].0));
                    }
                    lvl = amp * c.gain.linear() * track.gain.linear();
                }
            }
        }
        levels.push(lvl.min(1.0));
    }
    ui.global::<Model>().set_levels(vec_model(levels));
}

fn track_rms(s: &Session, track: &tl::Track) -> Option<f32> {
    let mut acc = 0.0f64;
    let mut n = 0usize;
    for c in track.clips() {
        let p = s.peaks.get(&c.asset)?;
        let a = (c.source_in.as_secs_f64() * p.per_second) as usize;
        let b = ((c.source_in.as_secs_f64() + c.duration.as_secs_f64() * c.speed.as_f64()) * p.per_second) as usize;
        for k in a..b.min(p.data.len()) {
            let v = p.data[k].1.max(-p.data[k].0) as f64 * c.gain.linear() as f64;
            acc += v * v;
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    Some(((acc / n as f64).sqrt()) as f32)
}

// ---------------------------------------------------------------- audio cleanup

fn audio_clips(st: &State) -> Vec<(ClipId, String)> {
    let Some(s) = &st.session else { return vec![] };
    s.sequence().audio_tracks().iter().flat_map(|t| t.clips().map(|c| (c.id, c.name.clone())).collect::<Vec<_>>()).collect()
}

fn wave_image(p: &session::Peaks, from: f64, len: f64, gain: f32, w: u32, h: u32) -> Image {
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
    let px = buf.make_mut_bytes();
    for x in 0..w {
        let t0 = from + len * x as f64 / w as f64;
        let t1 = from + len * (x + 1) as f64 / w as f64;
        let (i0, i1) = ((t0 * p.per_second) as usize, ((t1 * p.per_second) as usize).max((t0 * p.per_second) as usize + 1));
        let mut amp = 0.0f32;
        for i in i0..i1.min(p.data.len()) {
            amp = amp.max(p.data[i].1.max(-p.data[i].0));
        }
        let amp = (amp * gain).min(1.0);
        let bar = (amp * h as f32).round() as u32;
        let top = (h - bar.min(h)) / 2;
        for y in top..(top + bar).min(h) {
            let i = ((y * w + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[255, 255, 255, if amp > 0.98 { 255 } else { 230 }]);
        }
    }
    Image::from_rgba8(buf)
}

fn cleanup_refresh(st: &mut State, ui: &App) {
    let m = ui.global::<Model>();
    let clips = audio_clips(st);
    let Some((id, _)) = clips.get(m.get_cleanup_clip().max(0) as usize).cloned() else {
        m.set_loudness_text("No audio on the timeline yet.".into());
        return;
    };
    let Some(s) = st.session.as_mut() else { return };
    s.selected = Some(id);
    let Some((_, c)) = s.find_clip(id) else { return };
    if let Some(p) = s.peaks.get(&c.asset).cloned() {
        let from = c.source_in.as_secs_f64();
        let len = c.duration.as_secs_f64() * c.speed.as_f64();
        m.set_waveform_original(wave_image(&p, from, len, 1.0, 900, 80));
        m.set_waveform_clean(wave_image(&p, from, len, c.gain.linear(), 900, 80));
    }
    m.set_loudness_text(format!("Clip level: {:+.1} dB", c.gain.db()).into());
}

fn clip_span(s: &Session, id: ClipId) -> Result<(std::path::PathBuf, f64, f64), String> {
    let (_, c) = s.find_clip(id).ok_or("No clip")?;
    let a = s.project().asset(c.asset).ok_or("No media")?;
    Ok((a.path.clone(), c.source_in.as_secs_f64(), c.duration.as_secs_f64() * c.speed.as_f64()))
}

fn measure_clip(path: &std::path::Path, from: f64, len: f64) -> Result<f64, String> {
    let mut dec = me::AudioDecoder::open(path, me::AudioOptions { sample_rate: Some(48_000), channels: Some(2) }).map_err(|e| e.to_string())?;
    dec.seek(from).map_err(|e| e.to_string())?;
    let mut meter = audio_engine::LoudnessMeter::new(48_000, 2);
    let mut got = 0.0;
    while let Ok(Some(buf)) = dec.next_buffer() {
        let start = buf.pts_secs;
        let frames = buf.frames();
        let skip = (((from - start) * 48_000.0).max(0.0) as usize).min(frames);
        let take = ((((from + len) - start) * 48_000.0).max(0.0) as usize).min(frames);
        if take > skip {
            meter.push(&buf.samples[skip * 2..take * 2]);
            got += (take - skip) as f64 / 48_000.0;
        }
        if start > from + len {
            break;
        }
    }
    if got <= 0.0 {
        return Err("That clip has no sound.".into());
    }
    meter.result().integrated_lufs.ok_or_else(|| "That clip is silent.".to_string())
}

// ---------------------------------------------------------------- colour

const LOOKS: [&str; 5] = ["Natural", "Cinematic", "Warm", "Cool", "Mono"];

fn look_values(i: i32) -> [(EffectKind, f64); 5] {
    use EffectKind::*;
    match i {
        1 => [(Temperature, -8.0), (Contrast, 18.0), (Saturation, 85.0), (Exposure, -0.1), (Grayscale, 0.0)],
        2 => [(Temperature, 25.0), (Contrast, 6.0), (Saturation, 110.0), (Exposure, 0.0), (Grayscale, 0.0)],
        3 => [(Temperature, -25.0), (Contrast, 6.0), (Saturation, 95.0), (Exposure, 0.0), (Grayscale, 0.0)],
        4 => [(Temperature, 0.0), (Contrast, 15.0), (Saturation, 100.0), (Exposure, 0.0), (Grayscale, 100.0)],
        _ => [(Temperature, 0.0), (Contrast, 0.0), (Saturation, 100.0), (Exposure, 0.0), (Grayscale, 0.0)],
    }
}

fn read_grading(st: &mut State, ui: &App) {
    let m = ui.global::<Model>();
    let Some(s) = &st.session else { return };
    let Some(id) = s.selected else { return };
    let Some((_, c)) = s.find_clip(id) else { return };
    let mut wheels = vec![0.0f32; 12];
    let mut curve = vec![0.25f32, 0.5, 0.75];
    for e in &c.effects {
        if e.kind == EffectKind::Balance {
            for (i, w) in wheels.iter_mut().enumerate() {
                *w = e.value(i) as f32;
            }
        }
        if e.kind == EffectKind::Curve {
            for (i, v) in curve.iter_mut().enumerate() {
                *v = e.value(i) as f32;
            }
        }
    }
    m.set_wheels(vec_model(wheels));
    m.set_curve_points(vec_model(curve));
    m.set_highlights((s.colour_value(id, EffectKind::Gamma) as f32 - 1.0) * -100.0);
}

fn set_multi_effect(s: &mut Session, id: ClipId, kind: EffectKind, index: usize, value: f64) -> Result<(), String> {
    let existing = s.find_clip(id).and_then(|(_, c)| c.effects.iter().find(|e| e.kind == kind).map(|e| e.id));
    let fx_id = match existing {
        Some(f) => f,
        None => s.add_effect(id, kind)?,
    };
    let name = kind.params()[index].name;
    s.set_effect_value(id, fx_id, name, value)
}

/// Waveform, vectorscope, histogram and RGB parade of a small RGBA frame.
fn scopes(ui: &App, f: &me::VideoFrame) {
    let (w, h) = (f.width as usize, f.height as usize);
    let d = &f.data;
    let m = ui.global::<Model>();
    // waveform (luma), 256 high
    let (ww, wh) = (w.max(1), 128usize);
    let mut wf = vec![0u32; ww * wh];
    let (pw, ph) = (w.max(1), 128usize);
    let mut parade = vec![[0u32; 3]; pw * ph];
    let mut hist = [[0u32; 256]; 3];
    let vs = 128usize;
    let mut vec_px = vec![0u32; vs * vs];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let (r, g, b) = (d[i] as f32, d[i + 1] as f32, d[i + 2] as f32);
            let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let ly = wh - 1 - ((l / 255.0) * (wh - 1) as f32) as usize;
            wf[ly * ww + x] += 1;
            for (c, v) in [r, g, b].iter().enumerate() {
                let py = ph - 1 - ((v / 255.0) * (ph - 1) as f32) as usize;
                parade[py * pw + x][c] += 1;
                hist[c][*v as usize] += 1;
            }
            let cb = -0.1146 * r - 0.3854 * g + 0.5 * b;
            let cr = 0.5 * r - 0.4542 * g - 0.0458 * b;
            let vx = ((cb / 255.0 + 0.5) * (vs - 1) as f32).clamp(0.0, (vs - 1) as f32) as usize;
            let vy = ((0.5 - cr / 255.0) * (vs - 1) as f32).clamp(0.0, (vs - 1) as f32) as usize;
            vec_px[vy * vs + vx] += 1;
        }
    }
    let norm = |n: u32, scale: f32| ((n as f32 * scale).sqrt().min(1.0) * 255.0) as u8;
    let mk = |pw: usize, ph: usize, f: &dyn Fn(usize, usize) -> [u8; 4]| {
        let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(pw as u32, ph as u32);
        let px = buf.make_mut_bytes();
        for y in 0..ph {
            for x in 0..pw {
                let i = (y * pw + x) * 4;
                px[i..i + 4].copy_from_slice(&f(x, y));
            }
        }
        Image::from_rgba8(buf)
    };
    let s1 = 4.0 / (h as f32).max(1.0);
    m.set_scope_waveform(mk(ww, wh, &|x, y| {
        let a = norm(wf[y * ww + x], s1 * 20.0);
        [46, 211, 154, a]
    }));
    m.set_scope_parade(mk(pw * 3, ph, &|x, y| {
        let c = x / pw;
        let a = norm(parade[y * pw + (x % pw)][c], s1 * 20.0);
        match c {
            0 => [229, 72, 77, a],
            1 => [34, 196, 122, a],
            _ => [46, 140, 255, a],
        }
    }));
    let total = (w * h) as f32;
    m.set_scope_vector(mk(vs, vs, &|x, y| {
        let a = norm(vec_px[y * vs + x], 400.0 / total.max(1.0));
        [217, 169, 46, a]
    }));
    let hmax = hist.iter().flat_map(|c| c.iter()).copied().max().unwrap_or(1).max(1) as f32;
    let hh = 100usize;
    m.set_scope_histogram(mk(256, hh, &|x, y| {
        let mut px = [0u8, 0, 0, 0];
        for c in 0..3 {
            let v = (hist[c][x] as f32 / hmax).sqrt();
            if (hh - 1 - y) as f32 <= v * (hh - 1) as f32 {
                px[c] = 220;
                px[3] = 170;
            }
        }
        if px == [0, 0, 0, 0] {
            px
        } else {
            [px[0].max(40), px[1].max(40), px[2].max(40), px[3]]
        }
    }));
}

fn look_thumbs(st: &mut State, ui: &App, screen: Screen, f: &me::VideoFrame) {
    let (w, h) = (f.width as usize, f.height as usize);
    // strip the selected clip's own colour effects so thumbnails show each look from the plain frame
    let mut imgs = Vec::new();
    if screen == Screen::Color {
        for i in 0..LOOKS.len() as i32 {
            let mut px = f.data.clone();
            for (k, v) in look_values(i) {
                fx::apply(k, &[v], &mut px, w, h);
            }
            imgs.push(util::rgba_image(f.width, f.height, &px, 320));
        }
        ui.global::<Model>().set_look_thumbs(vec_model(imgs));
    } else {
        for (k, vals) in EFFECT_TILES.iter().map(|t| (t.1, t.3)) {
            let mut px = f.data.clone();
            fx::apply(k, vals, &mut px, w, h);
            imgs.push(util::rgba_image(f.width, f.height, &px, 320));
        }
        ui.global::<Model>().set_effect_thumbs(vec_model(imgs));
    }
    let _ = st;
}

/// name, kind, category, sample values for the thumbnail
pub const EFFECT_TILES: [(&str, EffectKind, &str, &[f64]); 14] = [
    ("Gaussian blur", EffectKind::Blur, "Blur", &[1.5]),
    ("Sharpen", EffectKind::Sharpen, "Blur", &[150.0, 0.5]),
    ("Mosaic", EffectKind::Pixelate, "Stylize", &[3.0]),
    ("Vignette", EffectKind::Vignette, "Light", &[70.0, 50.0]),
    ("Brightness", EffectKind::Brightness, "Light", &[25.0]),
    ("Exposure", EffectKind::Exposure, "Light", &[0.7]),
    ("Contrast", EffectKind::Contrast, "Color", &[35.0]),
    ("Saturation", EffectKind::Saturation, "Color", &[170.0]),
    ("Hue shift", EffectKind::Hue, "Color", &[60.0]),
    ("Temperature", EffectKind::Temperature, "Color", &[40.0]),
    ("Sepia", EffectKind::Sepia, "Color", &[100.0]),
    ("Black and white", EffectKind::Grayscale, "Color", &[100.0]),
    ("Invert", EffectKind::Invert, "Stylize", &[100.0]),
    ("Gamma", EffectKind::Gamma, "Light", &[1.6]),
];

fn stats(f: &me::VideoFrame) -> [f64; 6] {
    let mut sum = [0.0f64; 3];
    let mut sq = [0.0f64; 3];
    let n = (f.width * f.height) as f64;
    for px in f.data.chunks_exact(4) {
        for c in 0..3 {
            let v = px[c] as f64;
            sum[c] += v;
            sq[c] += v * v;
        }
    }
    let mut out = [0.0; 6];
    for c in 0..3 {
        let mean = sum[c] / n.max(1.0);
        out[c] = mean;
        out[3 + c] = (sq[c] / n.max(1.0) - mean * mean).max(0.0).sqrt();
    }
    out
}

fn video_clips(st: &State) -> Vec<(ClipId, String, AssetId, f64)> {
    let Some(s) = &st.session else { return vec![] };
    s.sequence()
        .video_tracks()
        .iter()
        .flat_map(|t| t.clips().map(|c| (c.id, c.name.clone(), c.asset, c.source_in.as_secs_f64() + 0.5)).collect::<Vec<_>>())
        .collect()
}

// ---------------------------------------------------------------- titles

fn text_generator(st: &State, ui: &App) -> mg::Generator {
    let m = ui.global::<Model>();
    let main = m.get_title_text().to_string();
    let sub = m.get_title_sub().to_string();
    let text = if sub.trim().is_empty() { main } else { format!("{main}\n{sub}") };
    let template = m.get_title_template();
    let align = match template {
        0 | 1 => mg::Align::Left,
        _ => mg::Align::Centre,
    };
    let _ = st;
    mg::Generator::Text(mg::Text {
        text,
        font: None,
        size_pct: (m.get_title_size() as f64 / 4.0).clamp(1.0, 40.0),
        colour: session::colour_of(m.get_title_colour()),
        align,
        line_gap_pct: 10.0,
        tracking_pct: 0.0,
        outline: None,
        shadow: if template == 0 { None } else { Some(mg::Shadow { colour: mg::Colour { r: 0, g: 0, b: 0, a: 160 }, dx_pct: 0.3, dy_pct: 0.3 }) },
    })
}

fn title_preview(st: &mut State, ui: &App) {
    let g = text_generator(st, ui);
    let (w, h) = (640usize, 360usize);
    let fonts = st.tools.fonts.get_or_insert_with(mg::Fonts::new);
    let m = ui.global::<Model>();
    match mg::render(&g, w, h, fonts) {
        Ok(px) => {
            m.set_title_preview(util::rgba_image(w as u32, h as u32, &px, 640));
            m.set_has_title_preview(true);
        }
        Err(e) => {
            m.set_has_title_preview(false);
            st.toast(ui, format!("Title: {e}"));
        }
    }
}

fn add_title(st: &mut State, ui: &App) {
    let g = text_generator(st, ui);
    let m = ui.global::<Model>();
    let template = m.get_title_template();
    let anim_in = m.get_title_in();
    let anim_out = m.get_title_out();
    let anim = (m.get_title_anim_secs() as f64).clamp(0.0, 3.0);
    let name = m.get_title_text().to_string();
    // edit the selected title in place
    let sel_text = st.session.as_ref().and_then(|s| s.selected_clip().and_then(|(_, c)| s.project().asset(c.asset).filter(|a| matches!(a.generator, Some(tl::Generator::Text(_)))).map(|a| a.id)));
    if let Some(aid) = sel_text {
        st.edit(ui, |s| {
            s.editor.set_generated(aid, g.clone()).map_err(|e| e.to_string())?;
            s.bump();
            Ok(())
        });
        st.toast(ui, "Title updated.");
        return;
    }
    st.edit(ui, |s| {
        let aid = s.editor.add_generated(if name.is_empty() { "Title".to_string() } else { name.clone() }, g).map_err(|e| e.to_string())?;
        // top-most video track that is free at the playhead
        let at = s.playhead_time();
        let tracks: Vec<tl::TrackId> = s.sequence().video_tracks().iter().rev().map(|t| t.id).collect();
        let free = tracks
            .iter()
            .copied()
            .find(|t| s.sequence().track(*t).is_some_and(|tr| tr.clips_in(TimeRange::from_start_duration(at, Time::from_secs_f64(5.0)).unwrap()).is_empty()));
        let track = match free {
            Some(t) => t,
            None => s.editor.add_track(s.seq, TrackKind::Video, Some("Titles".into())).map_err(|e| e.to_string())?,
        };
        let mut spec = tl::ClipSpec::new(aid, at);
        spec.duration = Some(Time::from_secs_f64(5.0));
        let seq = s.seq;
        let clip = s.editor.place_clip(seq, track, spec, PlaceMode::Overwrite).map_err(|e| e.to_string())?;
        // lower thirds sit low
        if template <= 1 {
            s.editor.set_look_value(seq, clip, Param::PositionY, 32.0).map_err(|e| e.to_string())?;
            s.editor.set_look_value(seq, clip, Param::PositionX, -25.0).map_err(|e| e.to_string())?;
        }
        let dur = 5.0;
        let key = |s: &mut Session, p: Param, t: f64, v: f64| s.set_keyframe(clip, p, t, v);
        if anim > 0.0 {
            match anim_in {
                1 => {
                    let base = if template <= 1 { -25.0 } else { 0.0 };
                    key(s, Param::PositionX, 0.0, base - 40.0)?;
                    key(s, Param::PositionX, anim, base)?;
                }
                2 => {
                    key(s, Param::ScaleX, 0.0, 70.0)?;
                    key(s, Param::ScaleX, anim, 100.0)?;
                    key(s, Param::ScaleY, 0.0, 70.0)?;
                    key(s, Param::ScaleY, anim, 100.0)?;
                }
                _ => {
                    key(s, Param::Opacity, 0.0, 0.0)?;
                    key(s, Param::Opacity, anim, 100.0)?;
                }
            }
            match anim_out {
                0 => {
                    let base = if template <= 1 { -25.0 } else { 0.0 };
                    key(s, Param::PositionX, dur - anim, base)?;
                    key(s, Param::PositionX, dur - 1.0 / s.fps(), base - 40.0)?;
                }
                _ => {
                    if anim_in != 0 {
                        key(s, Param::Opacity, 0.0, 100.0)?;
                    }
                    key(s, Param::Opacity, dur - anim, 100.0)?;
                    key(s, Param::Opacity, dur - 1.0 / s.fps(), 0.0)?;
                }
            }
        }
        // the bar style gets a coloured band behind
        if template == 0 {
            let bar = mg::Generator::Shape(mg::Shape {
                kind: mg::ShapeKind::Rect { corner_pct: 0.0 },
                width_pct: 38.0,
                height_pct: 14.0,
                fill: mg::Fill::Solid(mg::Colour { r: 46, g: 140, b: 255, a: 235 }),
                stroke: None,
            });
            let bid = s.editor.add_generated("Title bar", bar).map_err(|e| e.to_string())?;
            let below = s.sequence().video_tracks().iter().map(|t| t.id).take_while(|t| *t != track).last();
            if let Some(bt) = below {
                if s.sequence().track(bt).is_some_and(|tr| tr.clips_in(TimeRange::from_start_duration(at, Time::from_secs_f64(5.0)).unwrap()).is_empty()) {
                    let mut bs = tl::ClipSpec::new(bid, at);
                    bs.duration = Some(Time::from_secs_f64(5.0));
                    let bc = s.editor.place_clip(seq, bt, bs, PlaceMode::Overwrite).map_err(|e| e.to_string())?;
                    s.editor.set_look_value(seq, bc, Param::PositionY, 32.0).map_err(|e| e.to_string())?;
                    s.editor.set_look_value(seq, bc, Param::PositionX, -25.0).map_err(|e| e.to_string())?;
                }
            }
        }
        s.selected = Some(clip);
        s.bump();
        Ok(())
    });
}

use tl::TimeRange;

// ---------------------------------------------------------------- wiring

pub fn wire(ui: &App) {
    let b = ui.global::<Backend>();

    // trim
    b.on_trim_nudge(|mode, frames| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            let f = st.session.as_ref().map(|s| s.frame_secs()).unwrap_or(1.0 / 30.0) * frames as f64;
            let r = st.session.as_mut().map(|s| match mode {
                0 => s.trim(id, tl::Edge::Head, f, true),
                1 => s.roll(id, f),
                2 => s.slip(id, f),
                _ => s.slide(id, f),
            });
            match r {
                Some(Ok(())) => {
                    st.tools.trim_frames += frames;
                    st.tools.trim_edits += 1;
                    st.sync(ui);
                    trim_frames(st, ui);
                }
                Some(Err(e)) => st.toast(ui, e),
                None => {}
            }
        });
    });
    b.on_trim_reset(|| {
        with(|st, ui| {
            for _ in 0..st.tools.trim_edits {
                if let Some(s) = st.session.as_mut() {
                    let _ = s.undo();
                }
            }
            st.tools.trim_edits = 0;
            st.tools.trim_frames = 0;
            st.sync(ui);
            trim_frames(st, ui);
        });
    });

    // multicam
    b.on_mc_sync_angles(|| {
        with(|st, ui| {
            let m = ui.global::<Model>();
            let method = m.get_mc_sync();
            let n = st.tools.mc_assets.len();
            if n < 2 {
                st.toast(ui, "Import at least two camera angles first.");
                return;
            }
            let mut offsets = vec![0.0; n];
            if method == 0 {
                let Some(s) = &st.session else { return };
                let base = st.tools.mc_assets[0];
                let Some(pa) = s.peaks.get(&base).cloned() else {
                    st.toast(ui, "Still reading the sound of angle A. Try again in a moment.");
                    return;
                };
                for i in 1..n {
                    if let Some(pb) = s.peaks.get(&st.tools.mc_assets[i]) {
                        offsets[i] = correlate(&pa, pb, 30.0);
                    }
                }
            } else if method == 1 {
                // timecode tags in the files
                let Some(s) = &st.session else { return };
                let tc: Vec<Option<f64>> = st
                    .tools
                    .mc_assets
                    .iter()
                    .map(|id| {
                        let a = s.project().asset(*id)?;
                        let info = me::probe(&a.path).ok()?;
                        let t = info.metadata.get("timecode")?;
                        let tcv = tl::Timecode::parse(t)?;
                        let fps = a.video.first()?.frame_rate?;
                        tcv.to_time(fps).map(|t| t.as_secs_f64())
                    })
                    .collect();
                if let Some(Some(t0)) = tc.first() {
                    for i in 1..n {
                        if let Some(Some(t)) = tc.get(i) {
                            offsets[i] = t - t0;
                        }
                    }
                } else {
                    st.toast(ui, "These files carry no timecode. Try audio sync.");
                    return;
                }
            }
            st.tools.mc_offsets = offsets;
            m.set_mc_synced(vec_model(vec![true; n]));
            mc_thumbs(st, ui, true);
            st.toast(ui, "Angles synced.");
        });
    });
    b.on_mc_select_angle(|i| {
        with(|st, ui| {
            ui.global::<Model>().set_mc_current(i);
            mc_thumbs(st, ui, true);
        });
    });
    b.on_mc_cut(|i| {
        with(|st, ui| {
            let i = i.max(0) as usize;
            let Some(asset) = st.tools.mc_assets.get(i).copied() else { return };
            let off = st.tools.mc_offsets.get(i).copied().unwrap_or(0.0);
            let switch_audio = ui.global::<Model>().get_mc_switch_audio();
            st.edit(ui, |s| {
                let seq = s.seq;
                let v1 = s.first_video_track();
                let ph = s.playhead_time();
                let track = s.sequence().track(v1).unwrap();
                // end of this angle segment: next cut on V1 or end of timeline
                let end = track.clip_from(ph + s.sequence().frame_duration()).map(|c| c.start).unwrap_or_else(|| s.sequence().duration().max(ph + Time::from_secs_f64(5.0)));
                let end = match track.clip_at(ph) {
                    Some(c) => c.end(),
                    None => end,
                };
                let dur = end - ph;
                if dur <= Time::ZERO {
                    return Err("Move the playhead into the program first.".into());
                }
                let src = (ph.as_secs_f64() - off).max(0.0);
                let at = if switch_audio { s.first_audio_track() } else { None };
                s.editor
                    .place_asset(seq, asset, ph, Time::from_secs_f64(src), Some(dur), Some(v1), at, PlaceMode::Overwrite)
                    .map_err(|e| e.to_string())?;
                s.bump();
                Ok(())
            });
            ui.global::<Model>().set_mc_current(i as i32);
        });
    });
    b.on_mc_flatten(|| {
        with(|st, ui| {
            let Some(asset) = st.tools.mc_assets.first().copied() else {
                st.toast(ui, "Import camera angles first.");
                return;
            };
            st.edit(ui, |s| {
                let seq = s.seq;
                let v1 = s.first_video_track();
                if s.sequence().track(v1).is_some_and(|t| !t.is_empty()) {
                    return Err("The program is already on the timeline. Cut between angles with 1 to 4.".into());
                }
                let at = s.first_audio_track();
                s.editor.place_asset(seq, asset, Time::ZERO, Time::ZERO, None, Some(v1), at, PlaceMode::Overwrite).map_err(|e| e.to_string())?;
                s.bump();
                Ok(())
            });
        });
    });

    // speed
    b.on_set_speed(|pct, rev| {
        with(|st, ui| {
            if let Some(id) = st.selected_clip(ui) {
                st.edit(ui, |s| s.set_speed(id, pct as f64, rev));
            }
        });
    });
    b.on_apply_speed_preset(|i| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            ui.global::<Model>().set_speed_preset(i);
            let ramp = match i {
                0 => [100.0, 400.0, 400.0, 25.0],
                1 => [50.0, 50.0, 50.0, 50.0],
                2 => [200.0, 200.0, 200.0, 200.0],
                3 => [25.0, 60.0, 100.0, 100.0],
                4 => [100.0, 100.0, 60.0, 25.0],
                _ => [100.0, 100.0, 100.0, 100.0],
            };
            match i {
                5 => {
                    // freeze frame: hold the picture at the playhead for two seconds
                    st.edit(ui, |s| {
                        let seq = s.seq;
                        let ph = s.playhead_time();
                        let (_, c) = s.find_clip(id).ok_or("No clip")?;
                        if !(c.start < ph && ph < c.end()) {
                            return Err("Put the playhead inside the clip.".into());
                        }
                        let (_, right) = s.editor.split_clip(seq, id, ph, tl::LinkMode::Ignore).map_err(|e| e.to_string())?;
                        let hold_end = ph + Time::from_secs_f64(2.0);
                        let rc = s.find_clip(right).map(|(_, c)| c.end()).unwrap_or(hold_end);
                        let mid = if hold_end < rc { s.editor.split_clip(seq, right, hold_end, tl::LinkMode::Ignore).map(|p| p.0).map_err(|e| e.to_string())? } else { right };
                        let sp = tl::Speed::from_f64(tl::MIN_SPEED).ok_or("speed")?;
                        s.editor.set_speed(seq, mid, sp, tl::SpeedMode::KeepLength, tl::Ripple::Off, tl::LinkMode::Ignore).map_err(|e| e.to_string())?;
                        s.selected = Some(mid);
                        s.bump();
                        Ok(())
                    });
                }
                6 => {
                    let rev = !st.session.as_ref().and_then(|s| s.find_clip(id)).map(|(_, c)| c.reverse).unwrap_or(false);
                    let sp = st.session.as_ref().and_then(|s| s.find_clip(id)).map(|(_, c)| c.speed.as_f64() * 100.0).unwrap_or(100.0);
                    st.edit(ui, |s| s.set_speed(id, sp, rev));
                }
                1 | 2 => {
                    st.edit(ui, |s| {
                        s.clear_animation(id, Param::Speed)?;
                        s.set_speed(id, ramp[0], false)
                    });
                    ui.global::<Model>().set_speed_ramp(vec_model(ramp.iter().map(|v| *v as f32).collect()));
                }
                _ => {
                    ui.global::<Model>().set_speed_ramp(vec_model(ramp.iter().map(|v| *v as f32).collect()));
                }
            }
        });
    });
    b.on_apply_speed_ramp(|| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            let ramp: Vec<f32> = ui.global::<Model>().get_speed_ramp().iter().collect();
            st.edit(ui, |s| {
                let (_, c) = s.find_clip(id).ok_or("No clip")?;
                let d = c.duration.as_secs_f64();
                s.clear_animation(id, Param::Speed)?;
                for (k, v) in ramp.iter().enumerate() {
                    let t = d * [0.0, 0.33, 0.66, 0.999][k.min(3)];
                    s.set_keyframe(id, Param::Speed, t, (*v as f64 / 100.0).clamp(tl::MIN_SPEED, tl::MAX_SPEED))?;
                }
                Ok(())
            });
        });
    });
    b.on_freeze_frame(|| {
        let _ = with(|_, ui| ui.global::<Backend>().invoke_apply_speed_preset(5));
    });

    // transitions
    b.on_apply_transition(|all| {
        with(|st, ui| {
            let m = ui.global::<Model>();
            let (k, secs, align) = (m.get_transition_pick(), m.get_transition_secs() as f64, m.get_transition_align());
            if all {
                let mut ok = 0;
                let mut fail = 0;
                if let Some(s) = st.session.as_mut() {
                    let ids: Vec<ClipId> = s.sequence().video_tracks().iter().flat_map(|t| t.clips().map(|c| c.id).collect::<Vec<_>>()).collect();
                    for id in ids {
                        if s.prev_clip(id).is_some() {
                            match s.set_transition(id, Some(k), secs, align) {
                                Ok(()) => ok += 1,
                                Err(_) => fail += 1,
                            }
                        }
                    }
                }
                st.sync(ui);
                if ok == 0 {
                    st.toast(ui, "No cuts where two clips touch with spare media.");
                } else if fail > 0 {
                    st.toast(ui, format!("Added to {ok} cuts. {fail} cuts need more spare media."));
                } else {
                    st.toast(ui, format!("Added to {ok} cuts."));
                }
            } else {
                let Some(id) = st.selected_clip(ui) else { return };
                let has_prev = st.session.as_ref().and_then(|s| s.prev_clip(id)).is_some();
                if !has_prev {
                    st.toast(ui, "Pick a clip that starts right where another ends.");
                    return;
                }
                st.edit(ui, |s| s.set_transition(id, Some(k), secs, align));
            }
        });
    });
    b.on_remove_transition(|| {
        with(|st, ui| {
            if let Some(id) = st.selected_clip(ui) {
                st.edit(ui, |s| s.set_transition(id, None, 1.0, 1));
            }
        });
    });

    // mixer
    b.on_reset_levels(|| {
        with(|st, ui| {
            st.edit(ui, |s| {
                let ids: Vec<tl::TrackId> = s.sequence().audio_tracks().iter().map(|t| t.id).collect();
                for t in ids {
                    s.update_track(t, tl::TrackUpdate { gain: Some(tl::Gain::UNITY), pan: Some(tl::Pan::CENTRE), ..Default::default() })?;
                }
                s.set_master_gain(0.0)
            });
        });
    });
    b.on_auto_balance(|| {
        with(|st, ui| {
            st.edit(ui, |s| {
                let tracks: Vec<tl::Track> = s.sequence().audio_tracks().iter().cloned().collect();
                let mut n = 0;
                for t in &tracks {
                    if let Some(rms) = track_rms(s, t) {
                        if rms > 1e-4 {
                            // bring every track's average level to about -20 dBFS
                            let cur = 20.0 * (rms as f64).log10();
                            let want = -20.0 - cur + t.gain.db() as f64;
                            let g = session::gain_db(want.clamp(-30.0, 15.0)).ok_or("level")?;
                            s.update_track(t.id, tl::TrackUpdate { gain: Some(g), ..Default::default() })?;
                            n += 1;
                        }
                    }
                }
                if n == 0 {
                    return Err("No sound to balance yet.".into());
                }
                Ok(())
            });
        });
    });

    // audio cleanup
    b.on_cleanup_select_clip(|i| {
        with(|st, ui| {
            ui.global::<Model>().set_cleanup_clip(i);
            cleanup_refresh(st, ui);
            st.sync(ui);
        });
    });
    b.on_normalize_loudness(|| {
        with(|st, ui| {
            let m = ui.global::<Model>();
            let target = match m.get_loudness_target() {
                1 => -16.0,
                2 => -23.0,
                _ => -14.0,
            };
            let clips = audio_clips(st);
            let Some((id, _)) = clips.get(m.get_cleanup_clip().max(0) as usize).cloned() else {
                st.toast(ui, "No audio clip to clean up.");
                return;
            };
            let Some(s) = &st.session else { return };
            let span = match clip_span(s, id) {
                Ok(v) => v,
                Err(e) => {
                    st.toast(ui, e);
                    return;
                }
            };
            m.set_busy(true);
            m.set_busy_text("Measuring loudness".into());
            m.set_busy_progress(-1.0);
            thread::spawn(move || {
                let r = measure_clip(&span.0, span.1, span.2);
                later(move |st, ui| {
                    ui.global::<Model>().set_busy(false);
                    match r {
                        Ok(lufs) => {
                            let cur = st.session.as_ref().and_then(|s| s.find_clip(id)).map(|(_, c)| c.gain.db() as f64).unwrap_or(0.0);
                            let new = (cur + target - lufs).clamp(-40.0, 24.0);
                            st.edit(ui, |s| {
                                let g = session::gain_db(new).ok_or("level")?;
                                s.set_clip_audio(id, tl::ClipAudio { gain: Some(g), ..Default::default() })
                            });
                            cleanup_refresh(st, ui);
                            ui.global::<Model>().set_loudness_text(format!("Measured {lufs:.1} LUFS · now {target:.0} LUFS ({new:+.1} dB)").into());
                        }
                        Err(e) => st.toast(ui, e),
                    }
                });
            });
        });
    });
    b.on_cleanup_reset(|| {
        with(|st, ui| {
            let m = ui.global::<Model>();
            let clips = audio_clips(st);
            if let Some((id, _)) = clips.get(m.get_cleanup_clip().max(0) as usize).cloned() {
                st.edit(ui, |s| s.set_clip_audio(id, tl::ClipAudio { gain: Some(tl::Gain::UNITY), ..Default::default() }));
                cleanup_refresh(st, ui);
            }
        });
    });

    // colour
    b.on_colour_select_clip(|i| {
        with(|st, ui| {
            let clips = video_clips(st);
            if let Some((id, ..)) = clips.get(i.max(0) as usize) {
                if let Some(s) = st.session.as_mut() {
                    s.selected = Some(*id);
                    let start = s.find_clip(*id).map(|(_, c)| c.start.as_secs_f64()).unwrap_or(0.0);
                    s.playhead = start + 0.04;
                }
                ui.global::<Model>().set_colour_clip(i);
                read_grading(st, ui);
                st.tools.thumbs_for = None;
                st.sync(ui);
            }
        });
    });
    b.on_apply_look(|i| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            ui.global::<Model>().set_look_preset(i);
            st.edit(ui, |s| {
                for (k, v) in look_values(i) {
                    s.set_colour_effect(id, k, v)?;
                }
                Ok(())
            });
        });
    });
    b.on_set_colour(|name, v| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            let (kind, value) = match name.as_str() {
                "temperature" => (EffectKind::Temperature, v as f64),
                "tint" => (EffectKind::Hue, v as f64),
                "exposure" => (EffectKind::Exposure, v as f64),
                "contrast" => (EffectKind::Contrast, v as f64),
                "saturation" => (EffectKind::Saturation, v as f64),
                "highlights" => (EffectKind::Gamma, (1.0 - v as f64 / 100.0).clamp(0.1, 5.0)),
                _ => return,
            };
            st.edit(ui, |s| s.set_colour_effect(id, kind, value));
        });
    });
    b.on_reset_colour(|| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            st.edit(ui, |s| {
                let colour = [
                    EffectKind::Temperature,
                    EffectKind::Hue,
                    EffectKind::Exposure,
                    EffectKind::Contrast,
                    EffectKind::Saturation,
                    EffectKind::Gamma,
                    EffectKind::Grayscale,
                    EffectKind::Balance,
                    EffectKind::Curve,
                ];
                let ids: Vec<EffectId> = s.find_clip(id).map(|(_, c)| c.effects.iter().filter(|e| colour.contains(&e.kind)).map(|e| e.id).collect()).unwrap_or_default();
                for f in ids {
                    s.remove_effect(id, f)?;
                }
                Ok(())
            });
            ui.global::<Model>().set_look_preset(0);
            read_grading(st, ui);
        });
    });
    b.on_set_wheel(|wheel, ch, v| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            // ch 0..2 = r,g,b ; 3 = master (moves all three)
            let wheel = wheel.clamp(0, 3) as usize;
            st.edit(ui, |s| {
                if ch == 3 {
                    let cur: Vec<f64> = (0..3)
                        .map(|c| s.find_clip(id).and_then(|(_, cl)| cl.effects.iter().find(|e| e.kind == EffectKind::Balance).map(|e| e.value(wheel * 3 + c))).unwrap_or(0.0))
                        .collect();
                    let avg = cur.iter().sum::<f64>() / 3.0;
                    for (c, cv) in cur.iter().enumerate() {
                        set_multi_effect(s, id, EffectKind::Balance, wheel * 3 + c, (cv - avg + v as f64).clamp(-1.0, 1.0))?;
                    }
                    Ok(())
                } else {
                    set_multi_effect(s, id, EffectKind::Balance, wheel * 3 + ch.clamp(0, 2) as usize, (v as f64).clamp(-1.0, 1.0))
                }
            });
            read_grading(st, ui);
        });
    });
    b.on_set_curve_point(|i, _x, y| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            st.edit(ui, |s| set_multi_effect(s, id, EffectKind::Curve, i.clamp(0, 2) as usize, (y as f64).clamp(0.0, 1.0)));
            read_grading(st, ui);
        });
    });
    b.on_reset_curve(|| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            st.edit(ui, |s| {
                let f = s.find_clip(id).and_then(|(_, c)| c.effects.iter().find(|e| e.kind == EffectKind::Curve).map(|e| e.id));
                match f {
                    Some(f) => s.remove_effect(id, f),
                    None => Ok(()),
                }
            });
            read_grading(st, ui);
        });
    });
    b.on_refresh_scopes(|| {
        with(|st, ui| {
            let m = ui.global::<Model>();
            let clips = video_clips(st);
            let (ri, ti) = (m.get_scopes_ref().max(0) as usize, m.get_scopes_target().max(0) as usize);
            let (Some(r), Some(t)) = (clips.get(ri).cloned(), clips.get(ti).cloned()) else {
                st.toast(ui, "Put two video clips on the timeline to match them.");
                return;
            };
            let Some(s) = &st.session else { return };
            let (Some(ra), Some(ta)) = (s.project().asset(r.2).cloned(), s.project().asset(t.2).cloned()) else { return };
            let (rid, tid) = (r.0, t.0);
            let mode = m.get_grading_tab(); // reused: 0 colour, 1 exposure, 2 both
            m.set_busy(true);
            m.set_busy_text("Matching shots".into());
            thread::spawn(move || {
                let rf = crate::media::thumbnail(&ra.path, r.3, 160);
                let tf = crate::media::thumbnail(&ta.path, t.3, 160);
                later(move |st, ui| {
                    ui.global::<Model>().set_busy(false);
                    let (Some(rf), Some(tf)) = (rf, tf) else {
                        st.toast(ui, "Could not read those clips.");
                        return;
                    };
                    let rs = stats(&me::VideoFrame { width: rf.0, height: rf.1, pts_secs: 0.0, data: rf.2 });
                    let ts = stats(&me::VideoFrame { width: tf.0, height: tf.1, pts_secs: 0.0, data: tf.2 });
                    let luma = |s: &[f64; 6]| 0.2126 * s[0] + 0.7152 * s[1] + 0.0722 * s[2];
                    let lstd = |s: &[f64; 6]| 0.2126 * s[3] + 0.7152 * s[4] + 0.0722 * s[5];
                    let _ = rid;
                    st.edit(ui, |s| {
                        if mode != 0 {
                            let exp = ((luma(&rs).max(1.0) / luma(&ts).max(1.0)).log2()).clamp(-3.0, 3.0);
                            s.set_colour_effect(tid, EffectKind::Exposure, (exp * 100.0).round() / 100.0)?;
                            let con = ((lstd(&rs) / lstd(&ts).max(1.0) - 1.0) * 100.0).clamp(-60.0, 60.0);
                            s.set_colour_effect(tid, EffectKind::Contrast, con.round())?;
                        }
                        if mode != 1 {
                            // warm/cool from the red-blue balance, then overall saturation
                            let rb_ref = (rs[0] - rs[2]) / luma(&rs).max(1.0);
                            let rb_tgt = (ts[0] - ts[2]) / luma(&ts).max(1.0);
                            let temp = ((rb_ref - rb_tgt) * 150.0).clamp(-100.0, 100.0);
                            s.set_colour_effect(tid, EffectKind::Temperature, temp.round())?;
                            let chroma = |s: &[f64; 6]| ((s[0] - s[1]).abs() + (s[1] - s[2]).abs() + (s[2] - s[0]).abs()) / luma(s).max(1.0);
                            let sat = (100.0 * chroma(&rs).max(0.01) / chroma(&ts).max(0.01)).clamp(30.0, 250.0);
                            s.set_colour_effect(tid, EffectKind::Saturation, sat.round())?;
                        }
                        Ok(())
                    });
                    st.toast(ui, "Matched to the reference.");
                });
            });
        });
    });

    // effects
    b.on_add_effect(|i| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            let kind = if (0..EffectKind::ALL.len() as i32).contains(&i) { EffectKind::ALL[i as usize] } else { return };
            let is_audio = st.session.as_ref().and_then(|s| s.find_clip(id)).is_some_and(|(t, _)| t.kind == TrackKind::Audio);
            if is_audio {
                st.toast(ui, "Effects work on pictures. Select a video clip.");
                return;
            }
            st.edit(ui, |s| s.add_effect(id, kind).map(|_| ()));
            let n = st.session.as_ref().map(|s| s.selected_item().effects.row_count()).unwrap_or(0);
            ui.global::<Model>().set_effect_selected(n as i32 - 1);
        });
    });
    b.on_remove_effect(|fid| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            st.edit(ui, |s| s.remove_effect(id, EffectId(fid as u32)));
            ui.global::<Model>().set_effect_selected(-1);
        });
    });
    b.on_toggle_effect(|fid, on| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            st.edit(ui, |s| s.set_effect_enabled(id, EffectId(fid as u32), on));
        });
    });
    b.on_set_effect_param(|fid, name, v| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            st.edit(ui, |s| s.set_effect_value(id, EffectId(fid as u32), name.as_str(), v as f64));
        });
    });
    b.on_move_effect(|fid, to| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            st.edit(ui, |s| s.move_effect(id, EffectId(fid as u32), to.max(0) as usize));
            ui.global::<Model>().set_effect_selected(to);
        });
    });
    b.on_copy_effects_to_clips(|| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            st.edit(ui, |s| {
                let (track, clip) = s.find_clip(id).ok_or("No clip")?;
                let effects: Vec<(EffectKind, Vec<(String, f64)>, bool)> = clip
                    .effects
                    .iter()
                    .filter(|e| e.plugin.is_none())
                    .map(|e| (e.kind, e.settings().iter().enumerate().map(|(i, p)| (p.name.clone(), e.value(i))).collect(), e.enabled))
                    .collect();
                let others: Vec<ClipId> = track.clips().filter(|c| c.id != id).map(|c| c.id).collect();
                if others.is_empty() {
                    return Err("No other clips on this track.".into());
                }
                let seq = s.seq;
                s.editor
                    .group("Copy effects", |ed| {
                        for o in &others {
                            for (k, vals, on) in &effects {
                                let f = ed.add_effect(seq, *o, *k, None)?;
                                for (n, v) in vals {
                                    ed.set_effect_value(seq, *o, f, n, *v)?;
                                }
                                if !on {
                                    ed.set_effect_enabled(seq, *o, f, false)?;
                                }
                            }
                        }
                        Ok(())
                    })
                    .map_err(|e| e.to_string())?;
                s.bump();
                Ok(())
            });
            st.toast(ui, "Effects copied to every clip on this track.");
        });
    });

    // titles
    b.on_title_preview(|| {
        with(|st, ui| title_preview(st, ui));
    });
    b.on_title_add(|| {
        with(|st, ui| {
            add_title(st, ui);
            st.go(ui, Screen::Editor);
        });
    });
    b.on_title_reset(|| {
        with(|st, ui| {
            let m = ui.global::<Model>();
            m.set_title_text("Lorem ipsum".into());
            m.set_title_sub("Dolor sit amet".into());
            m.set_title_size(26.0);
            m.set_title_colour(0);
            m.set_title_in(0);
            m.set_title_out(1);
            m.set_title_anim_secs(0.6);
            title_preview(st, ui);
        });
    });
    b.on_update_selected_text(|| {
        with(|st, ui| {
            add_title(st, ui);
            title_preview(st, ui);
        });
    });
    b.on_add_shape(|kind| {
        with(|st, ui| {
            let shape = match kind {
                0 => mg::ShapeKind::Rect { corner_pct: 0.0 },
                1 => mg::ShapeKind::Ellipse,
                _ => mg::ShapeKind::Star { points: 5, inner_pct: 45.0 },
            };
            let g = mg::Generator::Shape(mg::Shape { kind: shape, width_pct: 30.0, height_pct: 30.0, fill: mg::Fill::Solid(mg::Colour { r: 46, g: 140, b: 255, a: 200 }), stroke: None });
            st.edit(ui, |s| {
                let aid = s.editor.add_generated(["Rectangle", "Ellipse", "Star"][kind.clamp(0, 2) as usize], g).map_err(|e| e.to_string())?;
                let at = s.playhead_time();
                let track = s.editor.add_track(s.seq, TrackKind::Video, Some("Shapes".into())).map_err(|e| e.to_string())?;
                let mut spec = tl::ClipSpec::new(aid, at);
                spec.duration = Some(Time::from_secs_f64(5.0));
                let seq = s.seq;
                let c = s.editor.place_clip(seq, track, spec, PlaceMode::Overwrite).map_err(|e| e.to_string())?;
                s.selected = Some(c);
                s.bump();
                Ok(())
            });
        });
    });

    // keyframes & transform
    b.on_set_keyframe(|p, t, v| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            let param = KF_PARAMS[p.clamp(0, 3) as usize];
            st.edit(ui, |s| {
                let start = s.find_clip(id).map(|(_, c)| c.start.as_secs_f64()).unwrap_or(0.0);
                let local = (t as f64 - start).max(0.0);
                s.set_keyframe(id, param, local, v as f64)?;
                if param == Param::ScaleX {
                    s.set_keyframe(id, Param::ScaleY, local, v as f64)?;
                }
                Ok(())
            });
        });
    });
    b.on_remove_keyframe(|p, t| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            let param = KF_PARAMS[p.clamp(0, 3) as usize];
            st.edit(ui, |s| {
                s.remove_keyframe(id, param, t as f64)?;
                if param == Param::ScaleX {
                    let _ = s.remove_keyframe(id, Param::ScaleY, t as f64);
                }
                Ok(())
            });
        });
    });
    b.on_clear_keyframes(|p| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            let param = KF_PARAMS[p.clamp(0, 3) as usize];
            st.edit(ui, |s| {
                s.clear_animation(id, param)?;
                if param == Param::ScaleX {
                    s.clear_animation(id, Param::ScaleY)?;
                }
                if param == Param::PositionX {
                    s.clear_animation(id, Param::PositionY)?;
                }
                Ok(())
            });
        });
    });
    b.on_set_transform(|name, v| {
        with(|st, ui| {
            let Some(id) = st.selected_clip(ui) else { return };
            let v = v as f64;
            st.edit(ui, |s| {
                let animated = |s: &Session, p: Param| s.find_clip(id).is_some_and(|(_, c)| c.anim.is_animated(p));
                let start = s.find_clip(id).map(|(_, c)| c.start.as_secs_f64()).unwrap_or(0.0);
                let local = (s.playhead - start).max(0.0);
                let set = |s: &mut Session, p: Param| -> Result<(), String> {
                    if animated(s, p) {
                        s.set_keyframe(id, p, local, v)
                    } else {
                        s.set_look(id, p, v)
                    }
                };
                match name.as_str() {
                    "x" => set(s, Param::PositionX),
                    "y" => set(s, Param::PositionY),
                    "scale" => {
                        set(s, Param::ScaleX)?;
                        set(s, Param::ScaleY)
                    }
                    "rotation" => set(s, Param::Rotation),
                    "opacity" => set(s, Param::Opacity),
                    _ => Ok(()),
                }
            });
        });
    });
    b.on_set_blend(|i| {
        with(|st, ui| {
            if let Some(id) = st.selected_clip(ui) {
                st.edit(ui, |s| s.set_blend(id, i));
            }
        });
    });

    // masks
    b.on_apply_mask(|shape, invert, feather, x, y, w, h| {
        with(|st, ui| {
            if let Some(id) = st.selected_clip(ui) {
                st.edit(ui, |s| s.set_mask(id, shape, invert, feather as f64, x as f64, y as f64, w as f64, h as f64));
            }
        });
    });
    b.on_remove_mask(|| {
        with(|st, ui| {
            if let Some(id) = st.selected_clip(ui) {
                st.edit(ui, |s| s.remove_mask(id));
            }
        });
    });
    b.on_select_layer(|i| {
        with(|st, ui| {
            let ids = ui.global::<Model>().get_layer_ids();
            if let Some(cid) = ids.row_data(i.max(0) as usize) {
                if let Some(s) = st.session.as_mut() {
                    s.selected = Some(ClipId(cid as u64));
                }
                ui.global::<Model>().set_layer_current(i);
                st.sync(ui);
            }
        });
    });
}

pub fn _unused(_: SharedString, _: Arc<()>) {}
