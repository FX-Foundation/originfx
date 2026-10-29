// SPDX-License-Identifier: GPL-3.0-or-later
use crate::example::{invert_mix_text, module_text};
use crate::*;

fn wasm(text: &str) -> Vec<u8> {
    wat::parse_str(text).unwrap()
}

fn invert_mix() -> Plugin {
    Plugin::load(&wasm(&invert_mix_text())).unwrap()
}

const OK_MANIFEST: &str = r#"{"abi":1,"id":"t.p","name":"P","params":[]}"#;

fn custom(manifest: &str, body: &str) -> Vec<u8> {
    wasm(&module_text(manifest, "(local $i i32)", body))
}

fn picture(w: usize, h: usize, v: u8) -> Vec<u8> {
    [v, 255 - v, 7, 200]
        .iter()
        .copied()
        .cycle()
        .take(w * h * 4)
        .collect()
}

#[test]
fn the_manifest_is_read_and_checked() {
    let p = invert_mix();
    let m = p.manifest();
    assert_eq!(m.id, "originfx.example.invert-mix");
    assert_eq!(m.params.len(), 1);
    assert_eq!(m.params[0].name, "amount");
    assert_eq!(m.defaults(), vec![100.0]);
    for bad in [
        r#"{"abi":2,"id":"a","name":"A"}"#,
        r#"{"abi":1,"id":"has space","name":"A"}"#,
        r#"{"abi":1,"id":"","name":"A"}"#,
        r#"{"abi":1,"id":"a","name":" "}"#,
        r#"{"abi":1,"id":"a","name":"A","params":[{"name":"x","default":5,"min":0,"max":1}]}"#,
        r#"{"abi":1,"id":"a","name":"A","params":[{"name":"x","default":0,"min":0,"max":1},{"name":"x","default":0,"min":0,"max":1}]}"#,
        r#"{"abi":1,"id":"a","name":"A","params":[{"name":"bad name","default":0,"min":0,"max":1}]}"#,
        "not json",
    ] {
        let e = Plugin::load(&custom(bad, "i32.const 0")).err().unwrap();
        assert!(matches!(e, Error::Manifest(_)), "{bad}: {e}");
    }
}

#[test]
fn the_effect_changes_pixels_and_amount_zero_changes_nothing() {
    let p = invert_mix();
    let src = picture(8, 4, 40);
    let mut full = src.clone();
    p.apply(&[100.0], &mut full, 8, 4).unwrap();
    // 40 inverts to 215, 215 to 40; the blue 7 to 248; alpha is left alone.
    assert_eq!(&full[0..4], &[215, 40, 248, 200]);
    let mut none = src.clone();
    p.apply(&[0.0], &mut none, 8, 4).unwrap();
    assert_eq!(none, src);
    let mut half = src.clone();
    p.apply(&[50.0], &mut half, 8, 4).unwrap();
    assert_eq!(half[0], 128);
    // Out-of-range and not-a-number settings are clamped or defaulted, never passed through.
    let mut over = src.clone();
    p.apply(&[900.0], &mut over, 8, 4).unwrap();
    assert_eq!(over, full);
    let mut nan = src.clone();
    p.apply(&[f64::NAN], &mut nan, 8, 4).unwrap();
    assert_eq!(nan, full);
    // Repeatable: no state carries from one call to the next.
    let mut again = src.clone();
    p.apply(&[100.0], &mut again, 8, 4).unwrap();
    assert_eq!(again, full);
}

#[test]
fn wrong_input_is_refused_cleanly() {
    let p = invert_mix();
    let mut px = picture(4, 4, 1);
    assert!(matches!(
        p.apply(&[], &mut px, 4, 4),
        Err(Error::BadInput(_))
    ));
    assert!(matches!(
        p.apply(&[1.0, 2.0], &mut px, 4, 4),
        Err(Error::BadInput(_))
    ));
    assert!(matches!(
        p.apply(&[1.0], &mut px, 5, 4),
        Err(Error::BadInput(_))
    ));
    p.apply(&[1.0], &mut [], 0, 0).unwrap();
}

#[test]
fn bad_modules_are_refused() {
    assert!(matches!(
        Plugin::load(b"nope").err().unwrap(),
        Error::InvalidModule(_)
    ));
    // Imports are not allowed: nothing from the machine is reachable.
    let imports = wasm(
        r#"(module (import "env" "read_file" (func (param i32)))
        (memory (export "memory") 1))"#,
    );
    let e = Plugin::load(&imports).err().unwrap();
    assert!(
        matches!(&e, Error::Interface(m) if m.contains("env.read_file")),
        "{e}"
    );
    // Missing exports.
    let none = wasm("(module (memory (export \"memory\") 1))");
    assert!(matches!(
        Plugin::load(&none).err().unwrap(),
        Error::Interface(_)
    ));
    // Right names, wrong types.
    let wrong = wasm(
        r#"(module (memory (export "memory") 1)
        (func (export "originfx_manifest_ptr") (result i64) i64.const 0)
        (func (export "originfx_manifest_len") (result i32) i32.const 5))"#,
    );
    assert!(matches!(
        Plugin::load(&wrong).err().unwrap(),
        Error::Interface(_)
    ));
    // A manifest pointing outside memory.
    let lost = wasm(
        r#"(module (memory (export "memory") 1)
        (func (export "originfx_manifest_ptr") (result i32) i32.const 70000)
        (func (export "originfx_manifest_len") (result i32) i32.const 50))"#,
    );
    assert!(matches!(
        Plugin::load(&lost).err().unwrap(),
        Error::Manifest(_)
    ));
}

