#!/usr/bin/env python3
"""Serial runs with exact source/binary snapshots and unedited row output."""
import collections
import csv
import hashlib
import json
import os
import pathlib
import resource
import shutil
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
ROOT = pathlib.Path('/tmp/ft8rs-single-slot-lab')
BIN = ROOT / 'core/target/release/ft8rs-receiver-lab'


def main():
    name, trials, seed, scenario, *extra = sys.argv[1:]
    if name.startswith('hybrid-own-'):
        subprocess.run([sys.executable,str(HERE / 'preserve_campaigns.py')],check=True)
    if (ROOT / 'STOP_AFTER_JOB').exists():
        raise SystemExit((ROOT / 'STOP_AFTER_JOB').read_text())
    out = ROOT / 'runs' / name
    out.mkdir(parents=True, exist_ok=False)
    receiver = ROOT / 'core/src/receiver.rs'
    if not receiver.exists():
        receiver = ROOT / 'core/src/lab.rs'
    shutil.copy2(receiver, out / 'receiver.rs')
    shutil.copy2(ROOT / 'environment.json', out / 'environment.json')
    for name_in_core in ['Cargo.toml', 'Cargo.lock', 'src/lab_bridge.rs']:
        source = ROOT / 'core' / name_in_core
        if source.exists():
            shutil.copy2(source, out / pathlib.Path(name_in_core).name)
    shutil.copy2(HERE / 'prepare.py', out / 'prepare.py')
    shutil.copytree(ROOT / 'core/src', out / 'core-source')
    command = [str(BIN), trials, seed, scenario] + extra
    binary_hash = hashlib.sha256(BIN.read_bytes()).hexdigest()
    environment = json.loads((ROOT / 'environment.json').read_text())
    assert environment.get('build_complete', True), 'The last build did not succeed.'
    if 'binary_sha256' in environment:
        assert binary_hash == environment['binary_sha256'], 'Binary changed after the recorded build.'
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = time.monotonic()
    with (out / 'rows.tsv').open('w') as stdout, (out / 'stderr.txt').open('w') as stderr:
        result = subprocess.run(command, stdout=stdout,
                                stderr=stderr, cwd=BIN.parent,
                                env=dict(os.environ, FT8RS_LAB_AUDIO_DIR=str(out / 'waveforms')))
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    metadata = dict(command=command,
                    wall_seconds=time.monotonic()-start,
                    cpu_seconds=(after.ru_utime+after.ru_stime)-(before.ru_utime+before.ru_stime),
                    visible_cpus=os.cpu_count(), binary_sha256=binary_hash,
                    returncode=result.returncode)
    (out / 'run.json').write_text(json.dumps(metadata, indent=2)+'\n')
    if result.returncode:
        print((out / 'stderr.txt').read_text())
        raise SystemExit(result.returncode)
    groups = collections.defaultdict(lambda: [0, 0, 0, []])
    with (out / 'rows.tsv').open() as f:
        for row in csv.DictReader(f, delimiter='\t'):
            g = groups[(row['method'], row['snr'])]
            g[0] += int(row['correct']); g[1] += int(row['wrong']); g[2] += 1
            g[3].append(float(row['ms']))
    with (out / 'summary.tsv').open('w') as f:
        f.write('method\tsnr\tcorrect\twrong\ttrials\tmean_ms\tmax_ms\n')
        for (method, snr), (correct, wrong, n, times) in sorted(groups.items()):
            f.write(f'{method}\t{snr}\t{correct}\t{wrong}\t{n}\t{sum(times)/n:.3f}\t{max(times):.3f}\n')
    print(json.dumps(metadata))
    print(out / 'summary.tsv')
    if name.startswith('hybrid-own-'):
        subprocess.run([sys.executable,str(HERE / 'preserve_campaigns.py')],check=True)


if __name__ == '__main__':
    main()
