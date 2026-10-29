// SPDX-License-Identifier: GPL-3.0-or-later
//! Finding and keeping plugins.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{Error, Manifest, Plugin, Result};

/// The plugins found in a folder (every `*.wasm` file in it, not looking inside folders).
#[derive(Default)]
pub struct Host {
    plugins: BTreeMap<String, (PathBuf, Arc<Plugin>)>,
    /// Files that could not be loaded, with why.
    pub problems: Vec<(PathBuf, Error)>,
}

impl Host {
    pub fn new() -> Host {
        Host::default()
    }

    /// Load every plugin in `dir`. A file that fails is recorded in `problems` and does not stop
    /// the rest. When two files have the same id the first by file name wins.
    pub fn scan(dir: impl AsRef<Path>) -> Result<Host> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir.as_ref())
            .map_err(|e| Error::Io(format!("{}: {e}", dir.as_ref().display())))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "wasm") && p.is_file())
            .collect();
        files.sort();
        let mut host = Host::new();
        for f in files {
            match Plugin::load_file(&f) {
                Ok(p) => {
                    let id = p.manifest().id.clone();
                    match host.plugins.entry(id) {
                        std::collections::btree_map::Entry::Occupied(o) => host.problems.push((
                            f,
                            Error::Manifest(format!("id '{}' is already installed", o.key())),
                        )),
                        std::collections::btree_map::Entry::Vacant(v) => {
                            v.insert((f, Arc::new(p)));
                        }
                    }
                }
                Err(e) => host.problems.push((f, e)),
            }
        }
        Ok(host)
    }

    pub fn add(&mut self, path: impl Into<PathBuf>, plugin: Plugin) {
        self.plugins.insert(
            plugin.manifest().id.clone(),
            (path.into(), Arc::new(plugin)),
        );
    }

    pub fn get(&self, id: &str) -> Result<Arc<Plugin>> {
        self.plugins
            .get(id)
            .map(|(_, p)| Arc::clone(p))
            .ok_or_else(|| Error::NotFound(id.to_string()))
    }

    /// Installed plugins, by id.
    pub fn list(&self) -> Vec<(&Manifest, &Path)> {
        self.plugins
            .values()
            .map(|(path, p)| (p.manifest(), path.as_path()))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("plugins", &self.plugins.keys().collect::<Vec<_>>())
            .field("problems", &self.problems.len())
            .finish()
    }
}
