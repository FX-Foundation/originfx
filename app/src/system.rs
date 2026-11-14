// SPDX-License-Identifier: GPL-3.0-or-later
//! Start-up, projects, Section A screens, and Section G: settings, shortcuts, plugins, updates,
//! crash recovery.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model as _, SharedString};

use plugin_engine::{Host, Plugin};
use timeline_engine as tl;

use crate::session::Session;
use crate::settings::{expand_home, shorten_home, AppSettings};
use crate::{later, strings, util, vec_model, with, App, Backend, LessonItem, LibraryHead, LibraryItem, Model, PluginItem, RecentProject, Row4, Screen, Settings, ShortcutItem, State};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPO: &str = "https://github.com/FX-Foundation/originfx";

#[derive(Default)]
pub struct SysState {
    pub durations: HashMap<PathBuf, f64>,
    pub recent_filter: String,
    pub recovery: Vec<PathBuf>,
    pub sys: Option<sysinfo::System>,
    pub last_stats: Option<Instant>,
    pub lesson: Option<usize>,
    pub library: Vec<LibraryItem>,
}

// ---------------------------------------------------------------- shortcuts

/// id, command, keys in the OriginFX, Classic and Compact presets
pub const SHORTCUTS: [(&str, &str, [&str; 3]); 23] = [
    ("play", "Play / Pause", ["Space", "Space", "Space"]),
    ("split", "Split at playhead", ["K", "Ctrl+K", "S"]),
    ("delete", "Delete selection", ["Delete", "Delete", "Backspace"]),
    ("ripple", "Ripple delete", ["Shift+Delete", "Shift+Delete", "Shift+Backspace"]),
    ("undo", "Undo", ["Ctrl+Z", "Ctrl+Z", "Ctrl+Z"]),
    ("redo", "Redo", ["Ctrl+Shift+Z", "Ctrl+Y", "Ctrl+Shift+Z"]),
    ("marker", "Add marker", ["M", "M", "M"]),
    ("zoom-in", "Zoom timeline in", ["=", "=", "="]),
    ("zoom-out", "Zoom timeline out", ["-", "-", "-"]),
    ("export", "Export", ["Ctrl+E", "Ctrl+M", "Ctrl+E"]),
    ("mode", "Toggle Simple / Pro", ["Ctrl+Shift+P", "Ctrl+Shift+P", "Tab"]),
    ("save", "Save project", ["Ctrl+S", "Ctrl+S", "Ctrl+S"]),
    ("import", "Import media", ["Ctrl+I", "Ctrl+I", "Ctrl+I"]),
    ("back-frame", "Back one frame", ["Left", "Left", "Left"]),
    ("fwd-frame", "Forward one frame", ["Right", "Right", "Right"]),
    ("start", "Go to start", ["Home", "Home", "Home"]),
    ("end", "Go to end", ["End", "End", "End"]),
    ("in", "Mark in", ["I", "I", "I"]),
    ("out", "Mark out", ["O", "O", "O"]),
    ("select-tool", "Select tool", ["V", "A", "V"]),
    ("razor-tool", "Razor tool", ["C", "B", "C"]),
    ("ripple-tool", "Ripple trim tool", ["B", "R", "R"]),
    ("big-screen", "Big screen preview", ["F", "F", "F"]),
];

fn binding(st: &State, id: &str) -> String {
    if let Some(k) = st.settings.shortcut_overrides.get(id) {
        return k.clone();
    }
    let p = st.settings.shortcut_preset.clamp(0, 2) as usize;
    SHORTCUTS.iter().find(|s| s.0 == id).map(|s| s.2[p].to_string()).unwrap_or_default()
}

fn key_name(text: &str) -> Option<String> {
    use slint::platform::Key;
    let c = text.chars().next()?;
    let k = |key: Key| char::from(key);
    let name = if c == ' ' {
        "Space".into()
    } else if c == k(Key::Delete) {
        "Delete".into()
    } else if c == k(Key::Backspace) {
        "Backspace".into()
    } else if c == k(Key::Escape) {
        "Escape".into()
    } else if c == k(Key::LeftArrow) {
        "Left".into()
    } else if c == k(Key::RightArrow) {
        "Right".into()
    } else if c == k(Key::UpArrow) {
        "Up".into()
    } else if c == k(Key::DownArrow) {
        "Down".into()
    } else if c == k(Key::Home) {
        "Home".into()
    } else if c == k(Key::End) {
        "End".into()
    } else if c == k(Key::Tab) {
        "Tab".into()
    } else if c == k(Key::Return) {
        "Enter".into()
    } else if c == '+' {
        "=".into()
    } else if c == '_' {
        "-".into()
    } else if c.is_control() || (c as u32) >= 0xF700 {
        return None;
    } else {
        c.to_uppercase().to_string()
    };
    Some(name)
}

pub fn handle_key(st: &mut State, ui: &App, text: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
    let Some(name) = key_name(text) else { return false };
    let mut combo = String::new();
    if ctrl {
        combo += "Ctrl+";
    }
    if alt {
        combo += "Alt+";
    }
    if shift && name.chars().count() > 1 || shift && ctrl {
        combo += "Shift+";
    }
    combo += &name;
    let m = ui.global::<Model>();
    // rebinding a shortcut
    let rebinding = m.get_rebinding().to_string();
    if !rebinding.is_empty() {
        if name != "Escape" {
            st.settings.shortcut_overrides.insert(rebinding, combo.clone());
            st.settings.save();
        }
        m.set_rebinding("".into());
        refresh_shortcuts(st, ui);
        return true;
    }
    if name == "Escape" {
        if m.get_big_preview() {
            m.set_big_preview(false);
            return true;
        }
        let s = m.get_screen();
        if s != Screen::Editor && s != Screen::Home && s != Screen::Welcome {
            st.back(ui);
            return true;
        }
        return false;
    }
    let id = SHORTCUTS.iter().map(|s| s.0).find(|id| binding(st, id).eq_ignore_ascii_case(&combo));
    let Some(id) = id else { return false };
    let in_editor_ish = st.session.is_some();
    let zoom = m.get_zoom();
    let pro = ui.global::<Settings>().get_mode() == 2;
    // The key handler runs while the app state is borrowed, and the actions borrow
    // it again, so they run on the next turn of the event loop.
    let weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(0), move || {
        let Some(ui) = weak.upgrade() else { return };
        let b = ui.global::<Backend>();
        match id {
            "play" => b.invoke_play_pause(),
            "split" => b.invoke_split_at_playhead(),
            "delete" => b.invoke_delete_selected(),
            "ripple" => b.invoke_ripple_delete_selected(),
            "undo" => b.invoke_undo(),
            "redo" => b.invoke_redo(),
            "marker" => b.invoke_add_marker(),
            "zoom-in" => b.invoke_zoom_changed(zoom * 1.25),
            "zoom-out" => b.invoke_zoom_changed(zoom / 1.25),
            "export" => b.invoke_open_tool(Screen::Export),
            "mode" => b.invoke_set_mode(if pro { 0 } else { 2 }),
            "save" => b.invoke_save_project(),
            "import" => b.invoke_open_tool(Screen::Import),
            "back-frame" => b.invoke_step(-1),
            "fwd-frame" => b.invoke_step(1),
            "start" => b.invoke_go_start(),
            "end" => b.invoke_go_end(),
            "in" => b.invoke_set_in(),
            "out" => b.invoke_set_out(),
            "select-tool" => b.invoke_set_tool(0),
            "razor-tool" => b.invoke_set_tool(2),
            "ripple-tool" => b.invoke_set_tool(1),
            "big-screen" => {
                let m = ui.global::<Model>();
                if m.get_screen() == Screen::Editor {
                    m.set_big_preview(!m.get_big_preview());
                }
            }
            _ => {}
        }
    });
    in_editor_ish || matches!(id, "mode" | "import")
}


pub fn refresh_shortcuts(st: &mut State, ui: &App) {
    let m = ui.global::<Model>();
    let f = m.get_shortcut_filter().to_lowercase();
    let items: Vec<ShortcutItem> = SHORTCUTS
        .iter()
        .filter(|s| f.is_empty() || s.1.to_lowercase().contains(&f))
        .map(|s| ShortcutItem { id: s.0.into(), command: s.1.into(), keys: binding(st, s.0).into() })
        .collect();
    m.set_shortcuts(vec_model(items));
    m.set_shortcut_preset(st.settings.shortcut_preset);
}

// ---------------------------------------------------------------- plugins

pub fn rebuild_plugins(st: &mut State) {
    let mut host = Host::new();
    if !st.settings.plugins_disabled.contains("org.originfx.example.invert-mix") {
        if let Ok(p) = Plugin::load(&plugin_engine::example_invert_mix_wasm()) {
            host.add("invert-mix.wasm", p);
        }
    }
    if let Ok(rd) = std::fs::read_dir(AppSettings::plugins_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "wasm") {
                if let Ok(pl) = Plugin::load_file(&p) {
                    if !st.settings.plugins_disabled.contains(&pl.manifest().id) {
                        host.add(p.clone(), pl);
                    }
                }
            }
        }
    }
    st.plugins = Arc::new(host);
    if let Some(s) = st.session.as_mut() {
        s.bump();
    }
}

fn refresh_plugins(st: &mut State, ui: &App) {
    let mut items = Vec::new();
    let mut add = |id: String, name: String, desc: String, builtin: bool, colour: i32| {
        let enabled = !st.settings.plugins_disabled.contains(&id);
        items.push(PluginItem { id: id.into(), name: name.into(), desc: desc.into(), enabled, installed: true, colour, builtin });
    };
    if let Ok(p) = Plugin::load(&plugin_engine::example_invert_mix_wasm()) {
        let m = p.manifest();
        add(m.id.clone(), m.name.clone(), if m.description.is_empty() { "Example picture plugin".into() } else { m.description.clone() }, true, 1);
    }
    if let Ok(rd) = std::fs::read_dir(AppSettings::plugins_dir()) {
        for (i, e) in rd.flatten().enumerate() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "wasm") {
                match Plugin::load_file(&p) {
                    Ok(pl) => {
                        let m = pl.manifest();
                        let desc = if m.description.is_empty() { format!("{} · {}", m.version, m.author) } else { m.description.clone() };
                        add(m.id.clone(), m.name.clone(), desc, false, (i % 4) as i32);
                    }
                    Err(err) => add(p.to_string_lossy().to_string(), p.file_name().unwrap_or_default().to_string_lossy().to_string(), format!("Cannot load: {err}"), false, 3),
                }
            }
        }
    }
    ui.global::<Model>().set_plugins(vec_model(items));
}

