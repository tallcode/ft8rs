#!/usr/bin/env python3
"""Serial native-fixture/real-recording replay with row and callback evidence."""
import hashlib
import json
import pathlib
import shutil
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
REPO = next(p for p in HERE.parents if (p / "crates/ft8rs-core").is_dir())
BASE = REPO / "experiments/single-slot"
ROOT = pathlib.Path('/tmp/ft8rs-single-slot-lab')
BIN = ROOT / 'core/target/release/ft8rs-receiver-lab'


def main():
    name, preset, methods = sys.argv[1:]
    if name.startswith('hybrid-own-'):
        subprocess.run([sys.executable,str(HERE / 'preserve_campaigns.py')],check=True)
    out = ROOT / 'runs' / name
    out.mkdir(parents=True, exist_ok=False)
    for source in ['receiver.rs', 'lab_bridge.rs']:
        shutil.copy2(ROOT / 'core/src' / source, out / source)
    shutil.copy2(ROOT / 'environment.json', out / 'environment.json')
    shutil.copytree(ROOT / 'core/src', out / 'core-source')
    if preset == 'channels':
        tasks = [(p, 0) for p in sorted((REPO / 'crates/ft8rs-core/tests/decoder/data/ft8/channels').glob('*.wav'))]
    elif preset == 'historical':
        folder = REPO / 'crates/ft8rs-core/tests/ft8'
        tasks = [(folder / '210703_133430.wav', 0)] + [(folder / '230208_140300.wav', i) for i in range(19)]
    elif preset == 'short':
        tasks = [(REPO / 'crates/ft8rs-core/tests/ft8/210703_133430.wav', 0)]
    elif preset == 'degraded':
        tasks = [(p, 0) for p in sorted((ROOT / 'channel-degraded').glob('*.wav'))]
        assert len(tasks) == 24
    else:
        raise ValueError(preset)
    metadata = {'methods': methods, 'expected_tasks': len(tasks), 'binary_sha256': hashlib.sha256(BIN.read_bytes()).hexdigest(), 'tasks': []}
    for i, (path, slot) in enumerate(tasks):
        start = time.monotonic()
        command = [str(BIN), 'file', str(path), methods, str(slot)]
        with (out / f'{i:03d}.tsv').open('w') as stdout, (out / f'{i:03d}.stderr').open('w') as stderr:
            result = subprocess.run(command, stdout=stdout, stderr=stderr, cwd=BIN.parent)
        task = {'command': command, 'input_sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                'slot': slot, 'wall_seconds': time.monotonic()-start, 'returncode': result.returncode}
        metadata['tasks'].append(task)
        (out / 'run.json').write_text(json.dumps(metadata, indent=2)+'\n')
        print(f'{name}: {i+1}/{len(tasks)} {path.name} slot {slot}: {task["wall_seconds"]:.2f}s', flush=True)
        if result.returncode:
            raise SystemExit(result.returncode)
    if name.startswith('hybrid-own-'):
        subprocess.run([sys.executable,str(HERE / 'preserve_campaigns.py')],check=True)


if __name__ == '__main__':
    main()
