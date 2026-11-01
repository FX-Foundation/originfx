// Developer tool: renders each welcome screen headlessly to PPM files.
// Build with: cargo run --release --features snapshot --bin snapshot -- <out_dir>
slint::include_modules!();

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{PhysicalSize, PlatformError, Rgb8Pixel, SharedPixelBuffer};
use std::rc::Rc;

struct HeadlessPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let (w, h): (u32, u32) = (
        std::env::var("SNAP_W").ok().and_then(|v| v.parse().ok()).unwrap_or(1440),
        std::env::var("SNAP_H").ok().and_then(|v| v.parse().ok()).unwrap_or(900),
    );
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(PhysicalSize::new(w, h));
    slint::platform::set_platform(Box::new(HeadlessPlatform(window.clone()))).unwrap();

    let ui = App::new().unwrap();
    ui.set_gpu_title("Ready for smooth editing".into());
    ui.set_gpu_line("[GPU name] found. Hardware acceleration is on.".into());
    ui.show().unwrap();

    for (scheme, name) in [(slint::language::ColorScheme::Dark, "dark"), (slint::language::ColorScheme::Light, "light")] {
        ui.global::<Palette>().set_color_scheme(scheme);
        ui.global::<Settings>().set_look(0);
        for step in 1..=3 {
            ui.set_step(step);
            slint::platform::update_timers_and_animations();
            let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
            window.request_redraw();
            window.draw_if_needed(|renderer| {
                renderer.render(buf.make_mut_slice(), w as usize);
            });
            let mut data = format!("P6\n{} {}\n255\n", w, h).into_bytes();
            for p in buf.as_slice() {
                data.extend_from_slice(&[p.r, p.g, p.b]);
            }
            std::fs::write(format!("{out}/{name}-step{step}.ppm"), data).unwrap();
        }
    }
}
