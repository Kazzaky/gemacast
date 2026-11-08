use std::env;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    // --- macOS Swift runtime rpath ---
    // screencapturekit links a bundled Swift bridge, so the binary aborts at
    // launch without this. It lives here rather than in .cargo/config.toml
    // because `dist build` sets RUSTFLAGS, and that makes cargo drop
    // [target.*].rustflags. Link args from a build script always survive.
    if target_os == "macos" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }

    // --- Windows application resources (icon + ComCtl32 v6/DPI manifest) ---
    // rfd uses native task dialogs, which inherit the executable's application
    // icon. Compile the icon and manifest into one resource to keep every dialog
    // and the executable itself consistently branded.
    if target_os == "windows" {
        let manifest = embed_manifest::new_manifest("Gemacast.PC").to_string();
        let mut resources = tauri_winres::WindowsResource::new();
        resources
            .set_icon("../wix/assets/Product.ico")
            .set_manifest(&manifest)
            .compile()
            .expect("unable to embed Windows application resources");

        println!("cargo:rerun-if-changed=../wix/assets/Product.ico");
    }

}
