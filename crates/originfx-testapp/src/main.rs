// SPDX-License-Identifier: GPL-3.0-or-later
//! OriginFX test app: a small window to play, poke and check everything the engines do.

mod app;
mod audio;
mod inspector;
mod player;
mod selfcheck;

fn main() -> eframe::Result {
    // `originfx-testapp --selfcheck [video]` prints the report without opening a window.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--selfcheck") {
        selfcheck::run(args.get(1).map(Into::into), |l| println!("{l}"));
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 720.0])
            .with_title("OriginFX test app"),
        ..Default::default()
    };
    eframe::run_native(
        "OriginFX test app",
        options,
        Box::new(move |_cc| {
            let mut a = app::App::new();
            if let Some(p) = args.first() {
                a.open_path(p.into());
            }
            Ok(Box::new(a))
        }),
    )
}
