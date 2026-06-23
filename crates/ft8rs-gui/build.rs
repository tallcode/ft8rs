fn main() {
    ft8rs_build::emit_version();
    #[cfg(windows)]
    embed_windows_icon();
}

/// Embed the app icon into the Windows executable so it shows in Explorer and
/// the taskbar pin. The multi-size `.ico` is generated from `assets/ft8rs.png`
/// at build time (no binary committed). Best-effort: a failure only warns, so a
/// build environment without the resource compiler still succeeds.
#[cfg(windows)]
fn embed_windows_icon() {
    use std::fs::File;
    use std::path::PathBuf;

    println!("cargo:rerun-if-changed=assets/ft8rs.png");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let png_path = PathBuf::from(&manifest).join("assets/ft8rs.png");
    let Ok(png) = std::fs::read(&png_path) else {
        println!("cargo:warning=ft8rs.png not found; skipping Windows icon embed");
        return;
    };
    let Ok(decoded) = image::load_from_memory(&png) else {
        return;
    };
    let rgba = decoded.to_rgba8();

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16u32, 24, 32, 48, 64, 128, 256] {
        let resized =
            image::imageops::resize(&rgba, size, size, image::imageops::FilterType::Lanczos3);
        let img = ico::IconImage::from_rgba_data(size, size, resized.into_raw());
        if let Ok(entry) = ico::IconDirEntry::encode(&img) {
            dir.add_entry(entry);
        }
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("ft8rs.ico");
    let Ok(file) = File::create(&out) else {
        return;
    };
    if dir.write(file).is_err() {
        return;
    }

    let mut res = winresource::WindowsResource::new();
    res.set_icon(out.to_str().unwrap());
    if let Err(err) = res.compile() {
        println!("cargo:warning=failed to embed Windows icon: {err}");
    }
}
