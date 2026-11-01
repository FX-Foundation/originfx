// OriginFX: first-run setup screens (steps 1 to 3). GPL-3.0.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

slint::include_modules!();

use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize)]
struct SetupChoices {
    workspace: String,
    appearance: String,
    resolution: String,
    frame_rate: String,
    aspect_ratio: String,
    storage: String,
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix('~') {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest.trim_start_matches(['/', '\\']));
        }
    }
    PathBuf::from(path)
}

fn pick(names: &[&str], index: i32) -> String {
    names.get(index.max(0) as usize).copied().unwrap_or("").to_string()
}

fn save_setup(ui: &App) -> std::io::Result<PathBuf> {
    let choices = SetupChoices {
        workspace: pick(&["Simple", "Guided", "Pro"], ui.get_workspace()),
        appearance: pick(&["System", "Light", "Dark"], ui.global::<Settings>().get_look()),
        resolution: pick(&["720p", "1080p", "4K"], ui.get_resolution()),
        frame_rate: pick(&["24", "30", "60"], ui.get_fps()),
        aspect_ratio: pick(&["16:9", "9:16", "1:1", "21:9"], ui.get_aspect()),
        storage: expand_home(ui.get_storage().as_str()).to_string_lossy().into_owned(),
    };
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("OriginFX");
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("setup.json");
    std::fs::write(&file, serde_json::to_string_pretty(&choices).unwrap())?;
    Ok(file)
}

/// Turns a raw OpenGL renderer string into a name a person would recognise.
fn friendly_gpu_name(raw: &str) -> String {
    let mut name = raw.split('/').next().unwrap_or(raw).trim().to_string();
    for junk in ["(R)", "(TM)", "(tm)", "Mesa ", "ANGLE (", "Direct3D11 vs_5_0 ps_5_0", "OpenGL Engine"] {
        name = name.replace(junk, "");
    }
    name.split_whitespace().collect::<Vec<_>>().join(" ").trim_matches(|c| c == ',' || c == ')').trim().to_string()
}

fn is_software_renderer(raw: &str) -> bool {
    let lower = raw.to_lowercase();
    ["llvmpipe", "softpipe", "swrast", "software", "basic render", "swiftshader"]
        .iter()
        .any(|s| lower.contains(s))
}

fn report_gpu(weak: slint::Weak<App>, raw: Option<String>) {
    let _ = weak.upgrade_in_event_loop(move |ui| match raw {
        Some(raw) if !is_software_renderer(&raw) && !raw.trim().is_empty() => {
            ui.set_gpu_title("Ready for smooth editing".into());
            ui.set_gpu_line(format!("{} found. Hardware acceleration is on.", friendly_gpu_name(&raw)).into());
        }
        _ => {
            ui.set_gpu_title("Running on your processor".into());
            ui.set_gpu_line("No graphics card found. Hardware acceleration is off.".into());
        }
    });
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = App::new()?;

    // Ask the graphics driver which GPU is drawing this window.
    let gpu_weak = ui.as_weak();
    let notifier = ui.window().set_rendering_notifier(move |state, api| {
        if let (slint::RenderingState::RenderingSetup, slint::GraphicsAPI::NativeOpenGL { get_proc_address }) = (state, api) {
            const GL_RENDERER: u32 = 0x1F01;
            let raw = unsafe {
                let ptr = get_proc_address(c"glGetString");
                if ptr.is_null() {
                    None
                } else {
                    let gl_get_string: extern "system" fn(u32) -> *const std::os::raw::c_char =
                        std::mem::transmute(ptr);
                    let s = gl_get_string(GL_RENDERER);
                    if s.is_null() {
                        None
                    } else {
                        Some(std::ffi::CStr::from_ptr(s).to_string_lossy().into_owned())
                    }
                }
            };
            report_gpu(gpu_weak.clone(), raw);
        }
    });
    if notifier.is_err() {
        // Software renderer: there is no GPU in play.
        report_gpu(ui.as_weak(), None);
    }

    let weak = ui.as_weak();
    ui.on_finish(move |_action| {
        if let Some(ui) = weak.upgrade() {
            if let Err(err) = save_setup(&ui) {
                eprintln!("Could not save your setup: {err}");
            }
        }
    });

    ui.run()
}
