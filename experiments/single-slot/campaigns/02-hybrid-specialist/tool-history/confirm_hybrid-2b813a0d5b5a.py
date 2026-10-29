#!/usr/bin/env python3
"""Fresh frozen Hybrid-only validation. Timed jobs and replay processes are serial."""
import argparse
import hashlib
import json
import pathlib
import subprocess
import sys

HERE=pathlib.Path(__file__).resolve().parent
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


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('prefix')
    args=parser.parse_args()
    freeze=json.loads((HERE/'campaigns/02-hybrid-specialist/freeze-hybrid-specialist-v1.json').read_text())
    for relative,digest in freeze['source_sha256'].items():
        assert hashlib.sha256((HERE.parent.parent/relative).read_bytes()).hexdigest()==digest,relative
    binary=ROOT/'core/target/release/ft8rs-receiver-lab'
    assert hashlib.sha256(binary.read_bytes()).hexdigest()==freeze['binary_sha256']
    names=[args.prefix+'-'+job[0] for job in JOBS]+[args.prefix+'-channels',args.prefix+'-historical']
    assert all(not (ROOT/'runs'/name).exists() for name in names),'Evidence is immutable; choose a fresh prefix.'
    for (suffix,*job),name in zip(JOBS,names):
        subprocess.run([sys.executable,str(HERE/'run.py'),name,*job,'jtdx,hybrid,hybrid-specialist'],check=True)
    for preset in ['channels','historical']:
        subprocess.run([sys.executable,str(HERE/'run_files.py'),args.prefix+'-'+preset,preset,'native,jtdx,hybrid,hybrid-specialist'],check=True)


if __name__=='__main__':main()