// ---------------------------------------------------------------- start-up

pub fn detect_gpu(ui: &App) {
    let weak = ui.as_weak();
    let r = ui.window().set_rendering_notifier(move |state, api| {
        if let (slint::RenderingState::RenderingSetup, slint::GraphicsAPI::NativeOpenGL { get_proc_address }) = (state, api) {
            const GL_RENDERER: u32 = 0x1F01;
            let raw = unsafe {
                let ptr = get_proc_address(c"glGetString");
                if ptr.is_null() {
                    None
                } else {
                    let f: extern "system" fn(u32) -> *const std::os::raw::c_char = std::mem::transmute(ptr);
                    let s = f(GL_RENDERER);
                    if s.is_null() {
                        None
                    } else {
                        Some(std::ffi::CStr::from_ptr(s).to_string_lossy().into_owned())
                    }
                }
            };
            report_gpu(&weak, raw);
        }
    });
    if r.is_err() {
        report_gpu(&ui.as_weak(), None);
    }
}

fn report_gpu(weak: &slint::Weak<App>, raw: Option<String>) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let m = ui.global::<Model>();
        let software = |r: &str| {
            let l = r.to_lowercase();
            ["llvmpipe", "softpipe", "swrast", "software", "basic render", "swiftshader"].iter().any(|s| l.contains(s))
        };
        match raw {
            Some(r) if !software(&r) && !r.trim().is_empty() => {
                let mut name = r.split('/').next().unwrap_or(&r).trim().to_string();
                for junk in ["(R)", "(TM)", "(tm)", "Mesa ", "ANGLE (", "Direct3D11 vs_5_0 ps_5_0", "OpenGL Engine"] {
                    name = name.replace(junk, "");
                }
                let name = name.split_whitespace().collect::<Vec<_>>().join(" ").trim_matches(|c| c == ',' || c == ')').trim().to_string();
                m.set_gpu_name("Ready for smooth editing".into());
                m.set_gpu_line(format!("{name} found. Hardware acceleration is on.").into());
                m.set_gpu_device(name.into());
                m.set_gpu_ok(true);
            }
            _ => {
                m.set_gpu_name("Running on your processor".into());
                m.set_gpu_line("No graphics card found. Hardware acceleration is off.".into());
                m.set_gpu_device("Processor (no GPU found)".into());
                m.set_gpu_ok(false);
            }
        }
    });
}

thread_local! {
    // The scale factor we last set ourselves (0 = never).
    static LAST_SCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
}

/// Keep the interface at the size the user picked, on top of whatever scale the
/// window system reports. Called often and cheap when nothing changed. If the
/// window system changes the scale (first show, moving to another monitor), the
/// new value becomes the base and the user's percentage is applied again.
pub fn sync_ui_scale(ui: &App, percent: f32) {
    let win = ui.window();
    let cur = win.scale_factor();
    let last = LAST_SCALE.with(|l| l.get());
    if last > 0.0 && (cur - last).abs() < 0.001 && (percent - PERCENT.with(|p| p.get())).abs() < 0.01 {
        return;
    }
    let base = if last > 0.0 && (cur - last).abs() < 0.001 {
        BASE.with(|b| b.get())
    } else {
        cur
    };
    let want = base * (percent.clamp(50.0, 200.0) / 100.0);
    BASE.with(|b| b.set(base));
    PERCENT.with(|p| p.set(percent));
    LAST_SCALE.with(|l| l.set(want));
    if (cur - want).abs() < 0.001 {
        return;
    }
    // Keep the window the same size on screen; only the interface inside it grows.
    let physical = win.size();
    win.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: want });
    win.dispatch_event(slint::platform::WindowEvent::Resized {
        size: slint::LogicalSize::new(physical.width as f32 / want, physical.height as f32 / want),
    });
}

thread_local! {
    static BASE: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
    static PERCENT: std::cell::Cell<f32> = const { std::cell::Cell::new(100.0) };
}

pub fn apply_settings(st: &mut State, ui: &App) {
    let s = &st.settings;
    let g = ui.global::<Settings>();
    g.set_look(s.look);
    g.set_mode(s.workspace);
    g.set_hints(s.hints);
    g.set_reduce_motion(s.reduce_motion);
    g.set_timecode_style(s.timecode_style);
    let m = ui.global::<Model>();
    m.set_projects_folder(s.storage.clone().into());
    m.set_autosave_minutes(s.autosave_minutes);
    m.set_default_fps(s.frame_rate);
    m.set_default_res(s.resolution);
    m.set_hw_accel(s.hw_accel);
    m.set_crash_reports(s.crash_reports);
    m.set_theme_choice(match s.look {
        1 => 1,
        2 => 0,
        _ => 2,
    });
    m.set_layout_choice(if s.workspace == 2 { 1 } else { 0 });
    m.set_ui_scale(s.ui_scale);
    m.set_governor(s.governor);
    m.set_memory_limit_gb(s.memory_limit_gb);
    m.set_background_tasks(s.background_tasks);
    m.set_update_channel(s.update_channel);
    m.set_auto_updates(s.auto_updates);
    m.set_version(VERSION.into());
    m.set_update_checked(if s.last_update_check.is_empty() { "Never checked.".into() } else { format!("Last checked {}.", s.last_update_check).into() });
    m.set_language(s.language.clone().into());
    let total = sysinfo::System::new_with_specifics(sysinfo::RefreshKind::nothing().with_memory(sysinfo::MemoryRefreshKind::everything())).total_memory();
    m.set_memory_max_gb(((total as f64 / 1e9).floor() as f32).max(2.0));
}

pub fn startup(st: &mut State, ui: &App) {
    rebuild_plugins(st);
    apply_settings(st, ui);
    refresh_lessons(st, ui);
    refresh_shortcuts(st, ui);
    let m = ui.global::<Model>();
    // crash recovery
    let crashed = !st.settings.clean_exit;
    st.settings.clean_exit = false;
    st.settings.save();
    if crashed {
        let mut files: Vec<(PathBuf, std::time::SystemTime)> = std::fs::read_dir(AppSettings::recovery_dir())
            .map(|rd| rd.flatten().filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?))).collect())
            .unwrap_or_default();
        files.sort_by_key(|f| std::cmp::Reverse(f.1));
        files.truncate(3);
        if !files.is_empty() {
            let rows: Vec<Row4> = files
                .iter()
                .map(|(p, t)| {
                    let name = Project_name(p);
                    let when = t.duration_since(std::time::UNIX_EPOCH).map(|d| util::clock(d.as_secs() as i64)).unwrap_or_default();
                    Row4 { a: name.into(), b: format!("Autosaved today, {when}").into(), c: "".into(), d: "".into(), status: 0, status_text: "".into() }
                })
                .collect();
            st.sys.recovery = files.into_iter().map(|f| f.0).collect();
            m.set_recovery_options(vec_model(rows));
            m.set_recovery_pick(0);
            m.set_screen(Screen::Recovery);
            return;
        }
    }
    // a project passed on the command line
    if let Some(arg) = std::env::args().nth(1) {
        let p = PathBuf::from(arg);
        if p.extension().is_some_and(|e| e == "originfx") {
            open_project(st, ui, &p);
            return;
        }
    }
    if st.settings.setup_done {
        m.set_screen(Screen::Home);
        refresh_recent(st, ui);
    } else {
        m.set_screen(Screen::Welcome);
    }
    if st.settings.auto_updates {
        check_updates(st, ui, true);
    }
}

#[allow(non_snake_case)]
fn Project_name(p: &Path) -> String {
    tl::Project::load(p).map(|pr| pr.name).unwrap_or_else(|_| p.file_stem().unwrap_or_default().to_string_lossy().to_string())
}

pub fn shutdown(st: &mut State, _ui: &App) {
    if let Some(s) = st.session.as_mut() {
        if s.is_dirty() {
            let _ = s.save_recovery(&AppSettings::recovery_dir());
            if let Some(p) = s.path.clone() {
                let _ = s.save(&p);
            }
        }
    }
    st.settings.clean_exit = true;
    st.settings.save();
}

pub fn autosave_tick(st: &mut State, ui: &App) {
    // performance numbers, once a second while settings are open
    if ui.global::<Model>().get_screen() == Screen::Settings && st.sys.last_stats.is_none_or(|t| t.elapsed() > Duration::from_secs(1)) {
        st.sys.last_stats = Some(Instant::now());
        let sys = st.sys.sys.get_or_insert_with(sysinfo::System::new);
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let m = ui.global::<Model>();
        m.set_cpu_pct(sys.global_cpu_usage());
        let total = sys.total_memory().max(1) as f32;
        m.set_mem_pct(sys.used_memory() as f32 / total * 100.0);
    }
    let mins = st.settings.autosave_minutes;
    if mins <= 0 {
        return;
    }
    let Some(s) = st.session.as_mut() else { return };
    if s.last_autosave.elapsed() < Duration::from_secs(mins as u64 * 60) {
        return;
    }
    s.last_autosave = Instant::now();
    if !s.is_dirty() {
        return;
    }
    let _ = s.save_recovery(&AppSettings::recovery_dir());
    if let Some(p) = s.path.clone() {
        if s.save(&p).is_ok() {
            ui.global::<Model>().set_status("Saved automatically".into());
            ui.global::<Model>().set_dirty(false);
        }
    }
}

// ---------------------------------------------------------------- projects

