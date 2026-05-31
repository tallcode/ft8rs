use std::env;
use std::process::Command;

fn main() {
    link_fftw_when_enabled();

    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_LIBDIR");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_SYSROOT_DIR");
    println!("cargo:rerun-if-env-changed=FT8RS_RELEASE_TAG");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_NAME");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_TYPE");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=.git/packed-refs");
    if let Some(head_ref) = git_head_ref() {
        println!("cargo:rerun-if-changed=.git/{head_ref}");
    }

    let package_version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    let release_tag = release_tag();
    let git_hash = git_output(&["rev-parse", "--short=7", "HEAD"]);
    let dirty = git_dirty();

    let version = if let Some(tag) = release_tag {
        strip_v_prefix(&tag).to_string()
    } else if let Some(hash) = git_hash {
        let suffix = if dirty { ".dirty" } else { "" };
        format!("{package_version}-dev+{hash}{suffix}")
    } else {
        format!("{package_version}-dev")
    };

    println!("cargo:rustc-env=FT8RS_VERSION={version}");
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

fn release_tag() -> Option<String> {
    if let Ok(tag) = env::var("FT8RS_RELEASE_TAG") {
        return valid_release_tag(&tag);
    }

    let ref_type = env::var("GITHUB_REF_TYPE").ok();
    if ref_type.as_deref() == Some("tag") {
        if let Ok(tag) = env::var("GITHUB_REF_NAME") {
            return valid_release_tag(&tag);
        }
    }

    None
}

fn valid_release_tag(tag: &str) -> Option<String> {
    let tag = tag.trim();
    if tag.starts_with('v') && tag[1..].split('.').count() == 3 {
        Some(tag.to_string())
    } else {
        None
    }
}

fn strip_v_prefix(tag: &str) -> &str {
    tag.strip_prefix('v').unwrap_or(tag)
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn git_head_ref() -> Option<String> {
    let head = std::fs::read_to_string(".git/HEAD").ok()?;
    head.strip_prefix("ref: ")
        .map(|value| value.trim().to_string())
}

fn git_dirty() -> bool {
    match Command::new("git").args(["status", "--porcelain"]).output() {
        Ok(output) if output.status.success() => !output.stdout.is_empty(),
        _ => false,
    }
}
