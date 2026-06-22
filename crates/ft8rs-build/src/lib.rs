//! Shared build-time version logic for the ft8rs binary crates.
//!
//! Each binary crate (`ft8rs-cli`, later `ft8rs-gui`) calls [`emit_version`]
//! from its own `build.rs` so that `env!("FT8RS_VERSION")` resolves in that
//! crate. `cargo:rustc-env` only applies to the package whose build script
//! emits it, so the logic is shared as a library rather than living in one
//! crate's build script.
//!
//! The git directory is resolved explicitly (`git rev-parse
//! --absolute-git-dir`) because in a workspace the `.git` lives at the
//! workspace root, not in the calling member's directory.

use std::env;
use std::path::PathBuf;
use std::process::Command;

/// Emit `cargo:rustc-env=FT8RS_VERSION=...` for the calling binary crate.
///
/// Tagged releases print the tag (e.g. `0.0.2`); development builds append git
/// metadata (e.g. `0.0.0-dev+3ed8eaa` or `...dirty`). This mirrors the original
/// single-crate build script behavior exactly.
pub fn emit_version() {
    println!("cargo:rerun-if-env-changed=FT8RS_RELEASE_TAG");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_NAME");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_TYPE");

    if let Some(git_dir) = git_dir() {
        println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
        println!("cargo:rerun-if-changed={}", git_dir.join("index").display());
        println!(
            "cargo:rerun-if-changed={}",
            git_dir.join("packed-refs").display()
        );
        if let Some(head_ref) = git_head_ref(&git_dir) {
            println!(
                "cargo:rerun-if-changed={}",
                git_dir.join(head_ref).display()
            );
        }
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

fn git_dir() -> Option<PathBuf> {
    git_output(&["rev-parse", "--absolute-git-dir"]).map(PathBuf::from)
}

fn git_head_ref(git_dir: &std::path::Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    head.strip_prefix("ref: ")
        .map(|value| value.trim().to_string())
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

fn git_dirty() -> bool {
    match Command::new("git").args(["status", "--porcelain"]).output() {
        Ok(output) if output.status.success() => !output.stdout.is_empty(),
        _ => false,
    }
}
