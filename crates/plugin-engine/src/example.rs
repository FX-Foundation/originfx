// SPDX-License-Identifier: GPL-3.0-or-later
//! A tiny example plugin, written in WebAssembly text and compiled when asked for.

/// The example's id.
pub const EXAMPLE_ID: &str = "originfx.example.invert-mix";

const MANIFEST: &str = r#"{"abi":1,"id":"originfx.example.invert-mix","name":"Invert Mix","version":"1.0.0","author":"FX Foundation","description":"Mixes the picture with its negative.","params":[{"name":"amount","unit":"%","default":100,"min":0,"max":100}]}"#;

/// The WebAssembly text of a plugin with `manifest` and the given body for `originfx_effect`
/// (parameters `$px $w $h $pp $np`, result i32). Shared by the example and the tests.
pub(crate) fn module_text(manifest: &str, extra_locals: &str, body: &str) -> String {
    let escaped: String = manifest
        .bytes()
        .map(|b| match b {
            b'"' => "\\22".to_string(),
            b'\\' => "\\5c".to_string(),
            0x20..=0x7e => (b as char).to_string(),
            _ => format!("\\{b:02x}"),
        })
        .collect();
    format!(
        r#"(module
  (memory (export "memory") 2)
  (global $heap (mut i32) (i32.const 4096))
  (data (i32.const 1024) "{escaped}")
  (func (export "originfx_manifest_ptr") (result i32) i32.const 1024)
  (func (export "originfx_manifest_len") (result i32) i32.const {len})
  (func (export "originfx_alloc") (param $n i32) (result i32)
    (local $p i32) (local $end i32)
    global.get $heap local.set $p
    local.get $p local.get $n i32.add i32.const 7 i32.add i32.const -8 i32.and local.set $end
    (block $done
      (loop $l
        local.get $end memory.size i32.const 16 i32.shl i32.le_u br_if $done
        i32.const 64 memory.grow i32.const -1 i32.eq
        if i32.const 0 return end
        br $l))
    local.get $end global.set $heap
    local.get $p)
  (func (export "originfx_effect") (param $px i32) (param $w i32) (param $h i32) (param $pp i32) (param $np i32) (result i32)
    {extra_locals}
    {body})
)"#,
        len = manifest.len()
    )
}

const INVERT_MIX_LOCALS: &str =
    "(local $i i32) (local $n i32) (local $a f64) (local $v i32) (local $c i32) (local $q i32)";

const INVERT_MIX_BODY: &str = r#"
    local.get $np i32.const 1 i32.lt_s
    if i32.const 1 return end
    local.get $pp f64.load local.set $a
    local.get $w local.get $h i32.mul local.set $n
    (loop $pixels
      i32.const 0 local.set $c
      (loop $ch
        local.get $px local.get $i i32.const 2 i32.shl i32.add local.get $c i32.add local.set $q
        local.get $q i32.load8_u local.set $v
        local.get $q
        local.get $v f64.convert_i32_u
        f64.const 255 local.get $v f64.convert_i32_u f64.const 2 f64.mul f64.sub
        local.get $a f64.mul f64.const 0.01 f64.mul f64.add
        f64.const 0.5 f64.add i32.trunc_f64_s
        i32.store8
        local.get $c i32.const 1 i32.add local.tee $c i32.const 3 i32.lt_u br_if $ch)
      local.get $i i32.const 1 i32.add local.tee $i local.get $n i32.lt_u br_if $pixels)
    i32.const 0"#;

pub(crate) fn invert_mix_text() -> String {
    module_text(MANIFEST, INVERT_MIX_LOCALS, INVERT_MIX_BODY)
}

/// The example plugin as a `.wasm` file's bytes: mixes the picture with its negative (one setting,
/// `amount`, 0 to 100 %).
pub fn example_invert_mix_wasm() -> Vec<u8> {
    wat::parse_str(invert_mix_text()).expect("the example plugin text is valid")
}