pub fn new_project_path(st: &State, name: &str) -> PathBuf {
    let safe: String = name.chars().map(|c| if "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let safe = if safe.trim().is_empty() { "Untitled project".to_string() } else { safe.trim().to_string() };
    let root = st.settings.storage_dir();
    let mut dir = root.join(&safe);
    let mut n = 2;
    while dir.join(format!("{safe}.originfx")).exists() {
        dir = root.join(format!("{safe} {n}"));
        n += 1;
    }
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
    dir.join(format!("{file}.originfx"))
}

pub fn after_save(st: &mut State, ui: &App) {
    if let Some(s) = &st.session {
        if let Some(p) = &s.path {
            let (name, path, aspect) = (s.name(), p.to_string_lossy().to_string(), s.aspect_name());
            st.settings.touch_recent(&name, &path, &aspect);
            st.settings.save();
        }
    }
    st.sync(ui);
}

fn create_project(st: &mut State, ui: &App, name: &str, folder: Option<PathBuf>, code: i32) {
    let name = if name.trim().is_empty() { "Untitled project" } else { name.trim() };
    let mut s = Session::create(name, code, st.settings.default_fps());
    let path = match folder {
        Some(f) => {
            let dir = f.join(name);
            let _ = std::fs::create_dir_all(&dir);
            let mut p = dir.join(format!("{name}.originfx"));
            let mut n = 2;
            while p.exists() {
                p = dir.join(format!("{name} {n}.originfx"));
                n += 1;
            }
            p
        }
        None => new_project_path(st, name),
    };
    if let Err(e) = s.save(&path) {
        st.toast(ui, format!("Could not save the project: {e}"));
    }
    st.load_session(ui, s);
    after_save(st, ui);
    st.nav.clear();
    st.nav.push(Screen::Home);
    ui.global::<Model>().set_screen(Screen::Editor);
    crate::bump_focus(ui);
}

pub fn open_project(st: &mut State, ui: &App, path: &Path) {
    match Session::open(path) {
        Ok(s) => {
            st.load_session(ui, s);
            let missing = st.session.as_ref().map(|s| s.project().missing_assets().len()).unwrap_or(0);
            st.nav.clear();
            st.nav.push(Screen::Home);
            ui.global::<Model>().set_screen(Screen::Editor);
            crate::bump_focus(ui);
            if missing > 0 {
                st.toast(ui, format!("{missing} media files are missing. Relink them in the Media inspector."));
            }
        }
        Err(e) => st.toast(ui, format!("Could not open: {e}")),
    }
}

pub fn refresh_recent(st: &mut State, ui: &App) {
    let hidden = st.settings.hidden.clone();
    st.settings.recent.retain(|r| Path::new(&r.path).exists() && !hidden.contains(&r.path));
    // also list projects found in the projects folder
    let root = st.settings.storage_dir();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            let d = e.path();
            if d.is_dir() {
                if let Ok(inner) = std::fs::read_dir(&d) {
                    for f in inner.flatten() {
                        let p = f.path();
                        if p.extension().is_some_and(|x| x == "originfx") && !hidden.contains(&p.to_string_lossy().to_string()) && !st.settings.recent.iter().any(|r| Path::new(&r.path) == p) {
                            let t = f.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
                            st.settings.recent.push(crate::settings::RecentEntry {
                                name: p.file_stem().unwrap_or_default().to_string_lossy().to_string(),
                                path: p.to_string_lossy().to_string(),
                                edited_unix: t,
                                aspect: "16:9".into(),
                            });
                        }
                    }
                }
            }
        }
    }
    st.settings.recent.sort_by_key(|r| std::cmp::Reverse(r.edited_unix));
    let f = st.sys.recent_filter.to_lowercase();
    let items: Vec<RecentProject> = st
        .settings
        .recent
        .iter()
        .filter(|r| f.is_empty() || r.name.to_lowercase().contains(&f))
        .map(|r| RecentProject {
            name: r.name.clone().into(),
            path: r.path.clone().into(),
            edited: util::relative_date(r.edited_unix).into(),
            aspect: r.aspect.clone().into(),
            thumb: Default::default(),
            has_thumb: false,
        })
        .collect();
    ui.global::<Model>().set_recent(vec_model(items));
}

// ---------------------------------------------------------------- learn

const LESSONS: [(&str, &str); 26] = [
    ("Bring in your footage", "Import a clip from your computer."),
    ("Put a clip on the timeline", "Double-click a clip in the media list."),
    ("Play and scrub", "Press Space to play. Click the ruler to jump."),
    ("Cut your first clip", "Drag the edge of a clip to trim it, then press K to split it."),
    ("Delete a piece", "Select a clip and press Delete."),
    ("Close the gap", "Use Shift+Delete to delete and pull the rest left."),
    ("Move clips around", "Drag a clip along its track."),
    ("Set the volume", "Select an audio clip and drag the volume slider."),
    ("Fade in and out", "Pro mode, Audio tab: set the fade times."),
    ("Mix your tracks", "Open the Mixer and balance each track."),
    ("Even out loudness", "Audio cleanup: pick a target and press Normalize."),
    ("Pick a look", "Open Color and click a look under the picture."),
    ("Fix exposure", "In Color, raise or lower Exposure."),
    ("Warm it up", "Move Temperature to the right."),
    ("Read the scopes", "Open Scopes and watch the waveform."),
    ("Match two shots", "Scopes: pick a reference and a target, then Match."),
    ("Add a title", "Open Titles, write your words, Add to timeline."),
    ("Animate it in", "Titles: set the In animation to Slide."),
    ("Smooth a cut", "Transitions: pick Cross dissolve, apply to the cut."),
    ("Keyframe a move", "Keyframes: add two keys with different positions."),
    ("Speed ramps", "Speed and retime: pick Speed ramp, then Apply."),
    ("Precision trim", "Double-click a cut and nudge frame by frame."),
    ("Blend modes", "Pro mode: change a clip's blend mode."),
    ("Masks", "Shapes and masks: draw an ellipse on a clip."),
    ("Multicam", "Sync angles, then cut between them."),
    ("Export your film", "Press Ctrl+E and pick YouTube."),
];

fn refresh_lessons(st: &mut State, ui: &App) {
    let items: Vec<LessonItem> = LESSONS
        .iter()
        .enumerate()
        .map(|(i, (t, s))| LessonItem { title: (*t).into(), sub: (*s).into(), done: st.settings.lessons_done.contains(&i), level: 0 })
        .collect();
    let m = ui.global::<Model>();
    m.set_lessons_done(st.settings.lessons_done.len() as i32);
    let next = (0..LESSONS.len()).find(|i| !st.settings.lessons_done.contains(i)).unwrap_or(0);
    if m.get_lessons().row_count() == 0 {
        m.set_current_lesson(next as i32);
    }
    m.set_lessons(vec_model(items));
}

// ---------------------------------------------------------------- library

/// name, group, description, length, looks like, path. An empty path means design only for now.
/// The `transition:N` entries are the eleven that work today.
const LIB_TRANSITIONS: [(&str, &str, &str, &str, &str, &str); 38] = [
    ("Cross dissolve", "Basics", "One clip fades into the next.", "1 second", "Smooth", "transition:0"),
    ("Dip to black", "Basics", "Fades out to black, then in to the next clip.", "1 second", "Smooth", "transition:1"),
    ("Dip to white", "Basics", "Fades out to white, then in to the next clip.", "1 second", "Smooth", "transition:2"),
    ("Fade in", "Basics", "A clip fades up from black.", "1 second", "Smooth", ""),
    ("Fade out", "Basics", "A clip fades down to black.", "1 second", "Smooth", ""),
    ("Film flash", "Basics", "A short white pulse on the cut.", "0.3 seconds", "Punchy", ""),
    ("Wipe left", "Wipes", "The next clip wipes in from the right.", "1 second", "Clean", "transition:3"),
    ("Wipe right", "Wipes", "The next clip wipes in from the left.", "1 second", "Clean", "transition:4"),
    ("Wipe up", "Wipes", "The next clip wipes in from the bottom.", "1 second", "Clean", "transition:5"),
    ("Wipe down", "Wipes", "The next clip wipes in from the top.", "1 second", "Clean", "transition:6"),
    ("Diagonal wipe", "Wipes", "The next clip wipes in along a diagonal edge.", "1 second", "Clean", ""),
    ("Circle iris", "Wipes", "A circle opens or closes to reveal the next clip.", "1 second", "Classic", ""),
    ("Clock wipe", "Wipes", "A sweeping hand reveals the next clip like a clock.", "1 second", "Classic", ""),
    ("Push left", "Push and slide", "The next clip pushes the old one out to the left.", "1 second", "Smooth", "transition:7"),
    ("Push right", "Push and slide", "The next clip pushes the old one out to the right.", "1 second", "Smooth", "transition:8"),
    ("Push up", "Push and slide", "The next clip pushes the old one out upward.", "1 second", "Smooth", "transition:9"),
    ("Push down", "Push and slide", "The next clip pushes the old one out downward.", "1 second", "Smooth", "transition:10"),
    ("Slide over", "Push and slide", "The next clip slides on top of the old one.", "1 second", "Smooth", ""),
    ("Reveal", "Push and slide", "The old clip slides away to show the next one underneath.", "1 second", "Smooth", ""),
    ("Zoom in", "Zoom and motion", "The picture zooms in and cuts to the next clip.", "0.6 seconds", "Dynamic", ""),
    ("Zoom out", "Zoom and motion", "The picture zooms out into the next clip.", "0.6 seconds", "Dynamic", ""),
    ("Whip pan", "Zoom and motion", "A fast blurred swipe, popular in Reels and Shorts.", "0.4 seconds", "Dynamic", ""),
    ("Spin zoom", "Zoom and motion", "A fast spinning zoom into the next clip.", "0.6 seconds", "Dynamic", ""),
    ("Shake", "Zoom and motion", "A quick camera shake on the cut.", "0.4 seconds", "Punchy", ""),
    ("Push zoom", "Zoom and motion", "The picture zooms through the cut.", "0.6 seconds", "Dynamic", ""),
    ("Blur dissolve", "Blur and light", "Blurs out, then sharpens into the next clip.", "1 second", "Soft", ""),
    ("Light leak", "Blur and light", "A warm flare of light sweeps across the cut.", "1 second", "Warm", ""),
    ("Glow flash", "Blur and light", "The picture blooms with glow, then settles on the next clip.", "0.8 seconds", "Soft", ""),
    ("Defocus", "Blur and light", "The picture drifts out of focus, then back into the next clip.", "1 second", "Soft", ""),
    ("Film burn", "Blur and light", "An orange film burn covers the cut.", "1 second", "Warm", ""),
    ("Glitch", "Style", "Broken digital blocks and tearing on the cut.", "0.5 seconds", "Digital", ""),
    ("RGB split", "Style", "The red, green and blue channels slide apart on the cut.", "0.5 seconds", "Digital", ""),
    ("VHS", "Style", "Tape distortion and noise bars on the cut.", "0.6 seconds", "Retro", ""),
    ("Digital stretch", "Style", "The picture smears and stretches across the cut.", "0.5 seconds", "Digital", ""),
    ("Luma fade", "Style", "The next clip appears in order of brightness.", "1 second", "Smooth", ""),
    ("Ink bleed", "Style", "Ink spreads across the frame to reveal the next clip.", "1.2 seconds", "Organic", ""),
    ("Save as preset", "My transitions", "Tweak any transition, then save it here under your own name.", "Any", "Yours", ""),
    ("Import a pack", "My transitions", "Add transitions shared by other creators. Drop a pack into your Library folder.", "Any", "Shared", ""),
];

