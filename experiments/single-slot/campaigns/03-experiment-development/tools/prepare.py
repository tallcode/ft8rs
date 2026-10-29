#!/usr/bin/env python3
"""Freeze current core and build an isolated, reproducible measurement harness.

Only the private lab snapshot exposes the unchanged protocol synthesis helpers.
No generator or public exp decoder entry is added to the product.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

HERE = Path(__file__).resolve().parent
CAMPAIGN = HERE.parent
REPO = next(p for p in HERE.parents if (p / 'crates/ft8rs-core').is_dir())


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('name', help='new immutable snapshot name')
    args = parser.parse_args()
    assert Path(args.name).name == args.name
    out = CAMPAIGN / 'frozen' / args.name
    out.mkdir(parents=True, exist_ok=False)
    core = out / 'core'
    original = REPO / 'crates/ft8rs-core'
    shutil.copytree(original / 'src', core / 'src')
    for name in ['Cargo.toml', 'build.rs', 'ALLCALL7.TXT']:
        shutil.copy2(original / name, core / name)
    source_hashes = {str(p.relative_to(REPO)): sha(p) for p in sorted((original / 'src').rglob('*')) if p.is_file()}
    for name in ['Cargo.toml', 'Cargo.lock', '.cargo/config.toml']:
        target = out / 'repository-config' / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(REPO / name, target)
    # Record UI/reconfiguration sources as well as the measured DSP boundary.
    for name in ['crates/ft8rs-engine/src/reconfig.rs', 'crates/ft8rs-gui/src/app.rs',
                 'crates/ft8rs-cli/src/main.rs']:
        target = out / 'product-source' / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(REPO / name, target)
        source_hashes[name] = sha(REPO / name)
    with (core / 'src/lib.rs').open('a') as f:
        f.write('\npub mod lab_bridge;\n')
    (core / 'src/lab_bridge.rs').write_text('''// Measurement-only protocol helpers; original routines unchanged.
pub fn pack(s: &str) -> Vec<u8> { crate::decode::packjt77::pack77(s) }
pub fn unpack(b: &[u8], book: Option<&crate::HashCallBook>) -> Option<String> { crate::decode::packjt77::unpack77(b, book) }
pub fn tones(b: &[u8]) -> [i32;79] { crate::decode::genft8::get_ft8_tones_from_77bits(b) }
pub fn wave(t: &[i32;79], f:f64) -> (Vec<f64>,Vec<f64>) { crate::decode::gen_ft8wave::gen_ft8wave(t,f) }
''')
    lab = out / 'lab'
    (lab / 'src').mkdir(parents=True)
    shutil.copy2(CAMPAIGN / 'sources/measure.rs', lab / 'src/main.rs')
    (out / 'Cargo.toml').write_text('''[workspace]
resolver = "2"
members = ["core", "lab"]
[profile.release]
codegen-units = 1
lto = "fat"
panic = "abort"
''')
    shutil.copy2(REPO / 'Cargo.lock', out / 'Cargo.lock')
    (lab / 'Cargo.toml').write_text('''[package]
name = "ft8rs-experiment-measure"
version = "0.0.0"
edition = "2021"
[dependencies]
ft8rs = { package="ft8rs-core", path="../core" }
hound = "3.5.1"
''')
    manifest = dict(revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip(),
                    source_sha256=source_hashes, generator_sha256=sha(lab/'src/main.rs'),
                    database_sha256=sha(core/'ALLCALL7.TXT'),
                    rustc=subprocess.check_output(['rustc','--version'],text=True).strip(),
                    machine=subprocess.check_output(['uname','-a'],text=True).strip(),
                    cpus=os.cpu_count(), build_complete=False)
    record=out/'freeze.json'
    record.write_text(json.dumps(manifest, indent=2)+'\n')
    with (out/'build.log').open('w') as log:
        subprocess.run(['cargo','build','--release','--offline','--manifest-path',str(lab/'Cargo.toml')],
                       env=dict(os.environ,RUSTC_WRAPPER=''), stdout=log, stderr=subprocess.STDOUT, check=True)
    binary=out/'target/release/ft8rs-experiment-measure'
    manifest.update(build_complete=True,binary_sha256=sha(binary))
    record.write_text(json.dumps(manifest,indent=2)+'\n')
    print(binary)


if __name__ == '__main__':
    main()
