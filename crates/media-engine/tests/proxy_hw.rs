// SPDX-License-Identifier: GPL-3.0-or-later
mod common;

use common::*;
use media_engine::{hwaccel, probe, Error, ProxyManager, ProxySettings};

#[test]
fn proxy_is_created_once_and_reused() {
    require_ffmpeg!();
    let dir = scratch("proxy1");
    let src = pattern_clip(&dir, "big.mp4", "640x360", 25, 2);
    let _ = std::fs::remove_dir_all(dir.join("proxies")); // scratch dirs survive between runs
    let pm = ProxyManager::new(dir.join("proxies")).with_settings(ProxySettings {
        height: 180,
        keyint: 5,
    });

    assert!(pm.existing(&src).is_none());
    assert_eq!(
        pm.resolve(&src, true),
        src,
        "no proxy yet: fall back to original"
    );

    let p = pm.ensure(&src).unwrap();
    assert!(p.is_file());
    let v = probe(&p).unwrap().video[0].clone();
    assert_eq!((v.width, v.height), (320, 180));

    let before = std::fs::metadata(&p).unwrap().modified().unwrap();
    let again = pm.ensure(&src).unwrap();
    assert_eq!(again, p);
    assert_eq!(
        std::fs::metadata(&again).unwrap().modified().unwrap(),
        before,
        "must not regenerate"
    );

    assert_eq!(pm.existing(&src), Some(p.clone()));
    assert_eq!(pm.resolve(&src, true), p);
    assert_eq!(pm.resolve(&src, false), src);
}

#[test]
fn proxy_never_upscales_and_tracks_source_changes() {
    require_ffmpeg!();
    let dir = scratch("proxy2");
    let original = pattern_clip(&dir, "small.mp4", "160x90", 25, 1);
    let src = dir.join("small-copy.mp4"); // we modify this one, never the shared clip
    std::fs::copy(&original, &src).unwrap();
    let _ = std::fs::remove_dir_all(dir.join("proxies"));
    let pm = ProxyManager::new(dir.join("proxies")); // default 540p target
    let p = pm.ensure(&src).unwrap();
    let v = probe(&p).unwrap().video[0].clone();
    assert_eq!((v.width, v.height), (160, 90));

    // Replacing the source changes its identity, so the old proxy no longer matches.
    let old_path = pm.proxy_path(&src).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let bytes = std::fs::read(&src).unwrap();
    std::fs::write(&src, [bytes.as_slice(), b"\0\0\0\0"].concat()).unwrap();
    let new_path = pm.proxy_path(&src).unwrap();
    assert_ne!(old_path, new_path);
    assert!(pm.existing(&src).is_none());
}

#[test]
fn prune_removes_only_orphans() {
    require_ffmpeg!();
    let dir = scratch("proxy3");
    let a = pattern_clip(&dir, "a.mp4", "160x90", 25, 1);
    let b = pattern_clip(&dir, "b.mp4", "160x90", 25, 1);
    let _ = std::fs::remove_dir_all(dir.join("proxies"));
    let pm = ProxyManager::new(dir.join("proxies"));
    let pa = pm.ensure(&a).unwrap();
    let pb = pm.ensure(&b).unwrap();
    let removed = pm.prune(&[&a]).unwrap();
    assert_eq!(removed, 1);
    assert!(pa.exists() && !pb.exists());
}

#[test]
fn proxy_errors_are_clean() {
    let dir = scratch("proxy4");
    let pm = ProxyManager::new(dir.join("proxies"));
    assert!(matches!(pm.ensure("/no/such.mp4"), Err(Error::NotFound(_))));
    assert!(pm.existing("/no/such.mp4").is_none());
}

#[test]
fn hardware_probe_never_panics_and_is_consistent() {
    let all = hwaccel::probe_backends();
    let usable = hwaccel::usable_backends();
    assert!(usable.iter().all(|b| b.usable && b.error.is_none()));
    assert_eq!(usable.len(), all.iter().filter(|b| b.usable).count());
    for b in &all {
        assert!(!b.name.is_empty());
        assert_eq!(b.usable, b.error.is_none(), "{b:?}");
    }
    // Just exercise the call; the answer depends on the machine.
    let _ = hwaccel::hardware_codecs();
}
