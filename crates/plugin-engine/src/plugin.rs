// SPDX-License-Identifier: GPL-3.0-or-later
//! Loading and running one plugin in the sandbox.

use std::path::Path;

use wasmi::{
    Config, Engine, Instance, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder,
};

use crate::{Error, Manifest, Result};

/// What one call to a plugin may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Work allowance for a call: this much plus `fuel_per_pixel` for every pixel.
    pub base_fuel: u64,
    pub fuel_per_pixel: u64,
    /// The most memory the plugin may have, in bytes (its own code and data included).
    pub memory_bytes: usize,
    /// The largest picture it is given, in pixels.
    pub max_pixels: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            base_fuel: 50_000_000,
            fuel_per_pixel: 600,
            memory_bytes: 512 * 1024 * 1024,
            max_pixels: 8192 * 4608,
        }
    }
}

/// A checked plugin, ready to run.
pub struct Plugin {
    engine: Engine,
    module: Module,
    manifest: Manifest,
    limits: Limits,
}

struct State {
    limits: StoreLimits,
}

fn export_fn<P: wasmi::WasmParams, R: wasmi::WasmResults>(
    store: &mut Store<State>,
    inst: &Instance,
    name: &str,
) -> Result<wasmi::TypedFunc<P, R>> {
    inst.get_typed_func::<P, R>(&mut *store, name)
        .map_err(|_| Error::Interface(format!("missing or wrongly typed export `{name}`")))
}

fn memory(store: &mut Store<State>, inst: &Instance) -> Result<Memory> {
    inst.get_memory(&*store, "memory")
        .ok_or_else(|| Error::Interface("missing export `memory`".into()))
}

fn trap(e: wasmi::Error) -> Error {
    use wasmi::errors::ErrorKind;
    match e.kind() {
        ErrorKind::TrapCode(wasmi::TrapCode::OutOfFuel) => Error::OutOfFuel,
        ErrorKind::Memory(_) => Error::OutOfMemory,
        _ => {
            let m = e.to_string();
            if m.contains("fuel") {
                Error::OutOfFuel
            } else if m.contains("memory") && m.contains("limit") {
                Error::OutOfMemory
            } else {
                Error::Trapped(m)
            }
        }
    }
}

impl Plugin {
    /// Read and check a plugin file.
    pub fn load_file(path: impl AsRef<Path>) -> Result<Plugin> {
        let bytes = std::fs::read(path.as_ref()).map_err(|e| Error::Io(e.to_string()))?;
        Plugin::load(&bytes)
    }

    pub fn load(bytes: &[u8]) -> Result<Plugin> {
        Plugin::load_with(bytes, Limits::default())
    }