/// name, group, description, length, looks like, path. An empty path means the tile is design only
/// for now (no engine behind it yet). The four with `title:N` paths are the ones that work today.
const TITLES: [(&str, &str, &str, &str, &str, &str); 30] = [
    ("Lower third · Bar", "Lower thirds", "A name tag with an accent bar. Type a name and a role.", "5 seconds", "Broadcast", "title:0"),
    ("Lower third · Minimal", "Lower thirds", "A thin line with small text. Quiet and clean.", "5 seconds", "Clean", "title:1"),
    ("Lower third · Box", "Lower thirds", "A solid panel behind the name and role.", "5 seconds", "Solid", ""),
    ("Lower third · Slide-in tag", "Lower thirds", "A tag that slides in from the edge of the screen.", "5 seconds", "Dynamic", ""),
    ("Lower third · Split two-tone", "Lower thirds", "Two colour blocks, one for the name and one for the role.", "5 seconds", "Bold", ""),
    ("Lower third · Glass panel", "Lower thirds", "A frosted, blurred panel with the name on it.", "5 seconds", "Soft", ""),
    ("Lower third · Name, role, logo", "Lower thirds", "A name tag with a slot for a logo.", "5 seconds", "Branded", ""),
    ("Title · Centered", "Full-screen titles", "Words in the middle of the screen. Change the text, size and timing after adding it.", "5 seconds", "Cinematic", "title:2"),
    ("Title · Split", "Full-screen titles", "A title in two parts, with a big word and a smaller one.", "5 seconds", "Editorial", "title:3"),
    ("Title · Cinematic", "Full-screen titles", "Wide-spaced letters that fade in slowly.", "5 seconds", "Cinematic", ""),
    ("Title · Stacked", "Full-screen titles", "Words stacked in big lines.", "5 seconds", "Bold", ""),
    ("Title · Number and label", "Full-screen titles", "A big number with a small label under it.", "5 seconds", "Bold", ""),
    ("Title · Quote card", "Full-screen titles", "A quote with the speaker's name below.", "5 seconds", "Editorial", ""),
    ("Opening · Film title reveal", "Opening and closing", "The title appears like the start of a film.", "5 seconds", "Cinematic", ""),
    ("Opening · Chapter number", "Opening and closing", "Part 2, Day 3 or any section marker.", "3 seconds", "Clean", ""),
    ("Closing · End card", "Opening and closing", "The channel name and a subscribe prompt.", "8 seconds", "Branded", ""),
    ("Closing · Scrolling credits", "Opening and closing", "Names that scroll up the screen.", "Fits the list", "Classic", ""),
    ("Captions · Standard", "Captions and subtitles", "Plain captions along the bottom, timed to the voice.", "Follows speech", "Clean", ""),
    ("Captions · Word highlight", "Captions and subtitles", "Each spoken word lights up as it is said.", "Follows speech", "Social", ""),
    ("Captions · Karaoke fill", "Captions and subtitles", "Words fill with colour as they are spoken.", "Follows speech", "Social", ""),
    ("Captions · Two speakers", "Captions and subtitles", "A different colour for each voice.", "Follows speech", "Clean", ""),
    ("Call-out · Label with arrow", "Call-outs", "A label with an arrow that points at something in the shot.", "4 seconds", "Clean", ""),
    ("Call-out · Circle and ping", "Call-outs", "A ring that pulses around an object.", "3 seconds", "Bold", ""),
    ("Call-out · Underline draw-on", "Call-outs", "A line that draws itself under the words.", "3 seconds", "Clean", ""),
    ("Call-out · Counter", "Call-outs", "A number that counts up to a value.", "3 seconds", "Bold", ""),
    ("Call-out · Progress bar", "Call-outs", "A bar that fills to a percentage.", "4 seconds", "Clean", ""),
    ("Social · Reel hook", "Social styles", "A bold opening line for the first second of a Reel or Short.", "2 seconds", "Social", ""),
    ("Social · TikTok captions", "Social styles", "Big, punchy captions in the short-video style.", "Follows speech", "Social", ""),
    ("Social · Countdown", "Social styles", "A 3, 2, 1 countdown.", "3 seconds", "Bold", ""),
    ("Social · Follow tag", "Social styles", "A swipe-up or follow prompt that slides in.", "3 seconds", "Social", ""),
];

/// name, group, description, looks like. Order must match `prim-icons` in start.slint.
const PRIMITIVES: [(&str, &str, &str, &str); 25] = [
    ("Rectangle", "Shape", "A box. Each corner can be rounded on its own.", "Solid"),
    ("Rounded rectangle", "Shape", "A box with soft corners, set with one radius slider.", "Soft"),
    ("Ellipse", "Shape", "A circle or an oval.", "Round"),
    ("Polygon", "Shape", "A shape with 3 to 12 straight sides.", "Angular"),
    ("Star", "Shape", "Set the number of points and how deep they cut in.", "Pointed"),
    ("Line", "Shape", "A straight line with round, square or arrow ends. Can be dashed.", "Thin"),
    ("Arrow", "Shape", "A line with an arrowhead. Good for pointing at things.", "Thin"),
    ("Free path", "Shape", "Draw any curve with the pen.", "Custom"),
    ("Solid fill", "Fill and outline", "One flat colour for a shape or for text.", "Flat"),
    ("Linear gradient", "Fill and outline", "A colour that fades from one side to the other.", "Smooth"),
    ("Radial gradient", "Fill and outline", "A colour that fades outward from a centre point.", "Smooth"),
    ("Angular gradient", "Fill and outline", "A colour that sweeps around a centre like a clock face.", "Smooth"),
    ("Outline", "Fill and outline", "A border with its own width, colour and corner style.", "Crisp"),
    ("Dashed outline", "Fill and outline", "An outline made of dashes or dots.", "Crisp"),
    ("Opacity", "Fill and outline", "How see-through a shape or text is.", "Soft"),
    ("Blur", "Fill and outline", "Softens the edges of one element, not the whole picture.", "Soft"),
    ("Shadow", "Fill and outline", "A soft shadow behind a shape or text.", "Soft"),
    ("Text block", "Text", "One or many lines with font, size, weight, spacing and alignment.", "Clean"),
    ("Mixed styling", "Text", "A different colour or weight on a single word.", "Clean"),
    ("Fonts and features", "Text", "Variable fonts and stylistic alternates, bundled so every computer shows the same thing.", "Clean"),
    ("Text on a curve", "Text", "Words that follow a path.", "Flowing"),
    ("Background", "Helper", "A full-frame solid colour or gradient.", "Flat"),
    ("Group", "Helper", "Several elements that move together as one.", "Neutral"),
    ("Anchor", "Helper", "The point a shape rotates and scales around.", "Neutral"),
    ("Safe area", "Helper", "A guide so titles never get cropped on TVs and phones.", "Neutral"),
];

/// name, group, description, looks like, real effect (name in EFFECT_TILES, or empty for design only).
const LIB_EFFECTS: [(&str, &str, &str, &str, &str); 54] = [
    ("Brightness", "Light", "Make the picture lighter or darker.", "Light", "Brightness"),
    ("Exposure", "Light", "Like opening or closing the camera lens.", "Light", "Exposure"),
    ("Contrast", "Light", "Push darks darker and lights lighter.", "Punchy", "Contrast"),
    ("Gamma", "Light", "Change the middle tones without touching black and white.", "Soft", "Gamma"),
    ("Shadows and highlights", "Light", "Rescue dark corners and blown-out skies separately.", "Natural", ""),
    ("Vignette", "Light", "Darken the edges to pull the eye to the centre.", "Moody", "Vignette"),
    ("Saturation", "Color", "How strong or washed out the colours are.", "Vivid", "Saturation"),
    ("Hue shift", "Color", "Rotate every colour around the wheel.", "Wild", "Hue shift"),
    ("Temperature", "Color", "Warmer like sunset or cooler like shade.", "Warm", "Temperature"),
    ("Tint", "Color", "Slide between green and magenta.", "Subtle", ""),
    ("Vibrance", "Color", "Boost dull colours and leave strong ones alone.", "Natural", ""),
    ("Black and white", "Color", "Remove all colour.", "Classic", "Black and white"),
    ("Sepia", "Color", "An old brown photograph look.", "Vintage", "Sepia"),
    ("Invert", "Color", "Flip every colour to its opposite.", "Wild", "Invert"),
    ("Curves", "Color grade", "Draw your own tone curve, per colour channel if you like.", "Pro", ""),
    ("Colour wheels", "Color grade", "Tint shadows, mid-tones and highlights separately.", "Pro", ""),
    ("LUT", "Color grade", "Import a .cube look file made in any other grading tool.", "Pro", ""),
    ("Teal and orange", "Color grade", "The blockbuster look: cool shadows, warm skin.", "Cinematic", ""),
    ("Film look", "Color grade", "Lifted blacks and soft colours like real film stock.", "Cinematic", ""),
    ("Match colour", "Color grade", "Make one clip look like another in one click.", "Smart", ""),
    ("Gaussian blur", "Blur and sharpen", "A smooth, even blur.", "Soft", "Gaussian blur"),
    ("Sharpen", "Blur and sharpen", "Bring out fine detail.", "Crisp", "Sharpen"),
    ("Motion blur", "Blur and sharpen", "Smear the picture in one direction like fast movement.", "Fast", ""),
    ("Radial blur", "Blur and sharpen", "Spin or zoom blur from a centre point.", "Fast", ""),
    ("Tilt-shift", "Blur and sharpen", "Blur the top and bottom so the scene looks like a tiny model.", "Miniature", ""),
    ("Lens blur", "Blur and sharpen", "Round, camera-style blur with bright bokeh discs.", "Dreamy", ""),
    ("Mosaic", "Stylize", "Big blocky pixels.", "Blocky", "Mosaic"),
    ("Posterize", "Stylize", "Cut the picture down to a few flat colour levels.", "Graphic", ""),
    ("Halftone", "Stylize", "Newspaper-style dots.", "Print", ""),
    ("Edge glow", "Stylize", "Neon light along every edge.", "Neon", ""),
    ("Cartoon outline", "Stylize", "Flat colours with ink lines.", "Comic", ""),
    ("Duotone", "Stylize", "Paint the picture with just two colours.", "Graphic", ""),
    ("Pixel art", "Stylize", "Retro game pixels with a limited palette.", "Retro", ""),
    ("Mirror", "Distort", "Reflect one half of the picture onto the other.", "Symmetric", ""),
    ("Kaleidoscope", "Distort", "Repeat a slice of the picture in a circle.", "Trippy", ""),
    ("Wave", "Distort", "Ripple the picture like water.", "Fluid", ""),
    ("Fisheye", "Distort", "A bulging wide-angle lens.", "Action", ""),
    ("Twirl", "Distort", "Spin the middle of the picture into a whirl.", "Trippy", ""),
    ("Denoise", "Cleanup", "Clean up grain from dark, noisy footage.", "Clean", ""),
    ("Deflicker", "Cleanup", "Even out flickering lights and time-lapse jumps.", "Clean", ""),
    ("Stabilise", "Cleanup", "Calm down shaky handheld footage.", "Smooth", ""),
    ("Defog", "Cleanup", "Cut through haze and mist.", "Clear", ""),
    ("One-click auto-fix", "Cleanup", "Fixes exposure and colour for you. Undo if you don't like it.", "Smart", ""),
    ("Background remover", "Keying and masks", "Cut out a person from any background, no green screen needed.", "Smart", ""),
    ("Green screen key", "Keying and masks", "Remove a green or blue backdrop.", "Studio", ""),
    ("Luma key", "Keying and masks", "Remove the brightest or darkest parts.", "Studio", ""),
    ("Colour key", "Keying and masks", "Remove any single colour you pick.", "Studio", ""),
    ("Edge cleanup", "Keying and masks", "Remove green fringes around a keyed subject.", "Studio", ""),
    ("Soft mask edge", "Keying and masks", "Feather the border of any mask.", "Soft", ""),
    ("Film grain", "Film and noise", "Fine grain like real film.", "Vintage", ""),
    ("Halation", "Film and noise", "A red glow around bright lights, like old film.", "Vintage", ""),
    ("Chromatic aberration", "Film and noise", "Colour fringes at the edges, like a cheap lens.", "Glitchy", ""),
    ("Scanlines", "Film and noise", "Old TV lines.", "Retro", ""),
    ("Noise", "Film and noise", "Random speckle you control.", "Rough", ""),
];

