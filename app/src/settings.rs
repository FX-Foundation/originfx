// SPDX-License-Identifier: GPL-3.0-or-later
//! App-wide preferences, saved as JSON in the user's config directory.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct AppSettings {
    pub setup_done: bool,
    /// 0 Simple, 1 Guided, 2 Pro
    pub workspace: i32,
    /// 0 System, 1 Light, 2 Dark
    pub look: i32,
    /// 0 720p, 1 1080p, 2 4K
    pub resolution: i32,
    /// 0 24, 1 30, 2 60
    pub frame_rate: i32,
    /// 0 16:9, 1 9:16, 2 1:1, 3 21:9
    pub aspect: i32,
    pub storage: String,
    pub autosave_minutes: i32,
    /// 0 frames, 1 seconds
    pub timecode_style: i32,
    pub hw_accel: bool,
    pub crash_reports: bool,
    pub ui_scale: f32,
    pub hints: bool,
    pub reduce_motion: bool,
    /// 0 Quiet, 1 Balanced, 2 Maximum
    pub governor: i32,
    pub memory_limit_gb: f32,
    pub background_tasks: bool,
    pub cache_limit_gb: f32,
    pub cache_location: String,
    /// 0 auto, 1 always, 2 off
    pub proxy_mode: i32,
    /// 0 quarter, 1 half
    pub proxy_res: i32,
    /// 0 OriginFX, 1 Classic, 2 Compact
    pub shortcut_preset: i32,
    pub shortcut_overrides: BTreeMap<String, String>,
    pub plugins_enabled: BTreeSet<String>,
    pub plugins_disabled: BTreeSet<String>,
    /// 0 stable, 1 beta
    pub update_channel: i32,
    pub auto_updates: bool,
    pub last_update_check: String,
    pub recent: Vec<RecentEntry>,
    /// Projects the user removed from the Recent list (the files stay on disk).
    pub hidden: Vec<String>,
    pub lessons_done: BTreeSet<usize>,
    pub favorites: BTreeSet<String>,
    pub clean_exit: bool,
    pub export_folder: String,
    pub last_import_dir: String,
    pub language: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RecentEntry {
    pub name: String,
    pub path: String,
    pub edited_unix: i64,
    pub aspect: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            setup_done: false,
            workspace: 0,
            look: 0,
            resolution: 1,
            frame_rate: 1,
            aspect: 0,
            storage: "~/Videos/OriginFX".into(),
            autosave_minutes: 2,
            timecode_style: 0,
            hw_accel: true,
            crash_reports: false,
            ui_scale: 100.0,
            hints: true,
            reduce_motion: false,
            governor: 1,
            memory_limit_gb: 12.0,
            background_tasks: true,
            cache_limit_gb: 50.0,
            cache_location: String::new(),
            proxy_mode: 0,
            proxy_res: 1,
            shortcut_preset: 0,
            shortcut_overrides: BTreeMap::new(),
            plugins_enabled: BTreeSet::new(),
            plugins_disabled: BTreeSet::new(),
            update_channel: 0,
            auto_updates: true,
            last_update_check: String::new(),
            recent: Vec::new(),
            hidden: Vec::new(),
            lessons_done: BTreeSet::new(),
            favorites: BTreeSet::new(),
            clean_exit: true,
            export_folder: String::new(),
            last_import_dir: String::new(),
            language: "English".into(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("OriginFX");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix('~') {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest.trim_start_matches(['/', '\\']));
        }
    }
    PathBuf::from(path)
}

pub fn shorten_home(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            let r = rest.to_string_lossy();
            return if r.is_empty() { "~".into() } else { format!("~/{}", r.replace('\\', "/")) };
        }
    }
    path.to_string_lossy().into_owned()
}

impl AppSettings {
    pub fn path() -> PathBuf {
        config_dir().join("settings.json")
    }

    pub fn load() -> AppSettings {
        let mut s: AppSettings = std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        // Migrate the first-run setup file written by the welcome-only build.
        if !s.setup_done {
            let old = config_dir().join("setup.json");
            if let Ok(t) = std::fs::read_to_string(&old) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    let idx = |key: &str, names: &[&str]| -> Option<i32> {
                        let val = v.get(key)?.as_str()?;
                        names.iter().position(|n| *n == val).map(|p| p as i32)
                    };
                    if let Some(w) = idx("workspace", &["Simple", "Guided", "Pro"]) { s.workspace = w; }
                    if let Some(l) = idx("appearance", &["System", "Light", "Dark"]) { s.look = l; }
                    if let Some(r) = idx("resolution", &["720p", "1080p", "4K"]) { s.resolution = r; }
                    if let Some(f) = idx("frame_rate", &["24", "30", "60"]) { s.frame_rate = f; }
                    if let Some(a) = idx("aspect_ratio", &["16:9", "9:16", "1:1", "21:9"]) { s.aspect = a; }
                    if let Some(st) = v.get("storage").and_then(|x| x.as_str()) { s.storage = st.to_string(); }
                    s.setup_done = true;
                }
            }
        }
        if s.cache_location.is_empty() {
            s.cache_location = shorten_home(&expand_home(&s.storage).join("Cache"));
        }
        if s.export_folder.is_empty() {
            s.export_folder = shorten_home(&expand_home(&s.storage).join("Exports"));
        }
        s
    }

    pub fn save(&self) {
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), text);
        }
    }

    pub fn storage_dir(&self) -> PathBuf {
        let p = expand_home(&self.storage);
        let _ = std::fs::create_dir_all(&p);
        p
    }

    pub fn cache_dir(&self) -> PathBuf {
        let p = expand_home(&self.cache_location);
        let _ = std::fs::create_dir_all(&p);
        p
    }

    pub fn proxy_dir(&self) -> PathBuf {
        let p = self.cache_dir().join("Proxies");
        let _ = std::fs::create_dir_all(&p);
        p
    }

    pub fn recovery_dir() -> PathBuf {
        let p = config_dir().join("recovery");
        let _ = std::fs::create_dir_all(&p);
        p
    }

    pub fn plugins_dir() -> PathBuf {
        let p = config_dir().join("plugins");
        let _ = std::fs::create_dir_all(&p);
        p
    }

    pub fn touch_recent(&mut self, name: &str, path: &str, aspect: &str) {
        self.hidden.retain(|h| h != path);
        self.recent.retain(|r| r.path != path);
        self.recent.insert(
            0,
            RecentEntry {
                name: name.to_string(),
                path: path.to_string(),
                edited_unix: chrono::Local::now().timestamp(),
                aspect: aspect.to_string(),
            },
        );
        self.recent.truncate(30);
    }

    pub fn default_fps(&self) -> f64 {
        match self.frame_rate {
            0 => 24.0,
            2 => 60.0,
            _ => 30.0,
        }
    }

    pub fn default_size(&self) -> (u32, u32) {
        crate::util::frame_size(self.resolution, self.aspect)
    }
}
