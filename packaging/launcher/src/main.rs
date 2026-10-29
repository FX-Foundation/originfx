// SPDX-License-Identifier: GPL-3.0-or-later
//! A single file that carries the OriginFX test app and every library it needs. It unpacks them
//! into a fresh temporary folder, runs the app, waits for it to close, and deletes the folder.
//! Nothing is installed and nothing is left behind.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A zip of the app and its libraries, made by `packaging/make_zip.py` before this is built.
static PAYLOAD: &[u8] = include_bytes!(env!("ORIGINFX_PAYLOAD"));

const APP: &str = if cfg!(windows) {
    "originfx-testapp.exe"
} else {
    "originfx-testapp"
};

/// Deletes the unpacked folder when dropped, even if something goes wrong.
struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn unpack(into: &Path) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(PAYLOAD)).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = f.enclosed_name() else {
            continue;
        };
        let out = into.join(rel);
        if f.is_dir() {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut file = fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        std::io::copy(&mut f, &mut file).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = f.unix_mode() {
                let _ = fs::set_permissions(&out, fs::Permissions::from_mode(mode));
            }
        }
    }
    Ok(())
}

/// Remove folders left by an earlier run that was killed, when its launcher is gone.
fn sweep() {
    #[cfg(unix)]
    if let Ok(entries) = fs::read_dir(std::env::temp_dir()) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(rest) = name.strip_prefix("originfx-run-") else {
                continue;
            };
            let pid = rest.split('-').next().unwrap_or("");
            if !pid.is_empty() && !Path::new(&format!("/proc/{pid}")).exists() {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
}

fn run() -> Result<i32, String> {
    sweep();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("originfx-run-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let _cleanup = Cleanup(dir.clone());
    unpack(&dir)?;

    let mut cmd = Command::new(dir.join(APP));
    cmd.args(std::env::args_os().skip(1));
    #[cfg(unix)]
    {
        let old = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        cmd.env("LD_LIBRARY_PATH", format!("{}:{old}", dir.display()));
    }
    let status = cmd
        .status()
        .map_err(|e| format!("cannot start the app: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(windows)]
fn say(text: &str) {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(h: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    // SAFETY: both strings are NUL-terminated and live for the call.
    unsafe {
        MessageBoxW(
            0,
            wide(text).as_ptr(),
            wide("OriginFX test app").as_ptr(),
            0x10,
        );
    }
}

#[cfg(not(windows))]
fn say(text: &str) {
    eprintln!("OriginFX test app: {text}");
}

fn main() {
    match run() {
        Ok(0) => {}
        Ok(code) => {
            #[cfg(windows)]
            say(&format!(
                "The app closed with error code {code:#x}. If it did not even open, a Windows library may be missing; tell Claude this number."
            ));
            std::process::exit(code);
        }
        Err(e) => {
            say(&e);
            std::process::exit(1);
        }
    }
}
