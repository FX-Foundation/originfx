// SPDX-License-Identifier: GPL-3.0-or-later
//! `originfx plugin`: look at plugins, and the plugin folder the other commands use.
//!
//! Plugins are `.wasm` files in a folder. Which folder: `--plugins DIR`, else the
//! `ORIGINFX_PLUGINS` environment variable, else `./plugins` if it exists.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Args, Subcommand};
use plugin_engine::{Host, Plugin};

use crate::CliResult;

/// The plugin folder to use, if any.
pub fn folder(given: Option<PathBuf>) -> Option<PathBuf> {
    given
        .or_else(|| std::env::var_os("ORIGINFX_PLUGINS").map(PathBuf::from))
        .or_else(|| Some(PathBuf::from("plugins")).filter(|p| p.is_dir()))
}

/// The installed plugins: what is in the folder, or nothing when there is no folder.
/// Plugins that fail to load are mentioned on stderr and skipped.
pub fn host(given: Option<PathBuf>) -> Result<Arc<Host>, String> {
    let Some(dir) = folder(given) else {
        return Ok(Arc::new(Host::new()));
    };
    let host = Host::scan(&dir).map_err(|e| e.to_string())?;
    for (file, why) in &host.problems {
        eprintln!("plugin {} skipped: {why}", file.display());
    }
    Ok(Arc::new(host))
}

#[derive(Args)]
pub struct PluginArgs {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List the installed plugins and their settings.
    List {
        /// The plugin folder.
        #[arg(long)]
        plugins: Option<PathBuf>,
    },
    /// Check one plugin file and show what it says about itself.
    Info { file: PathBuf },
    /// Write the small example plugin ("Invert Mix") into a folder, to try the plugin system.
    Example {
        /// The folder to put it in (made if needed).
        dir: PathBuf,
    },
}

fn show(m: &plugin_engine::Manifest, path: &Path) {
    println!("{}  {} {}  ({})", m.id, m.name, m.version, path.display());
    if !m.description.is_empty() {
        println!("    {}", m.description);
    }
    for p in &m.params {
        println!(
            "    {} ({} to {}, default {} {})",
            p.name, p.min, p.max, p.default, p.unit
        );
    }
}

pub fn run(a: PluginArgs) -> CliResult {
    match a.cmd {
        Cmd::List { plugins } => {
            let Some(dir) = folder(plugins) else {
                println!("no plugin folder: use --plugins DIR or set ORIGINFX_PLUGINS");
                return Ok(());
            };
            let host = Host::scan(&dir).map_err(|e| e.to_string())?;
            println!("plugins in {}:", dir.display());
            if host.is_empty() {
                println!("  (none)");
            }
            for (m, path) in host.list() {
                show(m, path);
            }
            for (file, why) in &host.problems {
                println!("  skipped {}: {why}", file.display());
            }
        }
        Cmd::Info { file } => {
            let p = Plugin::load_file(&file).map_err(|e| e.to_string())?;
            show(p.manifest(), &file);
        }
        Cmd::Example { dir } => {
            std::fs::create_dir_all(&dir)?;
            let file = dir.join("invert-mix.wasm");
            std::fs::write(&file, plugin_engine::example_invert_mix_wasm())?;
            println!("wrote {}", file.display());
            println!("effect id: plugin:{}", plugin_engine::EXAMPLE_ID);
        }
    }
    Ok(())
}
