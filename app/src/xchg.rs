// SPDX-License-Identifier: GPL-3.0-or-later
//! Project interchange: Final Cut Pro XML, CMX 3600 EDL and OpenTimelineIO, in and out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use timeline_engine as tl;
use tl::{AssetId, Editor, PlaceMode, SequenceSettings, Time, TrackKind};

pub struct Options {
    pub media_refs: bool,
    pub markers: bool,
}

struct Ev {
    track: (TrackKind, usize),
    name: String,
    path: PathBuf,
    start: f64,
    src_in: f64,
    dur: f64,
}

fn events(project: &tl::Project, seq: &tl::Sequence) -> Vec<Ev> {
    let mut v = Vec::new();
    for (kind, tracks) in [(TrackKind::Video, seq.video_tracks()), (TrackKind::Audio, seq.audio_tracks())] {
        for (i, t) in tracks.iter().enumerate() {
            for c in t.clips() {
                let a = project.asset(c.asset);
                if a.is_some_and(|a| a.is_generated()) {
                    continue;
                }
                v.push(Ev {
                    track: (kind, i),
                    name: c.name.clone(),
                    path: a.map(|a| a.path.clone()).unwrap_or_default(),
                    start: c.start.as_secs_f64(),
                    src_in: c.source_in.as_secs_f64(),
                    dur: c.duration.as_secs_f64(),
                });
            }
        }
    }
    v
}

