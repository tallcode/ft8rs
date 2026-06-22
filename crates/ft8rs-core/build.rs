use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    link_fftw_when_enabled();
    copy_allcall7_to_binary_dir();

    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_LIBDIR");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_SYSROOT_DIR");
    println!("cargo:rerun-if-changed=ALLCALL7.TXT");
}

fn copy_allcall7_to_binary_dir() {
    let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let source = PathBuf::from(manifest_dir).join("ALLCALL7.TXT");
    if !source.exists() {
        return;
    }
    let Some(binary_dir) = cargo_binary_dir() else {
        return;
    };
    if let Err(err) = fs::create_dir_all(&binary_dir) {
        panic!(
            "failed to create binary directory {} for ALLCALL7.TXT: {err}",
            binary_dir.display()
        );
    }
    let target = binary_dir.join("ALLCALL7.TXT");
    if let Err(err) = fs::copy(&source, &target) {
        panic!(
            "failed to copy {} to {}: {err}",
            source.display(),
            target.display()
        );
    }
}

fn cargo_binary_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").ok()?);
    out_dir
        .parent()
        .and_then(|path| path.parent())
        .and_then(|path| path.parent())
        .map(PathBuf::from)
}

fn link_fftw_when_enabled() {
    if env::var_os("CARGO_FEATURE_FFTW").is_none() {
        return;
    }

    emit_pkg_config_link_search("fftw3");

    // Keep the order explicit: fftw3_threads depends on fftw3. Some installs
    // provide fftw3.pc but not fftw3_threads.pc, so link the threaded library
    // explicitly after discovering the shared FFTW library search paths.
    println!("cargo:rustc-link-lib=fftw3_threads");
    println!("cargo:rustc-link-lib=fftw3");
}

fn emit_pkg_config_link_search(package: &str) {
    let Ok(output) = Command::new("pkg-config")
        .args(["--libs", package])
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let Ok(flags) = String::from_utf8(output.stdout) else {
        return;
    };

    for flag in flags.split_whitespace() {
        if let Some(path) = flag.strip_prefix("-L") {
            if !path.is_empty() {
                println!("cargo:rustc-link-search=native={path}");
            }
        }
    }
}
