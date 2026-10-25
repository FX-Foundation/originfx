//! Proxy workflow: edit against a small copy, export from the original.
//!
//!     cargo run --release -p originfx-media-engine --example proxy_workflow -- big-clip.mp4

use media_engine::{ProxyManager, ProxySettings, VideoDecoder};

fn main() -> media_engine::Result<()> {
    let source = std::env::args()
        .nth(1)
        .expect("usage: proxy_workflow <video file>");

    let proxies = ProxyManager::new(std::env::temp_dir().join("originfx-proxies")).with_settings(
        ProxySettings {
            height: 540,
            keyint: 12,
        },
    );

    // The first call encodes the proxy; later calls return the same file instantly.
    let proxy = proxies.ensure(&source)?;
    println!("proxy: {}", proxy.display());

    // The editor opens whichever file `resolve` returns: the proxy while editing...
    let editing = proxies.resolve(&source, true);
    let first = VideoDecoder::open(&editing)?
        .next_frame()?
        .expect("no frames");
    println!("editing at {}x{}", first.width, first.height);

    // ...and the original when exporting.
    let exporting = proxies.resolve(&source, false);
    assert_eq!(exporting.as_path(), std::path::Path::new(&source));
    Ok(())
}
