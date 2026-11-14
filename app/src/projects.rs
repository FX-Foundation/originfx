// SPDX-License-Identifier: GPL-3.0-or-later
//! Actions on one project from the right-click menu on the Home screen.

use std::path::{Path, PathBuf};

use slint::ComponentHandle;
use timeline_engine as tl;

use crate::session::Session;
use crate::settings::{shorten_home, AppSettings};
use crate::system::{new_project_path, open_project, refresh_recent};
use crate::{later, util, vec_model, with, App, Backend, Model, Screen, State, KV};

/// The project's own name, falling back to the file name.
fn project_name(path: &Path) -> String {
    tl::Project::load(path)
        .map(|p| p.name)
        .unwrap_or_else(|_| path.file_stem().unwrap_or_default().to_string_lossy().to_string())
}

/// Autosaved copies of a project, newest first.
fn recovery_files(name: &str) -> Vec<PathBuf> {
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    let prefix = format!("{safe}-");
    let mut files: Vec<(PathBuf, std::time::SystemTime)> = std::fs::read_dir(AppSettings::recovery_dir())
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
                .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?)))
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    files.into_iter().map(|f| f.0).collect()
}

fn clean_name(name: &str) -> String {
    name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

fn aspect_of(st: &State, path: &str) -> String {
    st.settings.recent.iter().find(|r| r.path == path).map(|r| r.aspect.clone()).unwrap_or_else(|| "16:9".into())
}

fn rename(st: &mut State, ui: &App, path: &str, new_name: &str) {
    let new_name = clean_name(new_name);
    if new_name.is_empty() {
        st.toast(ui, "Enter a name for the project.");
        return;
    }
    let mut project = match tl::Project::load(path) {
        Ok(p) => p,
        Err(_) => {
            st.toast(ui, "Could not open the project to rename it.");
            return;
        }
    };
    if project.name == new_name {
        return;
    }
    // Only the name changes. The file and folder stay where they are, so the media links keep working.
    project.name = new_name.clone();
    if let Err(e) = project.save(path) {
        st.toast(ui, format!("Could not rename: {e}"));
        return;
    }
    for r in st.settings.recent.iter_mut().filter(|r| r.path == path) {
        r.name = new_name.clone();
    }
    st.settings.save();
    refresh_recent(st, ui);
    st.toast(ui, "Project renamed.");
}

fn duplicate(st: &mut State, ui: &App, path: &str) {
    let mut project = match tl::Project::load(path) {
        Ok(p) => p,
        Err(_) => {
            st.toast(ui, "Could not open the project to duplicate it.");
            return;
        }
    };
    project.name = format!("{} copy", project.name);
    let target = new_project_path(st, &project.name);
    if let Err(e) = project.save(&target) {
        st.toast(ui, format!("Could not duplicate: {e}"));
        return;
    }
    let aspect = aspect_of(st, path);
    st.settings.touch_recent(&project.name, &target.to_string_lossy(), &aspect);
    st.settings.save();
    refresh_recent(st, ui);
    st.toast(ui, "Project duplicated.");
}

fn reveal(st: &mut State, ui: &App, path: &str) {
    let dir = Path::new(path).parent().map(Path::to_path_buf).unwrap_or_default();
    if open::that(&dir).is_err() {
        st.toast(ui, "Could not open the folder.");
    }
}

fn remove_from_recent(st: &mut State, ui: &App, path: &str) {
    st.settings.recent.retain(|r| r.path != path);
    if !st.settings.hidden.iter().any(|h| h == path) {
        st.settings.hidden.push(path.to_string());
    }
    st.settings.save();
    refresh_recent(st, ui);
    st.toast(ui, "Removed from Recent. The project is still on your computer.");
}

fn trash(st: &mut State, ui: &App, path: &str) {
    let file = PathBuf::from(path);
    // A project made by OriginFX lives in its own folder, with its copied media. Trash the whole folder.
    let stem = file.file_stem().map(|s| s.to_os_string());
    let dir = file.parent().map(Path::to_path_buf);
    let storage = st.settings.storage_dir();
    let target = match dir {
        Some(d) if d.file_name().map(|n| n.to_os_string()) == stem && d.parent() == Some(storage.as_path()) => d,
        _ => file.clone(),
    };
    match trash::delete(&target) {
        Ok(()) => {
            st.settings.recent.retain(|r| r.path != path);
            st.settings.hidden.retain(|h| h != path);
            st.settings.save();
            refresh_recent(st, ui);
            st.toast(ui, "Moved to the Trash.");
        }
        Err(e) => st.toast(ui, format!("Could not move to the Trash: {e}")),
    }
}

fn copy_path(st: &mut State, ui: &App, path: &str) {
    match arboard::Clipboard::new().and_then(|mut c| c.set_text(path.to_string())) {
        Ok(()) => st.toast(ui, "Path copied."),
        Err(_) => st.toast(ui, "Could not reach the clipboard."),
    }
}

fn properties(st: &mut State, ui: &App, path: &str) {
    let p = Path::new(path);
    let mut rows: Vec<(String, String)> = Vec::new();
    match tl::Project::load(p) {
        Ok(project) => {
            rows.push(("Name".into(), project.name.clone()));
            rows.push(("Location".into(), shorten_home(p.parent().unwrap_or(p))));
            if let Ok(meta) = std::fs::metadata(p) {
                if let Some(t) = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()) {
                    rows.push(("Last edited".into(), util::relative_date(t.as_secs() as i64)));
                }
                rows.push(("File size".into(), util::bytes(meta.len())));
            }
            rows.push(("Media files".into(), project.assets().len().to_string()));
            let secs = project.sequences().values().map(|s| s.duration().as_secs_f64()).fold(0.0, f64::max);
            let (m, s) = ((secs / 60.0).floor() as i64, (secs % 60.0).floor() as i64);
            rows.push(("Length".into(), format!("{m}:{s:02}")));
        }
        Err(_) => {
            rows.push(("Name".into(), p.file_stem().unwrap_or_default().to_string_lossy().to_string()));
            rows.push(("Location".into(), shorten_home(p.parent().unwrap_or(p))));
            rows.push(("Status".into(), "Could not read this project.".into()));
        }
    }
    let items: Vec<KV> = rows.into_iter().map(|(k, v)| KV { k: k.into(), v: v.into() }).collect();
    ui.global::<Model>().set_project_props(vec_model(items));
    let _ = st;
}

