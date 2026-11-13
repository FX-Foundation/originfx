fn main() {
    slint_build::compile("ui/app.slint").expect("failed to compile the OriginFX UI");

    // Windows: embed the app icon. Never fatal; a missing icon must not break the build.
    #[cfg(windows)]
    {
        if let Err(err) = winresource::WindowsResource::new()
            .set_icon("assets/originfx.ico")
            .compile()
        {
            println!("cargo:warning=could not embed the Windows icon: {err}");
        }
    }
}