/// name, group, description, length, looks like
const LIB_MUSIC: [(&str, &str, &str, &str, &str); 22] = [
    ("Slow horizon", "Cinematic", "Wide strings and a soft piano for opening shots.", "2:30", "Emotional"),
    ("Last light", "Cinematic", "A warm, rising theme for a story's ending.", "2:10", "Warm"),
    ("Open road", "Cinematic", "Steady, hopeful movement for travel footage.", "2:45", "Hopeful"),
    ("Quiet city", "Cinematic", "Soft and thoughtful, good under narration.", "3:00", "Calm"),
    ("Sunny side", "Upbeat", "Bright claps and ukulele. Instant good mood.", "2:00", "Happy"),
    ("Good vibes", "Upbeat", "Funky bass and a tight groove.", "2:20", "Fun"),
    ("Run it", "Upbeat", "Driving drums for sport and fitness.", "2:05", "Energetic"),
    ("Pop spark", "Upbeat", "Light, catchy and made for short videos.", "1:30", "Bouncy"),
    ("Late tea", "Lo-fi", "Dusty beat, warm keys.", "2:40", "Chill"),
    ("Rainy desk", "Lo-fi", "Slow drums with soft rain in the background.", "3:00", "Cosy"),
    ("Study loop", "Lo-fi", "A steady loop that never gets in the way.", "2:00", "Focus"),
    ("Clean slide", "Corporate", "Polished and positive for company videos.", "2:15", "Professional"),
    ("Big plans", "Corporate", "Steady build, great for product launches.", "2:30", "Confident"),
    ("Pitch day", "Corporate", "Light, modern and neutral.", "2:00", "Neutral"),
    ("Closing in", "Tension", "Low pulses that slowly get tighter.", "2:10", "Dark"),
    ("Cold room", "Tension", "Thin, uneasy textures for thrillers.", "2:30", "Eerie"),
    ("Heartbeat", "Tension", "A slow pulse under a rising drone.", "1:45", "Suspense"),
    ("Rise of kings", "Epic and trailer", "Huge drums and a heroic brass theme.", "2:40", "Heroic"),
    ("Final stand", "Epic and trailer", "Fast, dense and loud for action.", "2:20", "Intense"),
    ("Title drop", "Epic and trailer", "A short hit-and-build for trailers and reveals.", "1:00", "Dramatic"),
    ("Soft focus", "Ambient", "A drifting pad with no beat.", "3:30", "Floating"),
    ("Deep water", "Ambient", "Slow, deep tones for calm scenes.", "4:00", "Peaceful"),
];

/// name, group, description, length, looks like
const LIB_SFX: [(&str, &str, &str, &str, &str); 60] = [
    ("Fast whoosh", "Whooshes and hits", "A quick air swipe.", "0:01", "Sharp"),
    ("Slow whoosh", "Whooshes and hits", "A long, soft pass-by.", "0:02", "Smooth"),
    ("Whip pan", "Whooshes and hits", "A fast swish with a tail.", "0:01", "Fast"),
    ("Swipe", "Whooshes and hits", "A light sweep for text and graphics.", "0:01", "Light"),
    ("Punch hit", "Whooshes and hits", "A short, tight hit.", "0:01", "Tight"),
    ("Slap hit", "Whooshes and hits", "A crisp, bright smack.", "0:01", "Bright"),
    ("Sword swish", "Whooshes and hits", "A metal blade through the air.", "0:01", "Metal"),
    ("Arrow pass", "Whooshes and hits", "Something thin flying past the ear.", "0:01", "Thin"),
    ("Deep boom", "Impacts and booms", "A heavy low boom.", "0:03", "Heavy"),
    ("Cinematic hit", "Impacts and booms", "A big trailer-style slam.", "0:03", "Trailer"),
    ("Door slam", "Impacts and booms", "A solid wooden thud.", "0:02", "Solid"),
    ("Metal clang", "Impacts and booms", "A heavy metal strike with a ring.", "0:02", "Ringing"),
    ("Explosion", "Impacts and booms", "A rumbling blast with debris.", "0:04", "Loud"),
    ("Glass break", "Impacts and booms", "A crash of glass shards.", "0:02", "Sharp"),
    ("Riser short", "Risers and drones", "A 3-second build to a cut.", "0:03", "Rising"),
    ("Riser long", "Risers and drones", "A 8-second build for reveals.", "0:08", "Rising"),
    ("Reverse swell", "Risers and drones", "A backwards sound sweeping into a hit.", "0:04", "Reverse"),
    ("Low drone", "Risers and drones", "A deep, steady rumble.", "0:10", "Dark"),
    ("Tension drone", "Risers and drones", "A thin, uneasy hum.", "0:10", "Eerie"),
    ("Heartbeat pulse", "Risers and drones", "A slow, deep beat.", "0:08", "Pulse"),
    ("Button click", "UI and clicks", "A small, clean click.", "0:01", "Clean"),
    ("Toggle on", "UI and clicks", "A switch turning on.", "0:01", "Clean"),
    ("Toggle off", "UI and clicks", "A switch turning off.", "0:01", "Clean"),
    ("Notification", "UI and clicks", "A friendly two-note ping.", "0:01", "Friendly"),
    ("Success chime", "UI and clicks", "A bright chime for done and correct.", "0:01", "Bright"),
    ("Error tone", "UI and clicks", "A short low buzz for wrong.", "0:01", "Low"),
    ("Pop", "UI and clicks", "A small bubble pop.", "0:01", "Light"),
    ("Typing", "UI and clicks", "Keyboard keys in a quick burst.", "0:03", "Busy"),
    ("Footsteps on wood", "Foley", "A person walking indoors.", "0:06", "Natural"),
    ("Footsteps on gravel", "Foley", "Walking outside on small stones.", "0:06", "Natural"),
    ("Door open", "Foley", "A door creaking open.", "0:02", "Natural"),
    ("Door close", "Foley", "A door closing gently.", "0:01", "Natural"),
    ("Cloth movement", "Foley", "Jacket and fabric rustle.", "0:03", "Soft"),
    ("Paper shuffle", "Foley", "Sheets moving on a desk.", "0:03", "Soft"),
    ("Cup on table", "Foley", "A mug set down.", "0:01", "Natural"),
    ("Camera shutter", "Foley", "A photo being taken.", "0:01", "Crisp"),
    ("Rain", "Nature", "Steady rain on the ground.", "0:30", "Soft"),
    ("Thunder", "Nature", "A distant rolling rumble.", "0:08", "Deep"),
    ("Wind", "Nature", "Wind through open space.", "0:30", "Airy"),
    ("Birds", "Nature", "Morning birdsong.", "0:30", "Bright"),
    ("Waves", "Nature", "Gentle sea waves.", "0:30", "Calm"),
    ("Fire crackle", "Nature", "A small campfire.", "0:30", "Warm"),
    ("Quiet room", "Room tones", "The hush of an empty room.", "0:30", "Quiet"),
    ("Office hum", "Room tones", "Air conditioning and distant keys.", "0:30", "Steady"),
    ("Cafe chatter", "Room tones", "Murmur and cups, nothing clear.", "0:30", "Busy"),
    ("Street traffic", "Room tones", "Cars and the city outside.", "0:30", "Busy"),
    ("Crowd", "Room tones", "A large group talking and cheering.", "0:30", "Loud"),
    ("Night crickets", "Room tones", "A warm night outdoors.", "0:30", "Calm"),
    ("Laser", "Sci-fi and digital", "A bright energy shot.", "0:01", "Bright"),
    ("Power up", "Sci-fi and digital", "Energy rising in pitch.", "0:02", "Rising"),
    ("Power down", "Sci-fi and digital", "Energy falling away.", "0:02", "Falling"),
    ("Glitch", "Sci-fi and digital", "A short digital stutter.", "0:01", "Glitchy"),
    ("Data beeps", "Sci-fi and digital", "A computer chattering.", "0:03", "Busy"),
    ("Hologram", "Sci-fi and digital", "A shimmering appear sound.", "0:02", "Shimmer"),
    ("Whip transition", "Transition sounds", "Matches Whip pan and Slide.", "0:01", "Fast"),
    ("Zoom transition", "Transition sounds", "Matches Zoom and Spin.", "0:01", "Rising"),
    ("Glitch transition", "Transition sounds", "Matches Glitch and RGB split.", "0:01", "Glitchy"),
    ("Flash transition", "Transition sounds", "Matches Flash and Light leak.", "0:01", "Bright"),
    ("Fade transition", "Transition sounds", "Matches Dissolve and Fade.", "0:02", "Soft"),
    ("Page transition", "Transition sounds", "Matches Page turn and Peel.", "0:01", "Natural"),
];