fn open_recovery(st: &mut State, ui: &App, path: &str) {
    let name = project_name(Path::new(path));
    let Some(copy) = recovery_files(&name).into_iter().next() else {
        st.toast(ui, "There is no autosaved copy of this project.");
        return;
    };
    match Session::open(&copy) {
        Ok(mut s) => {
            // Saving writes the real project, not the autosave.
            s.path = Some(PathBuf::from(path));
            st.load_session(ui, s);
            st.nav.clear();
            st.nav.push(Screen::Home);
            ui.global::<Model>().set_screen(Screen::Editor);
            crate::bump_focus(ui);
            st.toast(ui, "Opened the latest autosave. Save to keep it.");
        }
        Err(e) => st.toast(ui, format!("Could not open the autosave: {e}")),
    }
}

pub fn wire(ui: &App) {
    let b = ui.global::<Backend>();
    b.on_project_menu_opened(|p| {
        with(|_, ui| {
            let has = !recovery_files(&project_name(Path::new(p.as_str()))).is_empty();
            ui.global::<Model>().set_menu_recovery(has);
        });
    });
    b.on_project_rename(|p, n| {
        with(|st, ui| rename(st, ui, p.as_str(), n.as_str()));
    });
    b.on_project_duplicate(|p| {
        with(|st, ui| duplicate(st, ui, p.as_str()));
    });
    b.on_project_reveal(|p| {
        with(|st, ui| reveal(st, ui, p.as_str()));
    });
    b.on_project_remove_recent(|p| {
        with(|st, ui| remove_from_recent(st, ui, p.as_str()));
    });
    b.on_project_trash(|p| {
        with(|st, ui| trash(st, ui, p.as_str()));
    });
    b.on_project_copy_path(|p| {
        with(|st, ui| copy_path(st, ui, p.as_str()));
    });
    b.on_project_properties(|p| {
        with(|st, ui| properties(st, ui, p.as_str()));
    });
    b.on_project_export(|p| {
        let path = p.to_string();
        with(|st, ui| open_project(st, ui, Path::new(&path)));
        later(|st, ui| st.go(ui, Screen::Export));
    });
    b.on_project_open_recovery(|p| {
        with(|st, ui| open_recovery(st, ui, p.as_str()));
    });
}
