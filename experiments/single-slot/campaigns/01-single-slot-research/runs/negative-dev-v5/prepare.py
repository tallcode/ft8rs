#!/usr/bin/env python3
"""Build an isolated research executable; never edit native decoder modules."""
import hashlib
import json
import os
import pathlib
import shutil
import subprocess

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent.parent
ROOT = pathlib.Path('/tmp/ft8rs-single-slot-lab')
CORE = REPO / 'crates/ft8rs-core'


def save_if_changed(path, value):
    data = value.encode() if isinstance(value, str) else value
    if not path.exists() or path.read_bytes() != data:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)


def fingerprints(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for folder in ['src/decode/lib_wsjtx', 'src/decode/lib_jtdx']
            for p in sorted((root / folder).rglob('*')) if p.is_file()}


def main():
    ROOT.mkdir(exist_ok=True)
    snapshot = ROOT / 'core'
    snapshot.mkdir(exist_ok=True)
    for p in (CORE / 'src').rglob('*'):
        if p.is_file() and p.name != 'lib.rs':
            save_if_changed(snapshot / p.relative_to(CORE), p.read_bytes())
    for name in ['ALLCALL7.TXT', 'build.rs']:
        save_if_changed(snapshot / name, (CORE / name).read_bytes())
    save_if_changed(snapshot / 'src/receiver.rs', (HERE / 'receiver.rs').read_bytes())
    save_if_changed(snapshot / 'src/lib.rs', (CORE / 'src/lib.rs').read_text()+'\npub mod lab_bridge;\n')
    save_if_changed(snapshot / 'src/lab_bridge.rs', '''// Unchanged native functions exposed only by the isolated harness.
pub fn pack(s: &str) -> Vec<u8> { crate::decode::packjt77::pack77(s) }
pub fn unpack(b: &[u8], book: Option<&crate::HashCallBook>) -> Option<String> { crate::decode::packjt77::unpack77(b, book) }
pub fn tones(b: &[u8]) -> [i32;79] { crate::decode::genft8::get_ft8_tones_from_77bits(b) }
pub fn wave(t: &[i32;79], f: f64) -> (Vec<f64>, Vec<f64>) { crate::decode::gen_ft8wave::gen_ft8wave(t, f) }
pub fn fec(l: &[f64], a: &[i8], m: isize) -> Option<crate::decode::decode174_91::DecodeResult> { crate::decode::decode174_91::decode174_91(l,a,m) }
pub fn baseline(a: &[f64], o: crate::DecodeOptions) -> Vec<crate::DecodedMessage> { crate::decode::ft8_decode::decode_f64(a,o) }
pub fn subtract(a: &mut Vec<f64>, t: &[i32;79], f:f64, dt:f64, refine:bool) { crate::decode::subtractft8::subtract_ft8_refined(a,t,f,dt,refine) }
pub fn parity(i:usize) -> &'static [usize] { crate::decode::ldpc_174_91_c_parity::nm(i) }
''')
    save_if_changed(snapshot / 'src/main.rs', 'mod receiver; fn main() { receiver::main(); }\n')
    save_if_changed(snapshot / 'Cargo.toml', '''[package]
name = "ft8rs-receiver-lab"
version = "0.0.0"
edition = "2021"
license = "GPL-3.0-only"
[lib]
name = "ft8rs"
[features]
default = []
fftw = []
[dependencies]
rustfft = "6.4.1"
hound = "3.5.1"
num-bigint = "0.5.1"
[profile.release]
codegen-units = 16
lto = false
''')
    assert fingerprints(CORE) == fingerprints(snapshot)
    manifest = {
        'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip(),
        'native_sha256': fingerprints(CORE),
        'database_sha256': hashlib.sha256((CORE / 'ALLCALL7.TXT').read_bytes()).hexdigest(),
        'receiver_sha256': hashlib.sha256((HERE / 'receiver.rs').read_bytes()).hexdigest(),
        'rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(),
        'machine': subprocess.check_output(['uname', '-a'], text=True).strip(),
    }
    (ROOT / 'environment.json').write_text(json.dumps(manifest, indent=2) + '\n')
    env = dict(os.environ, RUSTC_WRAPPER='')
    subprocess.run(['cargo', 'build', '--release', '--offline', '--manifest-path',
                    str(snapshot / 'Cargo.toml')], check=True, env=env)
    print(snapshot / 'target/release/ft8rs-receiver-lab')


if __name__ == '__main__':
    main()