/// name, group, description, length, looks like
const LIB_OVERLAYS: [(&str, &str, &str, &str, &str); 31] = [
    ("Warm leak", "Light leaks", "An orange glow sweeping over the picture.", "0:03", "Warm"),
    ("Cool leak", "Light leaks", "A blue and pink flare.", "0:03", "Cool"),
    ("Sun flare", "Light leaks", "A bright flare from one corner.", "0:04", "Bright"),
    ("Film burn", "Light leaks", "A fiery flash like a film strip burning.", "0:02", "Hot"),
    ("Fine dust", "Dust and film grain", "Small specks floating over the shot.", "0:10", "Subtle"),
    ("Heavy dust", "Dust and film grain", "Thick particles and scratches.", "0:10", "Rough"),
    ("8mm grain", "Dust and film grain", "Chunky home-movie grain.", "0:10", "Vintage"),
    ("35mm grain", "Dust and film grain", "Fine cinema grain.", "0:10", "Cinematic"),
    ("Soft bokeh", "Bokeh and particles", "Blurry round lights drifting.", "0:10", "Dreamy"),
    ("Gold sparks", "Bokeh and particles", "Shining particles rising.", "0:10", "Festive"),
    ("Floating dots", "Bokeh and particles", "Slow white dots in the air.", "0:10", "Calm"),
    ("Fireflies", "Bokeh and particles", "Tiny lights that blink and drift.", "0:10", "Magic"),
    ("Thin frame", "Frames and borders", "A simple line border.", "Still", "Clean"),
    ("Polaroid", "Frames and borders", "A white photo frame with a caption space.", "Still", "Vintage"),
    ("Film strip", "Frames and borders", "Sprocket holes on the edges.", "Still", "Vintage"),
    ("Rounded corners", "Frames and borders", "Soft-cornered screen with a dark background.", "Still", "Soft"),
    ("Glitch burst", "Glitch and VHS", "A quick block of digital noise.", "0:02", "Glitchy"),
    ("VHS tape", "Glitch and VHS", "Tracking lines and colour bleed.", "0:10", "Retro"),
    ("Old TV", "Glitch and VHS", "A curved screen with static.", "0:10", "Retro"),
    ("Moving lines", "Animated lines and shapes", "Thin lines sliding across the frame.", "0:05", "Clean"),
    ("Corner brackets", "Animated lines and shapes", "Viewfinder corners that draw on.", "0:03", "Tech"),
    ("Shape burst", "Animated lines and shapes", "Simple shapes popping outward.", "0:03", "Fun"),
    ("Rain", "Weather", "Rain streaks over the picture.", "0:10", "Moody"),
    ("Snow", "Weather", "Soft falling snow.", "0:10", "Calm"),
    ("Fog", "Weather", "Slow drifting mist.", "0:10", "Eerie"),
    ("Countdown 3-2-1", "Countdown and timer", "A clean number countdown.", "0:03", "Bold"),
    ("Timer bar", "Countdown and timer", "A bar that fills over time.", "0:10", "Clean"),
    ("Cinema bars", "Letterbox bars", "Black bars for a widescreen look.", "Still", "Cinematic"),
    ("Subscribe button", "Subscribe and like", "A generic button that slides in and taps.", "0:04", "Social"),
    ("Like and bell", "Subscribe and like", "A thumb and a bell that wiggle.", "0:03", "Social"),
    ("Follow prompt", "Subscribe and like", "A swipe-up style call to action.", "0:03", "Social"),
];

/// name, group, description, length, looks like
const LIB_TEMPLATES: [(&str, &str, &str, &str, &str); 20] = [
    ("YouTube intro", "Social and creator", "A 5-second channel opener with logo and tagline.", "0:05", "Bold"),
    ("Vlog opening", "Social and creator", "Quick cuts, big title and a date.", "0:10", "Fun"),
    ("Reel and Short hook", "Social and creator", "A vertical 9:16 hook that stops the scroll.", "0:03", "Punchy"),
    ("Podcast clip", "Social and creator", "Waveform, speaker names and live captions.", "0:30", "Clean"),
    ("Gaming highlight", "Social and creator", "Zoom, shake and flash for the big play.", "0:15", "Intense"),
    ("Unboxing", "Social and creator", "Reveal, close-up and a rating card.", "0:30", "Fun"),
    ("15-second product ad", "Business", "Hook, three benefits and a price.", "0:15", "Slick"),
    ("Event promo", "Business", "Date, place and a fast preview.", "0:30", "Energetic"),
    ("Tutorial", "Business", "Numbered steps with callouts.", "1:00", "Clear"),
    ("Launch countdown", "Business", "Days-to-go build-up with a big reveal.", "0:15", "Dramatic"),
    ("Real estate walkthrough", "Business", "Room names, size labels and a price.", "1:00", "Elegant"),
    ("Fitness promo", "Business", "Hard cuts, a timer and bold text.", "0:20", "Intense"),
    ("Interview", "Story and news", "Lower thirds, pull quotes and B-roll slots.", "2:00", "Professional"),
    ("Documentary opener", "Story and news", "Slow reveal, a big title and a year.", "0:30", "Cinematic"),
    ("Lyric video", "Story and news", "Words timed to the music.", "3:00", "Moody"),
    ("News bulletin", "Story and news", "A headline bar, a ticker and a studio frame.", "1:00", "Formal"),
    ("Memories slideshow", "Memories", "Photos with slow zoom and soft music.", "1:30", "Warm"),
    ("Travel recap", "Memories", "A map line, place names and quick clips.", "1:00", "Adventurous"),
    ("Wedding highlight", "Memories", "Gentle titles, soft light and names.", "2:00", "Romantic"),
    ("Birthday wishes", "Memories", "Balloons, a name and a message.", "0:30", "Joyful"),
];

pub fn refresh_library(st: &mut State, ui: &App) {
    let mut all: Vec<LibraryItem> = Vec::new();
    let mut id = 0;
    for (n, (name, group, desc, looks)) in PRIMITIVES.iter().enumerate() {
        all.push(LibraryItem {
            id,
            name: (*name).into(),
            kind: 8,
            kind_text: (*group).into(),
            desc: (*desc).into(),
            length: "Any".into(),
            works_on: "Titles and graphics".into(),
            looks_like: (*looks).into(),
            favorite: false,
            path: format!("primitive:{n}").into(),
            icon_id: n as i32,
            group: match *group {
                "Shape" => "Shapes",
                "Helper" => "Helpers",
                other => other,
            }
            .into(),
            col: 0,
            y: 0,
        });
        id += 1;
    }
    for (n, (name, group, desc, len, looks, path)) in TITLES.iter().enumerate() {
        all.push(LibraryItem {
            id,
            name: (*name).into(),
            kind: 1,
            kind_text: "Title".into(),
            desc: (*desc).into(),
            length: (*len).into(),
            works_on: "Any project".into(),
            looks_like: (*looks).into(),
            favorite: false,
            path: if path.is_empty() { format!("title-ui:{n}") } else { (*path).to_string() }.into(),
            icon_id: 0,
            group: (*group).into(),
            col: 0,
            y: 0,
        });
        id += 1;
    }
    for (n, (name, group, desc, len, looks, path)) in LIB_TRANSITIONS.iter().enumerate() {
        all.push(LibraryItem {
            id,
            name: (*name).into(),
            kind: 2,
            kind_text: "Transition".into(),
            desc: (*desc).into(),
            length: (*len).into(),
            works_on: "Any cut".into(),
            looks_like: (*looks).into(),
            favorite: false,
            path: if path.is_empty() { format!("transition-ui:{n}") } else { (*path).to_string() }.into(),
            icon_id: 0,
            group: (*group).into(),
            col: 0,
            y: 0,
        });
        id += 1;
    }
    let mut push = |name: &str, kind: i32, kt: &str, desc: &str, len: &str, works: &str, looks: &str, path: &str, group: &str| {
        all.push(LibraryItem {
            id,
            name: name.into(),
            kind,
            kind_text: kt.into(),
            desc: desc.into(),
            length: len.into(),
            works_on: works.into(),
            looks_like: looks.into(),
            favorite: false,
            path: path.into(),
            icon_id: 0,
            group: group.into(),
            col: 0,
            y: 0,
        });
        id += 1;
    };
    for (n, (name, group, desc, looks, real)) in LIB_EFFECTS.iter().enumerate() {
        let real_idx = crate::tools::EFFECT_TILES.iter().find(|t| t.0 == *real).map(|t| tl::EffectKind::ALL.iter().position(|x| *x == t.1).unwrap_or(0));
        push(
            name,
            3,
            "Effect",
            desc,
            "Clip length",
            "Video clips",
            looks,
            &match real_idx {
                Some(idx) => format!("effect:{idx}"),
                None => format!("effect-ui:{n}"),
            },
            group,
        );
    }
    for (kind, kt, works, ui_tag, list) in [
        (4, "Music", "Audio tracks", "music-ui", &LIB_MUSIC[..]),
        (5, "Sound effect", "Audio tracks", "sfx-ui", &LIB_SFX[..]),
        (6, "Overlay", "Video tracks", "overlay-ui", &LIB_OVERLAYS[..]),
        (7, "Template", "New projects", "template-ui", &LIB_TEMPLATES[..]),
    ] {
        for (n, (name, group, desc, len, looks)) in list.iter().enumerate() {
            push(name, kind, kt, desc, len, works, looks, &format!("{ui_tag}:{n}"), group);
        }
    }
    let lib = st.settings.storage_dir().join("Library");
    for (folder, kind, kt) in [("Music", 4, "Music"), ("Sound effects", 5, "Sound effect"), ("Overlays", 6, "Overlay"), ("Templates", 7, "Template")] {
        let dir = lib.join(folder);
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| util::is_media(p) || p.extension().is_some_and(|e| e == "originfx")).collect();
            files.sort();
            for p in files {
                let dur = st.sys.durations.get(&p).map(|d| util::short_duration(*d)).unwrap_or_else(|| "—".into());
                push(&p.file_stem().unwrap_or_default().to_string_lossy(), kind, kt, &format!("From {}", shorten_home(&dir)), &dur, if kind == 6 { "Video tracks" } else { "Audio tracks" }, kt, &p.to_string_lossy(), "My files");
            }
        }
    }
    for it in all.iter_mut() {
        it.favorite = st.settings.favorites.contains(it.path.as_str());
    }
    st.sys.library = all;
    filter_library(st, ui);
}

fn filter_library(st: &mut State, ui: &App) {
    let m = ui.global::<Model>();
    let cat = m.get_library_cat();
    let f = m.get_library_filter().to_lowercase();
    let items: Vec<LibraryItem> = st.sys.library.iter().filter(|i| (cat == 0 || i.kind == cat) && (f.is_empty() || i.name.to_lowercase().contains(&f))).cloned().collect();
    if m.get_library_pick() as usize >= items.len() {
        m.set_library_pick(0);
    }
    m.set_library(vec_model(layout_library(items, cat != 0, &m)));
}

