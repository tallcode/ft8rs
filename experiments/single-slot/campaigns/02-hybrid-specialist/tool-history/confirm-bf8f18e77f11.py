#!/usr/bin/env python3
"""Replay the frozen confirmation matrix into fresh immutable run directories."""
import hashlib
import argparse
import json
import pathlib
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
ROOT = pathlib.Path('/tmp/ft8rs-single-slot-lab')
JOBS_V8 = [
    ('holdout-awgn-v8', '32', '2026102991', 'sic-fresh-awgn', '-20,-21,-22', 'hybrid,hybrid-swl,receiver'),
    ('holdout-mix-v8', '8', '2026102991', 'sic-fresh-mix', '-18,-20,-22', 'hybrid,receiver'),
    ('holdout-cw-v8', '8', '2026102991', 'sic-fresh-cw', '-18,-20,-22', 'hybrid,receiver'),
    ('holdout-mixfade-v8', '8', '2026102991', 'sic-fresh-mixfade', '-18,-20', 'hybrid,receiver'),
    ('holdout-mix2-v8', '8', '2026102991', 'sic-fresh-mix2', '-18,-20', 'hybrid,receiver'),
    ('holdout-noise-v8', '32', '2026103041', 'sic-fresh-noise', '-24', 'hybrid,receiver'),
    ('holdout-cwonly-v8', '32', '2026103051', 'sic-fresh-cwonly', '-24', 'hybrid,receiver'),
    ('holdout-blocker-v8', '32', '2026103061', 'sic-fresh-blocker', '-24', 'hybrid,receiver'),
]

JOBS_V10 = [
    ('observed-holdout-awgn-v10', '32', '2026104991', 'sic-fresh-awgn', '-21,-22,-23', 'hybrid,hybrid-swl,conditional'),
    ('observed-holdout-mix-v10', '8', '2026104991', 'sic-fresh-mix', '-18,-20,-22', 'hybrid,conditional'),
    ('observed-holdout-cw-v10', '8', '2026104991', 'sic-fresh-cw', '-18,-20,-22', 'hybrid,conditional'),
    ('observed-holdout-mixfade-v10', '8', '2026104991', 'sic-fresh-mixfade', '-18,-20', 'hybrid,conditional'),
    ('observed-holdout-mix2-v10', '8', '2026104991', 'sic-fresh-mix2', '-18,-20', 'hybrid,conditional'),
    ('observed-holdout-noise-v10', '32', '2026105041', 'sic-fresh-noise', '-24', 'hybrid,conditional'),
    ('observed-holdout-cwonly-v10', '32', '2026105051', 'sic-fresh-cwonly', '-24', 'hybrid,conditional'),
    ('observed-holdout-blocker-v10', '32', '2026105061', 'sic-fresh-blocker', '-24', 'hybrid,conditional'),
    ('observed-holdout-targetdrift-v10', '8', '2026104991', 'sic-fresh-drift', '-18,-20', 'hybrid,conditional'),
]


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('prefix',nargs='?',default='replay')
    parser.add_argument('--version',choices=['v8','v10'],default='v10')
    options=parser.parse_args()
    prefix=options.prefix
    jobs=JOBS_V10 if options.version=='v10' else JOBS_V8
    receiver='receiver_observed.rs' if options.version=='v10' else 'receiver.rs'
    freeze_path = HERE / f'campaigns/01-single-slot-research/freeze-{options.version}.json'
    if not freeze_path.exists():
        freeze_path = ROOT / f'freeze-{options.version}.json'
    freeze = json.loads(freeze_path.read_text())
    assert hashlib.sha256((HERE / receiver).read_bytes()).hexdigest() == freeze['receiver_sha256']
    assert (ROOT/'core/src/receiver.rs').read_bytes()==(HERE/receiver).read_bytes(), 'Build the selected version first.'
    for name, *_ in jobs:
        assert not (ROOT / 'runs' / f'{prefix}-{name}').exists(), 'Choose a fresh prefix; evidence is immutable.'
    for name, *args in jobs:
        subprocess.run([sys.executable, str(HERE / 'run.py'), f'{prefix}-{name}', *args], check=True)
    if options.version=='v10':
        subprocess.run([sys.executable,str(HERE/'run_files.py'),f'{prefix}-channels-final-v10','channels','native,conditional'],check=True)
        subprocess.run([sys.executable,str(HERE/'run_files.py'),f'{prefix}-historical-final-v10','historical','hybrid,conditional,hybrid-conditional'],check=True)
    else:
        subprocess.run([sys.executable, str(HERE / 'run_files.py'), f'{prefix}-historical-confirm-v8',
                        'historical', 'hybrid,receiver,hybrid-plus'], check=True)


if __name__ == '__main__':
    main()