fn tc(secs: f64, fps: f64) -> String {
    crate::util::timecode(secs, fps, false)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.replace(' ', "%20");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

fn from_url(u: &str) -> PathBuf {
    let s = u.trim_start_matches("file://").replace("%20", " ");
    #[cfg(windows)]
    let s = s.trim_start_matches('/').to_string();
    PathBuf::from(s)
}

pub fn export(project: &tl::Project, seq: &tl::Sequence, path: &Path, format: i32, opt: &Options) -> Result<(), String> {
    let fps = seq.frame_rate.as_f64();
    let evs = events(project, seq);
    let text = match format {
        1 => {
            // CMX 3600: one event per clip on the first video and first audio track.
            let mut s = format!("TITLE: {}\nFCM: NON-DROP FRAME\n\n", project.name);
            let mut n = 1;
            for e in evs.iter().filter(|e| e.track.1 == 0) {
                let chan = if e.track.0 == TrackKind::Video { "V    " } else { "A    " };
                s += &format!(
                    "{:03}  AX       {} C        {} {} {} {}\n",
                    n,
                    chan,
                    tc(e.src_in, fps),
                    tc(e.src_in + e.dur, fps),
                    tc(e.start, fps),
                    tc(e.start + e.dur, fps)
                );
                s += &format!("* FROM CLIP NAME: {}\n", e.path.file_name().unwrap_or_default().to_string_lossy());
                if opt.media_refs {
                    s += &format!("* SOURCE FILE: {}\n", e.path.display());
                }
                s += "\n";
                n += 1;
            }
            if opt.markers {
                for m in seq.markers().iter() {
                    s += &format!("* MARKER {} {}\n", tc(m.time.as_secs_f64(), fps), m.name);
                }
            }
            s
        }
        2 => {
            // OpenTimelineIO JSON
            let rt = |v: f64| serde_json::json!({"OTIO_SCHEMA": "RationalTime.1", "value": (v * fps).round(), "rate": fps});
            let mut tracks = Vec::new();
            for (kind, list) in [(TrackKind::Video, seq.video_tracks()), (TrackKind::Audio, seq.audio_tracks())] {
                for (i, t) in list.iter().enumerate() {
                    let mut children = Vec::new();
                    let mut cursor = 0.0;
                    for e in evs.iter().filter(|e| e.track == (kind, i)) {
                        if e.start > cursor + 1e-6 {
                            children.push(serde_json::json!({"OTIO_SCHEMA": "Gap.1", "name": "", "source_range": {"OTIO_SCHEMA": "TimeRange.1", "start_time": rt(0.0), "duration": rt(e.start - cursor)}}));
                        }
                        let media = if opt.media_refs {
                            serde_json::json!({"OTIO_SCHEMA": "ExternalReference.1", "target_url": url(&e.path)})
                        } else {
                            serde_json::json!({"OTIO_SCHEMA": "MissingReference.1"})
                        };
                        children.push(serde_json::json!({
                            "OTIO_SCHEMA": "Clip.1", "name": e.name,
                            "source_range": {"OTIO_SCHEMA": "TimeRange.1", "start_time": rt(e.src_in), "duration": rt(e.dur)},
                            "media_reference": media
                        }));
                        cursor = e.start + e.dur;
                    }
                    tracks.push(serde_json::json!({"OTIO_SCHEMA": "Track.1", "name": t.name, "kind": if kind == TrackKind::Video {"Video"} else {"Audio"}, "children": children}));
                }
            }
            let markers: Vec<serde_json::Value> = if opt.markers {
                seq.markers()
                    .iter()
                    .map(|m| serde_json::json!({"OTIO_SCHEMA": "Marker.2", "name": m.name, "marked_range": {"OTIO_SCHEMA": "TimeRange.1", "start_time": rt(m.time.as_secs_f64()), "duration": rt(0.0)}, "color": "YELLOW"}))
                    .collect()
            } else {
                vec![]
            };
            let doc = serde_json::json!({
                "OTIO_SCHEMA": "Timeline.1", "name": project.name,
                "tracks": {"OTIO_SCHEMA": "Stack.1", "name": "tracks", "children": tracks, "markers": markers}
            });
            serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?
        }
        _ => {
            // FCPXML 1.9
            let den = (fps.round() as i64).max(1) * 100;
            let r = |v: f64| format!("{}/{}s", (v * den as f64).round() as i64, den);
            let mut assets: HashMap<PathBuf, (String, f64)> = HashMap::new();
            for a in project.assets().values() {
                if !a.is_generated() {
                    let id = format!("r{}", assets.len() + 2);
                    assets.insert(a.path.clone(), (id, a.duration.map(|d| d.as_secs_f64()).unwrap_or(3600.0)));
                }
            }
            let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE fcpxml>\n<fcpxml version=\"1.9\">\n  <resources>\n");
            s += &format!(
                "    <format id=\"r1\" frameDuration=\"{}/{}s\" width=\"{}\" height=\"{}\"/>\n",
                (den as f64 / fps).round() as i64,
                den,
                seq.width,
                seq.height
            );
            for (p, (id, d)) in &assets {
                s += &format!(
                    "    <asset id=\"{}\" name=\"{}\" start=\"0s\" duration=\"{}\" hasVideo=\"1\" hasAudio=\"1\" format=\"r1\">\n      <media-rep kind=\"original-media\" src=\"{}\"/>\n    </asset>\n",
                    id,
                    esc(&p.file_name().unwrap_or_default().to_string_lossy()),
                    r(*d),
                    esc(&url(p))
                );
            }
            s += "  </resources>\n  <library>\n    <event name=\"OriginFX\">\n";
            s += &format!(
                "      <project name=\"{}\">\n        <sequence format=\"r1\" duration=\"{}\" tcStart=\"0s\">\n          <spine>\n",
                esc(&project.name),
                r(seq.duration().as_secs_f64())
            );
            let mut cursor = 0.0;
            let lane_clips: Vec<&Ev> = evs.iter().filter(|e| e.track.1 > 0 || e.track.0 == TrackKind::Audio).collect();
            for e in evs.iter().filter(|e| e.track == (TrackKind::Video, 0)) {
                if e.start > cursor + 1e-6 {
                    s += &format!("            <gap name=\"Gap\" offset=\"{}\" duration=\"{}\"/>\n", r(cursor), r(e.start - cursor));
                }
                let id = assets.get(&e.path).map(|a| a.0.clone()).unwrap_or_else(|| "r2".into());
                s += &format!("            <asset-clip ref=\"{}\" name=\"{}\" offset=\"{}\" start=\"{}\" duration=\"{}\">\n", id, esc(&e.name), r(e.start), r(e.src_in), r(e.dur));
                // connected clips that start within this clip
                for c in lane_clips.iter().filter(|c| c.start >= e.start && c.start < e.start + e.dur) {
                    let lane = if c.track.0 == TrackKind::Video { c.track.1 as i32 } else { -(c.track.1 as i32) - 1 };
                    let cid = assets.get(&c.path).map(|a| a.0.clone()).unwrap_or_else(|| "r2".into());
                    s += &format!(
                        "              <asset-clip ref=\"{}\" lane=\"{}\" name=\"{}\" offset=\"{}\" start=\"{}\" duration=\"{}\"/>\n",
                        cid,
                        lane,
                        esc(&c.name),
                        r(e.src_in + (c.start - e.start)),
                        r(c.src_in),
                        r(c.dur)
                    );
                }
                if opt.markers {
                    for m in seq.markers().iter().filter(|m| m.time.as_secs_f64() >= e.start && m.time.as_secs_f64() < e.start + e.dur) {
                        s += &format!("              <marker start=\"{}\" duration=\"0s\" value=\"{}\"/>\n", r(e.src_in + m.time.as_secs_f64() - e.start), esc(&m.name));
                    }
                }
                s += "            </asset-clip>\n";
                cursor = e.start + e.dur;
            }
            s += "          </spine>\n        </sequence>\n      </project>\n    </event>\n  </library>\n</fcpxml>\n";
            s
        }
    };
    std::fs::write(path, text).map_err(|e| e.to_string())
}

