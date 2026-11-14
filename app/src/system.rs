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
use crate::{later, strings, util, vec_model, with, App, Backend, LessonItem, LibraryItem, Model, PluginItem, RecentProject, Row4, Screen, Settings, ShortcutItem, State};

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
pub const SHORTCUTS: [(&str, &str, [&str; 3]); 22] = [
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

pub fn refresh_library(st: &mut State, ui: &App) {
    let mut all: Vec<LibraryItem> = Vec::new();
    let mut id = 0;
    let mut push = |name: &str, kind: i32, kt: &str, desc: &str, len: &str, works: &str, looks: &str, path: &str| {
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
        });
        id += 1;
    };
    for (i, t) in ["Lower third · Bar", "Lower third · Minimal", "Title · Centered", "Title · Split"].iter().enumerate() {
        push(t, 1, "Title", "Animated words on screen. Change the text, size and timing after adding it.", "5 seconds", "Any project", ["Broadcast", "Clean", "Cinematic", "Editorial"][i], &format!("title:{i}"));
    }
    for (i, (n, _)) in crate::session::TRANSITIONS.iter().enumerate() {
        push(n, 2, "Transition", "Goes between two clips that touch.", "1 second", "Any cut", "Smooth", &format!("transition:{i}"));
    }
    for (n, k, cat, _) in crate::tools::EFFECT_TILES.iter() {
        let idx = tl::EffectKind::ALL.iter().position(|x| x == k).unwrap_or(0);
        push(n, 3, "Effect", &format!("{cat} effect for pictures. Every setting can be keyframed."), "Clip length", "Video clips", cat, &format!("effect:{idx}"));
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
                push(&p.file_stem().unwrap_or_default().to_string_lossy(), kind, kt, &format!("From {}", shorten_home(&dir)), &dur, if kind == 6 { "Video tracks" } else { "Audio tracks" }, kt, &p.to_string_lossy());
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
    m.set_library(vec_model(items));
}

fn library_add(st: &mut State, ui: &App, i: i32) {
    let m = ui.global::<Model>();
    let Some(item) = m.get_library().row_data(i.max(0) as usize) else { return };
    let path = item.path.to_string();
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
