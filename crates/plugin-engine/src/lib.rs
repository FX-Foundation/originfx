// SPDX-License-Identifier: GPL-3.0-or-later
//! # OriginFX Plugin Engine
//!
//! Plugins are WebAssembly modules that add picture effects. They run inside a sandbox: a plugin
//! has **no access** to files, the network, the clock or anything else of the machine. It gets a
//! picture and some numbers and hands back a picture. Each call runs in a fresh instance with a
//! cap on the work it may do (fuel) and the memory it may use, so a broken or hostile plugin can
//! fail but cannot hang the editor, eat the machine's memory or leave anything behind.
//!
//! See `docs/plugin-engine/README.md` for the plugin interface (ABI version 1) and how to write one.
//!
//! * [`Plugin::load`] checks a module and reads its [`Manifest`].
//! * [`Plugin::apply`] runs the effect on RGBA8 pixels in place.
//! * [`Host`] finds plugin files in a folder.

mod host;
mod manifest;
mod plugin;

#[cfg(feature = "example")]
mod example;
#[cfg(test)]
mod tests;

pub use host::Host;
pub use manifest::{Manifest, ParamDef, ABI_VERSION};
pub use plugin::{Limits, Plugin};

#[cfg(feature = "example")]
pub use example::{example_invert_mix_wasm, EXAMPLE_ID};

/// What can go wrong with a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The file could not be read.
    Io(String),
    /// Not a valid WebAssembly module.
    InvalidModule(String),
    /// The module asks for something the sandbox does not give (an import), or lacks a required export.
    Interface(String),
    /// The manifest is missing, not valid, or breaks a rule.
    Manifest(String),
    /// The plugin used more than its allowance of work.
    OutOfFuel,
    /// The plugin used more than its allowance of memory.
    OutOfMemory,
    /// The plugin crashed (a trap).
    Trapped(String),
    /// The plugin said it could not process this picture (its return code).
    Failed(i32),
    /// No installed plugin has this id.
    NotFound(String),
    /// The picture or the settings do not fit what the plugin was given.
    BadInput(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(m) => write!(f, "cannot read plugin: {m}"),
            Error::InvalidModule(m) => write!(f, "not a valid plugin module: {m}"),
            Error::Interface(m) => write!(f, "plugin interface: {m}"),
            Error::Manifest(m) => write!(f, "plugin manifest: {m}"),
            Error::OutOfFuel => write!(f, "plugin used too much processing time and was stopped"),
            Error::OutOfMemory => write!(f, "plugin used too much memory and was stopped"),
            Error::Trapped(m) => write!(f, "plugin crashed: {m}"),
            Error::Failed(c) => write!(f, "plugin could not process the picture (code {c})"),
            Error::NotFound(id) => write!(f, "plugin '{id}' is not installed"),
            Error::BadInput(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