/// What a timeline file describes: clips by file, on lanes.
struct Imported {
    name: String,
    fps: f64,
    size: (u32, u32),
    clips: Vec<(TrackKind, usize, PathBuf, String, f64, f64, f64)>, // kind, lane, path, name, start, src_in, dur
    markers: Vec<(f64, String)>,
}

fn parse_rational(s: &str) -> f64 {
    let s = s.trim_end_matches('s');
    if let Some((a, b)) = s.split_once('/') {
        a.parse::<f64>().unwrap_or(0.0) / b.parse::<f64>().unwrap_or(1.0).max(1e-9)
    } else {
        s.parse().unwrap_or(0.0)
    }
}

fn parse_tc(s: &str, fps: f64) -> f64 {
    let p: Vec<f64> = s.split([':', ';']).map(|x| x.parse().unwrap_or(0.0)).collect();
    if p.len() == 4 {
        p[0] * 3600.0 + p[1] * 60.0 + p[2] + p[3] / fps
    } else {
        0.0
    }
}

fn read(path: &Path, format: i32) -> Result<Imported, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    match format {
        1 => {
            let fps = 30.0;
            let mut clips: Vec<(TrackKind, usize, PathBuf, String, f64, f64, f64)> = Vec::new();
            let mut markers = Vec::new();
            let mut last: Option<usize> = None;
            let mut title = stem;
            for line in text.lines() {
                let l = line.trim();
                if let Some(t) = l.strip_prefix("TITLE:") {
                    title = t.trim().to_string();
                } else if let Some(n) = l.strip_prefix("* FROM CLIP NAME:") {
                    if let Some(i) = last {
                        let name = n.trim().to_string();
                        clips[i] = (clips[i].0, clips[i].1, base.join(&name), name, clips[i].4, clips[i].5, clips[i].6);
                    }
                } else if let Some(n) = l.strip_prefix("* SOURCE FILE:") {
                    if let Some(i) = last {
                        let c: &mut (TrackKind, usize, PathBuf, String, f64, f64, f64) = &mut clips[i];
                        c.2 = PathBuf::from(n.trim());
                    }
                } else if let Some(m) = l.strip_prefix("* MARKER ") {
                    let (t, name) = m.split_once(' ').unwrap_or((m, ""));
                    markers.push((parse_tc(t, fps), name.to_string()));
                } else {
                    let f: Vec<&str> = l.split_whitespace().collect();
                    if f.len() >= 8 && f[0].chars().all(|c| c.is_ascii_digit()) {
                        let kind = if f[2].starts_with('A') { TrackKind::Audio } else { TrackKind::Video };
                        let n = f.len();
                        let (si, so, ri) = (parse_tc(f[n - 4], fps), parse_tc(f[n - 3], fps), parse_tc(f[n - 2], fps));
                        clips.push((kind, 0, PathBuf::new(), f[1].to_string(), ri, si, so - si));
                        last = Some(clips.len() - 1);
                    }
                }
            }
            Ok(Imported { name: title, fps, size: (1920, 1080), clips, markers })
        }
        2 => {
            let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let name = v["name"].as_str().unwrap_or(&stem).to_string();
            let mut fps = 30.0;
            let mut clips = Vec::new();
            let rt = |t: &serde_json::Value| -> (f64, f64) {
                let r = t["rate"].as_f64().unwrap_or(30.0);
                (t["value"].as_f64().unwrap_or(0.0) / r.max(1.0), r)
            };
            let mut vlane = 0;
            let mut alane = 0;
            for t in v["tracks"]["children"].as_array().cloned().unwrap_or_default() {
                let kind = if t["kind"].as_str() == Some("Audio") { TrackKind::Audio } else { TrackKind::Video };
                let lane = if kind == TrackKind::Video {
                    vlane += 1;
                    vlane - 1
                } else {
                    alane += 1;
                    alane - 1
                };
                let mut cursor = 0.0;
                for c in t["children"].as_array().cloned().unwrap_or_default() {
                    let (start, r) = rt(&c["source_range"]["start_time"]);
                    let (dur, _) = rt(&c["source_range"]["duration"]);
                    fps = r;
                    let schema = c["OTIO_SCHEMA"].as_str().unwrap_or("");
                    if schema.starts_with("Clip") {
                        let p = c["media_reference"]["target_url"].as_str().map(from_url).unwrap_or_default();
                        clips.push((kind, lane, p, c["name"].as_str().unwrap_or("").to_string(), cursor, start, dur));
                    }
                    cursor += dur;
                }
            }
            let markers = v["tracks"]["markers"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|m| (rt(&m["marked_range"]["start_time"]).0, m["name"].as_str().unwrap_or("").to_string()))
                .collect();
            Ok(Imported { name, fps, size: (1920, 1080), clips, markers })
        }
        _ => {
            use quick_xml::events::Event;
            let mut r = quick_xml::Reader::from_str(&text);
            let mut assets: HashMap<String, PathBuf> = HashMap::new();
            let mut cur_asset: Option<String> = None;
            let mut clips = Vec::new();
            let mut markers = Vec::new();
            let mut fps = 30.0;
            let mut size = (1920, 1080);
            let mut name = stem;
            // parent clip context: (offset, start) for connected clips
            let mut parents: Vec<(f64, f64)> = Vec::new();
            let mut in_spine = false;
            loop {
                let ev = r.read_event().map_err(|e| e.to_string())?;
                let (e, empty) = match &ev {
                    Event::Start(e) => (Some(e.clone()), false),
                    Event::Empty(e) => (Some(e.clone()), true),
                    Event::End(e) => {
                        let n = String::from_utf8_lossy(e.name().as_ref()).to_string();
                        if (n == "asset-clip" || n == "clip" || n == "video") && in_spine {
                            parents.pop();
                        }
                        if n == "spine" {
                            in_spine = false;
                        }
                        if n == "asset" {
                            cur_asset = None;
                        }
                        (None, false)
                    }
                    Event::Eof => break,
                    _ => (None, false),
                };
                let Some(e) = e else { continue };
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let attrs: HashMap<String, String> = e
                    .attributes()
                    .flatten()
                    .map(|a| (String::from_utf8_lossy(a.key.as_ref()).to_string(), a.unescape_value().map(|v| v.to_string()).unwrap_or_default()))
                    .collect();
                match tag.as_str() {
                    "format" => {
                        if let Some(fd) = attrs.get("frameDuration") {
                            let d = parse_rational(fd);
                            if d > 0.0 {
                                fps = (1.0 / d * 1000.0).round() / 1000.0;
                            }
                        }
                        if let (Some(w), Some(h)) = (attrs.get("width"), attrs.get("height")) {
                            size = (w.parse().unwrap_or(1920), h.parse().unwrap_or(1080));
                        }
                    }
                    "asset" => {
                        let id = attrs.get("id").cloned().unwrap_or_default();
                        if let Some(src) = attrs.get("src") {
                            assets.insert(id.clone(), from_url(src));
                        }
                        if !empty {
                            cur_asset = Some(id);
                        }
                    }
                    "media-rep" => {
                        if let (Some(id), Some(src)) = (&cur_asset, attrs.get("src")) {
                            assets.insert(id.clone(), from_url(src));
                        }
                    }
                    "project" => {
                        if let Some(n) = attrs.get("name") {
                            name = n.clone();
                        }
                    }
                    "spine" => in_spine = true,
                    "asset-clip" | "clip" | "video" if in_spine => {
                        let offset = attrs.get("offset").map(|s| parse_rational(s)).unwrap_or(0.0);
                        let start = attrs.get("start").map(|s| parse_rational(s)).unwrap_or(0.0);
                        let dur = attrs.get("duration").map(|s| parse_rational(s)).unwrap_or(0.0);
                        let lane: i32 = attrs.get("lane").and_then(|l| l.parse().ok()).unwrap_or(0);
                        let p = attrs.get("ref").and_then(|r| assets.get(r)).cloned().unwrap_or_default();
                        let tl_start = match parents.last() {
                            Some((po, ps)) if lane != 0 => po + (offset - ps),
                            _ => offset,
                        };
                        let (kind, l) = if lane < 0 { (TrackKind::Audio, (-lane - 1) as usize) } else { (TrackKind::Video, lane as usize) };
                        clips.push((kind, l, p.clone(), attrs.get("name").cloned().unwrap_or_default(), tl_start, start, dur));
                        if lane == 0 {
                            // the primary storyline clip carries its audio too
                            clips.push((TrackKind::Audio, 0, p, attrs.get("name").cloned().unwrap_or_default(), tl_start, start, dur));
                        }
                        if !empty {
                            parents.push((tl_start, start));
                        }
                    }
                    "marker" => {
                        if let Some((po, ps)) = parents.last() {
                            let st = attrs.get("start").map(|s| parse_rational(s)).unwrap_or(0.0);
                            markers.push((po + st - ps, attrs.get("value").cloned().unwrap_or_default()));
                        }
                    }
                    _ => {}
                }
            }
            Ok(Imported { name, fps, size, clips, markers })
        }
    }
}

