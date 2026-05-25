fn main() {
    if let Ok(fftw_dir) = std::env::var("FFTW_DIR") {
        println!("cargo:rustc-link-search=native={fftw_dir}/lib");
    }
    for lib_dir in ["/opt/homebrew/lib", "/usr/local/lib"] {
        if std::path::Path::new(lib_dir).join("libfftw3.dylib").exists()
            || std::path::Path::new(lib_dir).join("libfftw3.a").exists()
        {
            println!("cargo:rustc-link-search=native={lib_dir}");
        }
    }

    // Link against system libfftw3.
    println!("cargo:rustc-link-lib=fftw3");

    println!("cargo:rerun-if-env-changed=FFTW_DIR");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/util/fft_fftw.rs");
}