/// Places tiles on a 4-column grid. The Primitives, Titles and Transitions views are split into sections, each
/// with its own heading and its own grid.
fn layout_library(mut items: Vec<LibraryItem>, sectioned: bool, m: &Model<'_>) -> Vec<LibraryItem> {
    const COLS: usize = 4;
    const ROW: i32 = 118;
    const HEAD: i32 = 28;
    const GAP: i32 = 14;
    let mut heads: Vec<LibraryHead> = Vec::new();
    let mut y = 0;
    if sectioned {
        let mut prev = String::new();
        let mut k = 0usize;
        let mut rows_open = false;
        for it in items.iter_mut() {
            if it.group.as_str() != prev {
                if rows_open {
                    y += (k.div_ceil(COLS) as i32) * ROW + GAP;
                }
                heads.push(LibraryHead { title: it.group.clone(), y });
                y += HEAD;
                prev = it.group.to_string();
                k = 0;
                rows_open = true;
            }
            it.col = (k % COLS) as i32;
            it.y = y + (k / COLS) as i32 * ROW;
            k += 1;
        }
        if rows_open {
            y += (k.div_ceil(COLS) as i32) * ROW;
        }
    } else {
        for (i, it) in items.iter_mut().enumerate() {
            it.col = (i % COLS) as i32;
            it.y = (i / COLS) as i32 * ROW;
        }
        y = (items.len().div_ceil(COLS) as i32) * ROW;
    }
    m.set_library_heads(vec_model(heads));
    m.set_library_h(if items.is_empty() { 80 } else { y });
    items
}

fn library_add(st: &mut State, ui: &App, i: i32) {
    let m = ui.global::<Model>();
    let Some(item) = m.get_library().row_data(i.max(0) as usize) else { return };
    let path = item.path.to_string();
    if path.starts_with("primitive:") {
        st.toast(ui, "Primitives are not wired into the engine yet.");
        return;
    }
    if path.starts_with("transition-ui:") {
        st.toast(ui, "This transition is design only for now. The engine part comes later.");
        return;
    }
    for (tag, what) in [("effect-ui:", "effect"), ("music-ui:", "track"), ("sfx-ui:", "sound"), ("overlay-ui:", "overlay"), ("template-ui:", "template")] {
        if path.starts_with(tag) {
            st.toast(ui, format!("This {what} is design only for now. The engine part comes later."));
            return;
        }
    }
    if path.starts_with("title-ui:") {
        st.toast(ui, "This title is design only for now. The engine part comes later.");
        return;
    }
    if st.session.is_none() {
        st.toast(ui, "Open or create a project first.");
        return;
    }
    if let Some(n) = path.strip_prefix("title:") {
        m.set_title_template(n.parse().unwrap_or(0));
        st.go(ui, Screen::Titles);
    } else if let Some(n) = path.strip_prefix("transition:") {
        m.set_transition_pick(n.parse().unwrap_or(0));
        st.go(ui, Screen::Transitions);
    } else if let Some(n) = path.strip_prefix("effect:") {
        st.go(ui, Screen::Editor);
        ui.global::<Backend>().invoke_add_effect(n.parse().unwrap_or(0));
    } else {
        let p = PathBuf::from(&path);
        match tl::media::probe_asset(&p) {
            Ok(a) => {
                st.edit(ui, |s| {
                    let id = s.import_asset(a)?;
                    let at = s.playhead;
                    s.insert_asset(id, at, None, None)
                });
                let a = st.session.as_ref().and_then(|s| s.project().assets().values().last().cloned());
                if let Some(a) = a {
                    crate::media::request_asset_media(st, &a);
                }
                st.go(ui, Screen::Editor);
            }
            Err(e) => st.toast(ui, format!("Could not add: {e}")),
        }
    }
}

// ---------------------------------------------------------------- settings

pub fn enter_settings(st: &mut State, ui: &App) {
    apply_settings(st, ui);
    refresh_shortcuts(st, ui);
    refresh_plugins(st, ui);
    let cache = util::dir_size(&st.settings.cache_dir());
    ui.global::<Model>().set_cache_size_text(util::bytes(cache).into());
}

fn setting_changed(st: &mut State, ui: &App, key: &str, value: &str) {
    let s = &mut st.settings;
    let num = || value.parse::<f32>().unwrap_or(0.0);
    let on = value == "true" || value == "1";
    match key {
        "autosave" => s.autosave_minutes = num() as i32,
        "default-fps" => s.frame_rate = num() as i32,
        "default-res" => s.resolution = num() as i32,
        "timecode" => {
            s.timecode_style = num() as i32;
            ui.global::<Settings>().set_timecode_style(s.timecode_style);
        }
        "hw-accel" => s.hw_accel = on,
        "crash-reports" => s.crash_reports = on,
        "theme" => {
            s.look = match num() as i32 {
                0 => 2,
                1 => 1,
                _ => 0,
            };
            ui.global::<Settings>().set_look(s.look);
        }
        "layout" => {
            s.workspace = if num() as i32 == 1 { 2 } else { 0 };
            ui.global::<Settings>().set_mode(s.workspace);
        }
        "ui-scale" => s.ui_scale = num(),
        "hints" => {
            s.hints = on;
            ui.global::<Settings>().set_hints(on);
        }
        "reduce-motion" => {
            s.reduce_motion = on;
            ui.global::<Settings>().set_reduce_motion(on);
        }
        "governor" => s.governor = num() as i32,
        "memory-limit" => s.memory_limit_gb = num().round(),
        "background-tasks" => s.background_tasks = on,
        "cache-limit" => s.cache_limit_gb = num(),
        "update-channel" => s.update_channel = num() as i32,
        "auto-updates" => s.auto_updates = on,
        "lesson-done" => {
            if let Some(l) = st.sys.lesson.take() {
                st.settings.lessons_done.insert(l);
                st.settings.save();
                refresh_lessons(st, ui);
                st.toast(ui, "Lesson complete.");
            }
            return;
        }
        _ => {}
    }
    st.settings.save();
    apply_settings(st, ui);
}

// ---------------------------------------------------------------- updates

fn newer(remote: &str, local: &str) -> bool {
    let p = |s: &str| s.trim_start_matches('v').split(['.', '-']).take(3).map(|x| x.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
    p(remote) > p(local)
}

fn check_updates(st: &mut State, ui: &App, quiet: bool) {
    let beta = st.settings.update_channel == 1;
    if !quiet {
        ui.global::<Model>().set_update_status("Checking…".into());
    }
    std::thread::spawn(move || {
        let url = "https://api.github.com/repos/FX-Foundation/originfx/releases";
        let r = ureq::get(url).set("User-Agent", "OriginFX").timeout(Duration::from_secs(10)).call();
        let result: Result<Option<(String, String)>, String> = match r {
            Ok(resp) => match resp.into_json::<serde_json::Value>() {
                Ok(v) => {
                    let best = v
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|r| beta || !r["prerelease"].as_bool().unwrap_or(false))
                        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
                        .find_map(|r| Some((r["tag_name"].as_str()?.to_string(), r["html_url"].as_str()?.to_string())));
                    Ok(best)
                }
                Err(e) => Err(e.to_string()),
            },
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(e) => Err(e.to_string()),
        };
        later(move |st, ui| {
            let m = ui.global::<Model>();
            st.settings.last_update_check = chrono::Local::now().format("%d %b, %H:%M").to_string();
            st.settings.save();
            m.set_update_checked(format!("Last checked {}.", st.settings.last_update_check).into());
            match result {
                Ok(Some((tag, url))) if newer(&tag, VERSION) => {
                    m.set_update_available(true);
                    m.set_update_url(url.into());
                    m.set_update_status(format!("OriginFX {} is available.", tag.trim_start_matches('v')).into());
                    if quiet {
                        st.toast(ui, format!("OriginFX {} is available. See Settings, Updates.", tag.trim_start_matches('v')));
                    }
                }
                Ok(_) => {
                    m.set_update_available(false);
                    m.set_update_status("You are up to date.".into());
                }
                Err(e) => {
                    if !quiet {
                        m.set_update_status(format!("Could not check: {e}").into());
                    }
                }
            }
        });
    });
}

// ---------------------------------------------------------------- interchange

pub fn import_timeline(st: &mut State, ui: &App, path: &Path, fmt: i32) {
    let m = ui.global::<Model>();
    let mut dirs = vec![path.parent().unwrap_or(Path::new(".")).to_path_buf()];
    if m.get_xchg_relink() {
        dirs.push(st.settings.storage_dir());
        if let Some(v) = dirs::video_dir() {
            dirs.push(v);
        }
    }
    match crate::xchg::import(path, fmt, &dirs, m.get_xchg_markers_in()) {
        Ok((ed, seq, offline)) => {
            let name = ed.project().name.clone();
            let mut s = Session::new(ed, seq, None);
            let p = new_project_path(st, &name);
            let _ = s.save(&p);
            st.load_session(ui, s);
            after_save(st, ui);
            ui.global::<Model>().set_xchg_status(
                if offline > 0 { format!("Imported. {offline} files were not found; relink them in the Media inspector.") } else { "Imported.".to_string() }.into(),
            );
            st.go(ui, Screen::Editor);
        }
        Err(e) => {
            ui.global::<Model>().set_xchg_status(format!("Could not import: {e}").into());
            st.toast(ui, format!("Could not import: {e}"));
        }
    }
}

pub fn export_timeline(st: &mut State, ui: &App, path: &Path, fmt: i32) {
    let m = ui.global::<Model>();
    let Some(s) = &st.session else { return };
    let opt = crate::xchg::Options { media_refs: m.get_xchg_media_refs(), markers: m.get_xchg_markers_out() };
    match crate::xchg::export(s.project(), s.sequence(), path, fmt, &opt) {
        Ok(()) => {
            m.set_xchg_status(format!("Saved {}", shorten_home(path)).into());
            st.toast(ui, format!("Saved {}", shorten_home(path)));
        }
        Err(e) => st.toast(ui, format!("Could not export: {e}")),
    }
}

// ---------------------------------------------------------------- wiring