/// Build a new project from a timeline file. Files that cannot be found come in offline,
/// ready to relink in the Media inspector.
pub fn import(path: &Path, format: i32, relink_dirs: &[PathBuf], markers: bool) -> Result<(Editor, tl::SequenceId, usize), String> {
    let imp = read(path, format)?;
    if imp.clips.is_empty() {
        return Err("No clips found in that file.".into());
    }
    let rate = tl::FrameRate::from_f64(imp.fps).unwrap_or(tl::FrameRate::FPS_30);
    let vlanes = imp.clips.iter().filter(|c| c.0 == TrackKind::Video).map(|c| c.1 + 1).max().unwrap_or(1).max(1);
    let alanes = imp.clips.iter().filter(|c| c.0 == TrackKind::Audio).map(|c| c.1 + 1).max().unwrap_or(1).max(1);
    let mut ed = Editor::new(&imp.name);
    let seq = ed
        .add_sequence(SequenceSettings {
            name: "Sequence 1".into(),
            frame_rate: rate,
            width: imp.size.0,
            height: imp.size.1,
            video_tracks: vlanes as u32,
            audio_tracks: alanes as u32,
            ..Default::default()
        })
        .map_err(|e| e.to_string())?;
    let mut by_path: HashMap<PathBuf, AssetId> = HashMap::new();
    let mut offline = 0;
    let find = |p: &Path| -> Option<PathBuf> {
        if p.exists() {
            return Some(p.to_path_buf());
        }
        let name = p.file_name()?;
        relink_dirs.iter().map(|d| d.join(name)).find(|c| c.exists())
    };
    for (kind, lane, p, name, start, src_in, dur) in &imp.clips {
        if *dur <= 0.0 {
            continue;
        }
        let asset = match by_path.get(p) {
            Some(a) => *a,
            None => {
                let new = match find(p).and_then(|real| tl::media::probe_asset(&real).ok()) {
                    Some(a) => a,
                    None => {
                        offline += 1;
                        tl::NewAsset {
                            name: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| name.clone()),
                            path: p.clone(),
                            duration: Some(Time::from_secs_f64(src_in + dur + 1.0)),
                            video: vec![tl::VideoProps { width: imp.size.0, height: imp.size.1, frame_rate: Some(rate) }],
                            audio: vec![tl::AudioProps { sample_rate: 48_000, channels: 2 }],
                        }
                    }
                };
                let id = ed.add_asset(new).map_err(|e| e.to_string())?;
                by_path.insert(p.clone(), id);
                id
            }
        };
        let s = ed.project().sequence(seq).unwrap();
        let tracks = s.tracks_of(*kind);
        let Some(track) = tracks.get(*lane).map(|t| t.id) else { continue };
        let a = ed.project().asset(asset).cloned().unwrap();
        if (*kind == TrackKind::Video && !a.has_video()) || (*kind == TrackKind::Audio && !a.has_audio()) {
            continue;
        }
        let mut spec = tl::ClipSpec::new(asset, Time::from_secs_f64(*start).snap(rate));
        spec.source_in = Time::from_secs_f64(*src_in);
        spec.duration = Some(Time::from_secs_f64(*dur).snap(rate));
        if !name.is_empty() {
            spec.name = Some(name.clone());
        }
        let _ = ed.place_clip(seq, track, spec, PlaceMode::Overwrite);
    }
    if markers {
        for (t, n) in imp.markers {
            let _ = ed.add_marker(seq, Time::from_secs_f64(t).snap(rate), Time::ZERO, n, tl::MarkerColour::Yellow);
        }
    }
    Ok((ed, seq, offline))
}
