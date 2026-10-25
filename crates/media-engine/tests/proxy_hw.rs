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

use media_engine::{FrameReader, HwMode, VideoDecoder};

#[test]
fn hw_mode_parses_from_text() {
    assert_eq!(HwMode::parse("off"), HwMode::Off);
    assert_eq!(HwMode::parse(""), HwMode::Off);
    assert_eq!(HwMode::parse("AUTO"), HwMode::Auto);
    assert_eq!(HwMode::parse("CUDA"), HwMode::Backend("cuda".into()));
}

#[test]
fn gpu_auto_mode_matches_software_whatever_the_machine_has() {
    require_ffmpeg!();
    let dir = scratch("hw-auto");
    let src = pattern_clip(&dir, "clip.mp4", "320x180", 25, 2);

    let mut sw = VideoDecoder::open(&src).unwrap();
    let mut gpu = VideoDecoder::open_with(&src, HwMode::Auto).unwrap();
    let backend = gpu.hw_backend().map(str::to_owned);
    eprintln!("GPU backend in use: {backend:?}");

    let mut frames = 0;
    while let Some(a) = sw.next_frame().unwrap() {
        let b = gpu.next_frame().unwrap().expect("same number of frames");
        assert_eq!((a.width, a.height), (b.width, b.height));
        assert!((a.pts_secs - b.pts_secs).abs() < 1e-6);
        let diff: u64 = a
            .data
            .iter()
            .zip(&b.data)
            .map(|(x, y)| u64::from(x.abs_diff(*y)))
            .sum();
        let mean = diff as f64 / a.data.len() as f64;
        // Without a GPU the two paths are the same code, so identical; with one,
        // rounding in the hardware decoder allows a small difference.
        let allowed = if backend.is_none() { 0.0 } else { 4.0 };
        assert!(mean <= allowed, "frame {frames}: mean difference {mean}");
        frames += 1;
    }
    assert!(gpu.next_frame().unwrap().is_none());
    assert_eq!(frames, 50);
}

#[test]
fn gpu_mode_works_with_seeking_and_the_frame_reader() {
    require_ffmpeg!();
    let dir = scratch("hw-reader");
    let src = id_clip(&dir);
    let mut reader = FrameReader::open_with(&src, 8 << 20, HwMode::Auto).unwrap();
    for t in [1.5, 0.2, 1.0, 0.0] {
        let f = reader.frame_at(t).unwrap();
        let expected = (t * 30.0 + 1e-6).floor() as i64;
        let got = frame_number(&f.data);
        assert!(
            (got - expected).abs() <= 1,
            "at {t}s: frame {got}, wanted {expected}"
        );
    }
}

#[test]
fn demanding_an_unavailable_backend_is_a_clean_error() {
    require_ffmpeg!();
    let dir = scratch("hw-demand");
    let src = id_clip(&dir);
    // Nonsense name.
    let r = VideoDecoder::open_with(&src, HwMode::Backend("not-a-gpu".into()));
    assert!(matches!(r, Err(Error::InvalidArgument(_))), "{:?}", r.err());
    // A real backend that this machine cannot open.
    if let Some(b) = hwaccel::probe_backends().into_iter().find(|b| !b.usable) {
        let r = VideoDecoder::open_with(&src, HwMode::Backend(b.name.clone()));
        assert!(
            matches!(
                r,
                Err(Error::Unsupported(_)) | Err(Error::InvalidArgument(_))
            ),
            "{}: {:?}",
            b.name,
            r.err()
        );
    }
}
