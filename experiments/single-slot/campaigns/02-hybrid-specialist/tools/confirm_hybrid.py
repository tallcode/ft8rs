#!/usr/bin/env python3
"""Fresh frozen Hybrid-only validation. Timed jobs and replay processes are serial."""
import argparse
import hashlib
import json
import pathlib
import subprocess
import sys

HERE=pathlib.Path(__file__).resolve().parent
REPO=next(p for p in HERE.parents if (p / "crates/ft8rs-core").is_dir())
BASE=REPO/"experiments/single-slot"
ROOT=pathlib.Path('/tmp/ft8rs-single-slot-lab')
JOBS=[
 ('awgn','16','2026108991','sic-fresh-awgn','-21,-22,-23'),
 ('cw','16','2026108991','sic-fresh-cw','-20,-22'),
 ('mix','8','2026108991','sic-fresh-mix','-20,-22'),
 ('mix2','8','2026108991','sic-fresh-mix2','-18,-20'),
 ('mixfade','8','2026108991','sic-fresh-mixfade','-18,-20'),
 ('drift','8','2026108991','sic-fresh-drift','-18,-20'),
 ('noise','16','2026109041','sic-fresh-noise','-24'),
 ('cwonly','16','2026109051','sic-fresh-cwonly','-24'),
 ('blocker','16','2026109061','sic-fresh-blocker','-24'),
]


def frozen_source(relative):
    campaign=BASE/'campaigns/02-hybrid-specialist'
    if relative.startswith('crates/ft8rs-core/src/'):
        data=(campaign/'frozen/src'/relative.removeprefix('crates/ft8rs-core/src/')).read_bytes()
        if relative.endswith('/src/lib.rs'):data=data.removesuffix(b'\npub mod lab_bridge;\n')
        return data
    if relative=='experiments/single-slot/receiver_hybrid.rs':return (campaign/'sources/receiver_hybrid.rs').read_bytes()
    return (campaign/'frozen/product-source'/relative).read_bytes()


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('prefix')
    args=parser.parse_args()
    freeze=json.loads((BASE/'campaigns/02-hybrid-specialist/freeze-hybrid-specialist-v1.json').read_text())
    for relative,digest in freeze['source_sha256'].items():
        assert hashlib.sha256(frozen_source(relative)).hexdigest()==digest,relative
    binary=ROOT/'core/target/release/ft8rs-receiver-lab'
    assert hashlib.sha256(binary.read_bytes()).hexdigest()==freeze['binary_sha256']
    names=[args.prefix+'-'+job[0] for job in JOBS]+[args.prefix+'-channels',args.prefix+'-historical']
    assert all(not (ROOT/'runs'/name).exists() for name in names),'Evidence is immutable; choose a fresh prefix.'
    for (suffix,*job),name in zip(JOBS,names):
        subprocess.run([sys.executable,str(HERE/'run.py'),name,*job,'jtdx,hybrid,hybrid-specialist'],check=True)
    for preset in ['channels','historical']:
        subprocess.run([sys.executable,str(HERE/'run_files.py'),args.prefix+'-'+preset,preset,'native,jtdx,hybrid,hybrid-specialist'],check=True)


if __name__=='__main__':main()