pub fn wire(ui: &App) {
    let b = ui.global::<Backend>();

    b.on_set_mode(|m| {
        with(|st, ui| {
            ui.global::<Settings>().set_mode(m);
            st.settings.workspace = m;
            st.settings.save();
            ui.global::<Model>().set_layout_choice(if m == 2 { 1 } else { 0 });
        });
    });
    b.on_set_look(|l| {
        with(|st, ui| {
            ui.global::<Settings>().set_look(l);
            st.settings.look = l;
            st.settings.save();
        });
    });
    b.on_finish_setup(|workspace, look, res, fps, aspect, storage, action| {
        with(|st, ui| {
            let s = &mut st.settings;
            s.workspace = workspace;
            s.look = look;
            s.resolution = res;
            s.frame_rate = fps;
            s.aspect = aspect;
            s.storage = storage.to_string();
            s.setup_done = true;
            s.cache_location = shorten_home(&expand_home(&s.storage).join("Cache"));
            s.export_folder = shorten_home(&expand_home(&s.storage).join("Exports"));
            s.save();
            apply_settings(st, ui);
            match action.as_str() {
                "editor" => {
                    let code = res * 10 + fps + aspect * 1000 + 100;
                    create_project(st, ui, "Untitled project", None, code);
                }
                "project" => {
                    ui.global::<Model>().set_screen(Screen::NewProject);
                }
                _ => {
                    ui.global::<Model>().set_screen(Screen::Home);
                    refresh_recent(st, ui);
                }
            }
        });
    });
    b.on_new_project(|name, folder, code| {
        with(|st, ui| {
            let f = if folder.is_empty() { None } else { Some(expand_home(folder.as_str())) };
            create_project(st, ui, name.as_str(), f, code);
        });
    });
    b.on_open_project_dialog(|| {
        let dir = with(|st, _| st.settings.storage_dir()).unwrap_or_default();
        if let Some(p) = rfd::FileDialog::new().set_title("Open project").add_filter("OriginFX project", &["originfx"]).set_directory(dir).pick_file() {
            with(|st, ui| open_project(st, ui, &p));
        }
    });
    b.on_open_recent(|p| {
        with(|st, ui| open_project(st, ui, Path::new(p.as_str())));
    });
    b.on_save_project(|| {
        let need_path = with(|st, ui| {
            let Some(s) = st.session.as_mut() else { return false };
            match s.path.clone() {
                Some(p) => {
                    match s.save(&p) {
                        Ok(()) => {
                            after_save(st, ui);
                            st.toast(ui, "Saved.");
                        }
                        Err(e) => st.toast(ui, format!("Could not save: {e}")),
                    }
                    false
                }
                None => true,
            }
        })
        .unwrap_or(false);
        if need_path {
            with(|_, ui| ui.global::<Backend>().invoke_save_project_as());
        }
    });
    b.on_save_project_as(|| {
        let (dir, name) = with(|st, _| (st.settings.storage_dir(), st.session.as_ref().map(|s| s.name()).unwrap_or_default())).unwrap_or_default();
        if let Some(p) = rfd::FileDialog::new().set_title("Save project").add_filter("OriginFX project", &["originfx"]).set_directory(dir).set_file_name(format!("{name}.originfx")).save_file() {
            with(|st, ui| {
                if let Some(s) = st.session.as_mut() {
                    match s.save(&p) {
                        Ok(()) => {
                            after_save(st, ui);
                            st.toast(ui, "Saved.");
                        }
                        Err(e) => st.toast(ui, format!("Could not save: {e}")),
                    }
                }
            });
        }
    });
    b.on_close_project(|| {
        with(|st, ui| {
            if let Some(s) = st.session.as_mut() {
                if let Some(p) = s.path.clone() {
                    let _ = s.save(&p);
                }
            }
            st.session = None;
            st.worker = None;
            ui.global::<Model>().set_has_project(false);
            ui.global::<Model>().set_screen(Screen::Home);
            refresh_recent(st, ui);
        });
    });
    b.on_set_sequence(|w, h, fps, aspect| {
        with(|st, ui| {
            st.edit(ui, |s| s.set_sequence_settings(w as u32, h as u32, fps as f64, aspect));
            st.toast(ui, "Project settings applied.");
            st.back(ui);
        });
    });
    b.on_restore_recovery(|i| {
        with(|st, ui| {
            let Some(p) = st.sys.recovery.get(i.max(0) as usize).cloned() else { return };
            match Session::open(&p) {
                Ok(mut s) => {
                    // keep it unsaved so the person chooses where it goes
                    let name = s.name();
                    let target = st.settings.recent.iter().find(|r| r.name == name).map(|r| PathBuf::from(&r.path));
                    s.path = target;
                    st.load_session(ui, s);
                    if let Some(sess) = st.session.as_mut() {
                        sess.bump();
                    }
                    ui.global::<Model>().set_screen(Screen::Editor);
                    st.nav = vec![Screen::Home];
                    st.toast(ui, "Restored. Save to keep it.");
                }
                Err(e) => st.toast(ui, format!("Could not restore: {e}")),
            }
        });
    });
    b.on_open_without_restoring(|| {
        with(|st, ui| {
            ui.global::<Model>().set_screen(if st.settings.setup_done { Screen::Home } else { Screen::Welcome });
            refresh_recent(st, ui);
        });
    });
    b.on_discard_recovery(|| {
        with(|st, ui| {
            for p in st.sys.recovery.drain(..) {
                let _ = std::fs::remove_file(p);
            }
            ui.global::<Model>().set_screen(if st.settings.setup_done { Screen::Home } else { Screen::Welcome });
            refresh_recent(st, ui);
        });
    });
    b.on_start_lesson(|i| {
        with(|st, ui| {
            let i = i as usize;
            if st.session.is_none() {
                let code = st.settings.resolution * 10 + st.settings.frame_rate + st.settings.aspect * 1000 + 100;
                create_project(st, ui, "Lesson project", None, code);
            }
            let text = if i >= 100 {
                ["Put the playhead where you want to cut, then press K.", "Open Titles from the Tools menu, write your words, Add to timeline.", "Select the second of two touching clips, open Transitions, apply Cross dissolve.", "Press Ctrl+E, pick YouTube, then Export."][(i - 100).min(3)].to_string()
            } else {
                st.sys.lesson = Some(i);
                let (t, s) = LESSONS[i.min(LESSONS.len() - 1)];
                format!("Lesson {}: {t}. {s}", i + 1)
            };
            ui.global::<Model>().set_hint_banner(text.into());
            st.go(ui, Screen::Editor);
            ui.global::<Model>().set_screen(Screen::Editor);
        });
    });
    b.on_search_projects(|t| {
        with(|st, ui| {
            st.sys.recent_filter = t.to_string();
            refresh_recent(st, ui);
        });
    });
    b.on_browse_folder(|_key| rfd::FileDialog::new().set_title("Choose a folder").pick_folder().map(|p| SharedString::from(shorten_home(&p))).unwrap_or_default());
    b.on_library_add(|i| {
        with(|st, ui| library_add(st, ui, i));
    });
    b.on_library_favorite(|i| {
        with(|st, ui| {
            let m = ui.global::<Model>();
            if let Some(item) = m.get_library().row_data(i.max(0) as usize) {
                let p = item.path.to_string();
                if !st.settings.favorites.remove(&p) {
                    st.settings.favorites.insert(p);
                }
                st.settings.save();
                refresh_library(st, ui);
            }
        });
    });
    b.on_library_filter(|_| {
        with(|st, ui| filter_library(st, ui));
    });
    b.on_library_select(|_| {});
    b.on_library_download(|_| {
        with(|st, ui| st.toast(ui, "Content packs are design only for now. Downloads come with the engine part."));
    });

    b.on_setting_changed(|k, v| {
        with(|st, ui| setting_changed(st, ui, k.as_str(), v.as_str()));
    });
    b.on_clear_app_cache(|| {
        with(|st, ui| {
            let dir = st.settings.cache_dir();
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let _ = if e.path().is_dir() { std::fs::remove_dir_all(e.path()) } else { std::fs::remove_file(e.path()) };
                }
            }
            st.jobs.proxies.clear();
            if let Some(s) = st.session.as_mut() {
                s.bump();
            }
            ui.global::<Model>().set_cache_size_text(util::bytes(util::dir_size(&dir)).into());
            st.toast(ui, "Cache cleared.");
        });
    });
    b.on_set_shortcut_preset(|p| {
        with(|st, ui| {
            st.settings.shortcut_preset = p;
            st.settings.shortcut_overrides.clear();
            st.settings.save();
            refresh_shortcuts(st, ui);
        });
    });
    b.on_rebind(|id| {
        with(|_, ui| {
            ui.global::<Model>().set_rebinding(id);
            crate::bump_focus(ui);
        });
    });
    b.on_rebind_key(|_| {});
    b.on_filter_shortcuts(|t| {
        with(|st, ui| {
            ui.global::<Model>().set_shortcut_filter(t);
            refresh_shortcuts(st, ui);
        });
    });
    b.on_toggle_plugin(|id, on| {
        with(|st, ui| {
            let id = id.to_string();
            if on {
                st.settings.plugins_disabled.remove(&id);
            } else {
                st.settings.plugins_disabled.insert(id);
            }
            st.settings.save();
            rebuild_plugins(st);
            refresh_plugins(st, ui);
        });
    });
    b.on_install_plugin(|| {
        if let Some(p) = rfd::FileDialog::new().set_title("Install a plugin").add_filter("OriginFX plugin", &["wasm"]).pick_file() {
            with(|st, ui| match Plugin::load_file(&p) {
                Ok(pl) => {
                    let dst = AppSettings::plugins_dir().join(p.file_name().unwrap_or_default());
                    match std::fs::copy(&p, &dst) {
                        Ok(_) => {
                            rebuild_plugins(st);
                            refresh_plugins(st, ui);
                            st.toast(ui, format!("Installed {}.", pl.manifest().name));
                        }
                        Err(e) => st.toast(ui, format!("Could not install: {e}")),
                    }
                }
                Err(e) => st.toast(ui, format!("That is not an OriginFX plugin: {e}")),
            });
        }
    });
    b.on_browse_plugins(|| {
        let _ = open::that_detached(format!("{REPO}/tree/main/crates/plugin-engine"));
    });
    b.on_check_updates(|| {
        with(|st, ui| check_updates(st, ui, false));
    });
    b.on_open_update(|| {
        with(|_, ui| {
            let u = ui.global::<Model>().get_update_url();
            let _ = open::that_detached(if u.is_empty() { format!("{REPO}/releases") } else { u.to_string() });
        });
    });
    b.on_open_link(|what| {
        let url = match what.as_str() {
            "source" => REPO.to_string(),
            "docs" => format!("{REPO}/tree/main/docs"),
            "issue" => format!("{REPO}/issues/new"),
            "licenses" => format!("{REPO}/blob/main/THIRD_PARTY.md"),
            "license" => format!("{REPO}/blob/main/LICENSE"),
            other => other.to_string(),
        };
        let _ = open::that_detached(url);
    });
}

pub fn _unused(_: SharedString) {
    let _ = strings(Vec::<String>::new());
}