#[test]
fn a_plugin_that_never_stops_is_stopped() {
    let p = Plugin::load(&custom(OK_MANIFEST, "(loop $l br $l) i32.const 0")).unwrap();
    let mut px = picture(16, 16, 9);
    let before = px.clone();
    let t = std::time::Instant::now();
    assert_eq!(p.apply(&[], &mut px, 16, 16), Err(Error::OutOfFuel));
    assert!(t.elapsed().as_secs() < 30);
    assert_eq!(px, before, "a failed plugin leaves the picture alone");
}

#[test]
fn crashes_and_failure_codes_are_reported_and_leave_the_picture_alone() {
    let boom = Plugin::load(&custom(OK_MANIFEST, "unreachable")).unwrap();
    let mut px = picture(4, 4, 3);
    let before = px.clone();
    assert!(matches!(
        boom.apply(&[], &mut px, 4, 4),
        Err(Error::Trapped(_))
    ));
    assert_eq!(px, before);
    // Reading past the end of its memory is a trap, not a leak.
    let oob = Plugin::load(&custom(
        OK_MANIFEST,
        "i32.const 1000000000 i32.load drop i32.const 0",
    ))
    .unwrap();
    assert!(matches!(
        oob.apply(&[], &mut px, 4, 4),
        Err(Error::Trapped(_))
    ));
    let fail = Plugin::load(&custom(OK_MANIFEST, "i32.const 7")).unwrap();
    assert_eq!(fail.apply(&[], &mut px, 4, 4), Err(Error::Failed(7)));
    assert_eq!(px, before);
    // A plugin that scribbles on the picture and then fails does not change it either.
    let half = Plugin::load(&custom(
        OK_MANIFEST,
        "local.get $px i32.const 255 i32.store8 i32.const 9",
    ))
    .unwrap();
    assert_eq!(half.apply(&[], &mut px, 4, 4), Err(Error::Failed(9)));
    assert_eq!(px, before);
}

#[test]
fn memory_is_capped() {
    // Asks for about 2 GB at the start.
    let hog = wasm(&module_text(OK_MANIFEST, "", "i32.const 0").replace(
        "(memory (export \"memory\") 2)",
        "(memory (export \"memory\") 30000)",
    ));
    let e = Plugin::load(&hog).err().unwrap();
    assert_eq!(e, Error::OutOfMemory);
    // Growing past the cap fails the allocation instead of taking the machine's memory.
    let small = Limits {
        memory_bytes: 8 * 1024 * 1024,
        ..Limits::default()
    };
    let p = Plugin::load_with(&wasm(&invert_mix_text()), small).unwrap();
    let mut px = picture(2000, 2000, 1); // 16 MB
    assert!(p.apply(&[100.0], &mut px, 2000, 2000).is_err());
    // A picture over the pixel cap is refused up front.
    let tiny = Limits {
        max_pixels: 100,
        ..Limits::default()
    };
    let p = Plugin::load_with(&wasm(&invert_mix_text()), tiny).unwrap();
    let mut px = picture(20, 20, 1);
    assert!(matches!(
        p.apply(&[100.0], &mut px, 20, 20),
        Err(Error::BadInput(_))
    ));
}

#[test]
fn a_full_size_frame_is_fast_enough_to_use() {
    let p = invert_mix();
    let (w, h) = (1280, 720);
    let mut px = picture(w, h, 33);
    let t = std::time::Instant::now();
    p.apply(&[100.0], &mut px, w, h).unwrap();
    eprintln!("720p through the interpreter: {:?}", t.elapsed());
    assert_eq!(px[0], 222);
}

#[test]
fn the_host_finds_plugins_and_reports_the_ones_it_cannot_load() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("b_invert.wasm"), wasm(&invert_mix_text())).unwrap();
    std::fs::write(dir.path().join("a_copy.wasm"), wasm(&invert_mix_text())).unwrap();
    std::fs::write(dir.path().join("broken.wasm"), b"garbage").unwrap();
    std::fs::write(dir.path().join("notes.txt"), b"ignored").unwrap();
    let host = Host::scan(dir.path()).unwrap();
    assert_eq!(host.list().len(), 1);
    // The first file by name wins; the duplicate and the broken file are reported.
    assert!(host.list()[0].1.ends_with("a_copy.wasm"));
    assert_eq!(host.problems.len(), 2);
    assert!(host.get("originfx.example.invert-mix").is_ok());
    assert_eq!(
        host.get("nope").err().unwrap(),
        Error::NotFound("nope".into())
    );
    assert!(Host::scan(dir.path().join("missing")).is_err());
    assert!(Host::new().is_empty());
}