    pub fn load_with(bytes: &[u8], limits: Limits) -> Result<Plugin> {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module =
            Module::new(&engine, bytes).map_err(|e| Error::InvalidModule(e.to_string()))?;
        // The sandbox gives nothing: a plugin may not import anything.
        if let Some(i) = module.imports().next() {
            return Err(Error::Interface(format!(
                "plugins may not import anything, but this one imports `{}.{}`",
                i.module(),
                i.name()
            )));
        }
        let mut plugin = Plugin {
            engine,
            module,
            manifest: Manifest {
                abi: 0,
                id: String::new(),
                name: String::new(),
                version: String::new(),
                author: String::new(),
                description: String::new(),
                params: vec![],
            },
            limits,
        };
        plugin.manifest = plugin.read_manifest()?;
        Ok(plugin)
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn instance(&self, fuel: u64) -> Result<(Store<State>, Instance)> {
        let state = State {
            limits: StoreLimitsBuilder::new()
                .memory_size(self.limits.memory_bytes)
                .memories(1)
                .tables(4)
                .instances(1)
                .trap_on_grow_failure(false)
                .build(),
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limits);
        store
            .set_fuel(fuel)
            .map_err(|e| Error::Interface(e.to_string()))?;
        let linker = <Linker<State>>::new(&self.engine);
        let inst = linker
            .instantiate_and_start(&mut store, &self.module)
            .map_err(trap)?;
        Ok((store, inst))
    }

    fn read_manifest(&self) -> Result<Manifest> {
        let (mut store, inst) = self.instance(self.limits.base_fuel)?;
        let ptr = export_fn::<(), i32>(&mut store, &inst, "originfx_manifest_ptr")?;
        let len = export_fn::<(), i32>(&mut store, &inst, "originfx_manifest_len")?;
        let (p, l) = (
            ptr.call(&mut store, ()).map_err(trap)?,
            len.call(&mut store, ()).map_err(trap)?,
        );
        if p < 0 || !(1..=64 * 1024).contains(&l) {
            return Err(Error::Manifest("manifest is empty or over 64 KiB".into()));
        }
        let mem = memory(&mut store, &inst)?;
        let mut buf = vec![0u8; l as usize];
        mem.read(&store, p as usize, &mut buf)
            .map_err(|_| Error::Manifest("manifest lies outside the plugin's memory".into()))?;
        let m = Manifest::parse(&buf)?;
        // The effect function must exist and be typed right.
        export_fn::<(i32, i32, i32, i32, i32), i32>(&mut store, &inst, "originfx_effect")?;
        export_fn::<i32, i32>(&mut store, &inst, "originfx_alloc")?;
        Ok(m)
    }

    /// Run the effect on `rgba` (straight-alpha RGBA8, `w` by `h`) in place. `values` are the
    /// settings in manifest order; each is clamped to its range. If the plugin fails, `rgba` is
    /// left exactly as it was.
    pub fn apply(&self, values: &[f64], rgba: &mut [u8], w: usize, h: usize) -> Result<()> {
        let pixels = w
            .checked_mul(h)
            .ok_or_else(|| Error::BadInput("picture too large".into()))?;
        if rgba.len() != pixels * 4 {
            return Err(Error::BadInput(
                "picture size does not match its data".into(),
            ));
        }
        if pixels == 0 {
            return Ok(());
        }
        if pixels > self.limits.max_pixels {
            return Err(Error::BadInput("picture too large for plugins".into()));
        }
        if values.len() != self.manifest.params.len() {
            return Err(Error::BadInput(format!(
                "{} takes {} settings, got {}",
                self.manifest.name,
                self.manifest.params.len(),
                values.len()
            )));
        }
        let fuel = self
            .limits
            .base_fuel
            .saturating_add(self.limits.fuel_per_pixel.saturating_mul(pixels as u64));
        let (mut store, inst) = self.instance(fuel)?;
        let alloc = export_fn::<i32, i32>(&mut store, &inst, "originfx_alloc")?;
        let effect =
            export_fn::<(i32, i32, i32, i32, i32), i32>(&mut store, &inst, "originfx_effect")?;
        let mem = memory(&mut store, &inst)?;
        let n = rgba.len();
        let pix_ptr = alloc.call(&mut store, n as i32).map_err(trap)?;
        let par_ptr = alloc
            .call(&mut store, (values.len().max(1) * 8) as i32)
            .map_err(trap)?;
        if pix_ptr <= 0 || par_ptr <= 0 {
            return Err(Error::OutOfMemory);
        }
        mem.write(&mut store, pix_ptr as usize, rgba)
            .map_err(|_| Error::OutOfMemory)?;
        let mut pbytes = Vec::with_capacity(values.len() * 8);
        for (v, d) in values.iter().zip(&self.manifest.params) {
            let v = if v.is_finite() {
                v.clamp(d.min, d.max)
            } else {
                d.default
            };
            pbytes.extend_from_slice(&v.to_le_bytes());
        }
        mem.write(&mut store, par_ptr as usize, &pbytes)
            .map_err(|_| Error::OutOfMemory)?;
        let code = effect
            .call(
                &mut store,
                (pix_ptr, w as i32, h as i32, par_ptr, values.len() as i32),
            )
            .map_err(trap)?;
        if code != 0 {
            return Err(Error::Failed(code));
        }
        let mut out = vec![0u8; n];
        mem.read(&store, pix_ptr as usize, &mut out)
            .map_err(|_| Error::Trapped("picture lies outside the plugin's memory".into()))?;
        rgba.copy_from_slice(&out);
        Ok(())
    }
}
