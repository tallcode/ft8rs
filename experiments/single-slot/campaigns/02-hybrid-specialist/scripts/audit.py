#!/usr/bin/env python3
"""Verify final evidence coverage and native invariants before reporting results."""
import csv
import hashlib
import json
import pathlib
import subprocess

from confirm import JOBS_V10
from prepare import fingerprints

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent.parent
ROOT = pathlib.Path('/tmp/ft8rs-single-slot-lab')


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    freeze = json.loads((ROOT / 'freeze-v10.json').read_text())
    environment = json.loads((ROOT / 'environment.json').read_text())
    core = REPO / 'crates/ft8rs-core'
    assert fingerprints(core) == environment['native_sha256']
    assert fingerprints(ROOT / 'core') == environment['native_sha256']
    assert len(environment['native_sha256']) == 82
    assert sha(core / 'ALLCALL7.TXT') == environment['database_sha256']
    assert sha(ROOT / 'core/target/release/ALLCALL7.TXT') == environment['database_sha256']
    assert sha(HERE / 'receiver_observed.rs') == freeze['receiver_sha256']
    assert sha(ROOT / 'core/src/receiver.rs') == freeze['receiver_sha256']
    assert sha(ROOT / 'core/target/release/ft8rs-receiver-lab') == freeze['binary_sha256']
    changed = subprocess.check_output(['git', 'diff', '--name-only', '--', 'crates', 'scripts',
                                       'Cargo.toml', 'Cargo.lock', '.cargo'], cwd=REPO, text=True).strip()
    assert not changed, f'Production/input changes need review: {changed}'
    jobs = []
    negatives = set()
    for name, trials, seed, scenario, levels, methods in JOBS_V10:
        folder = ROOT / 'runs' / name
        meta = json.loads((folder / 'run.json').read_text())
        assert meta['returncode'] == 0
        assert meta['binary_sha256'] == freeze['binary_sha256']
        assert meta['command'][1:] == [trials, seed, scenario, levels, methods]
        assert sha(folder / 'receiver.rs') == freeze['receiver_sha256']
        rows = list(csv.DictReader((folder / 'rows.tsv').open(), delimiter='\t'))
        expected_methods = {'native'} | {'blind-'+m for m in methods.split(',')}
        assert {r['method'] for r in rows} == expected_methods
        per_method = {}
        for method in expected_methods:
            subset = [r for r in rows if r['method'] == method]
            assert len(subset) == int(trials)*len(levels.split(','))
            keys = {(r['seed'], r['snr']) for r in subset}
            assert len(keys) == len(subset)
            assert all(int(r['correct']) in (0,1) and int(r['wrong']) >= 0 for r in subset)
            per_method[method] = dict(cases=len(subset), correct=sum(int(r['correct']) for r in subset),
                                      wrong=sum(int(r['wrong']) for r in subset),
                                      max_wall_ms=max(float(r['ms']) for r in subset))
        if any(x in scenario for x in ['-noise','-cwonly','-blocker']):
            keys = {r['seed'] for r in rows if r['method']=='blind-conditional'}
            assert not (keys & negatives)
            negatives |= keys
            assert per_method['blind-conditional']['correct'] == 0
        jobs.append(dict(name=name, methods=per_method))
    assert len(negatives) == 96
    for name, count in [('channels-final-v10',16),('historical-final-v10',20)]:
        folder = ROOT / 'runs' / name
        meta = json.loads((folder / 'run.json').read_text())
        assert len(meta['tasks']) == count
        assert meta['binary_sha256'] == freeze['binary_sha256']
        assert all(t['returncode']==0 for t in meta['tasks'])
        assert sha(folder / 'receiver.rs') == freeze['receiver_sha256']
    analysis = json.loads((ROOT / 'analysis/analysis.json').read_text())
    history = analysis['files']['historical-final-v10']['tasks']
    union_lost = sum(c['lost'] for t in history for c in t['comparisons'] if c['method']=='hybrid-conditional')
    assert union_lost == 0
    check = (ROOT / 'self-check-v10.tsv').read_text()
    assert 'noise observability, silence rejection and payload identity checks passed' in check
    result = dict(coverage_complete=True, native_sources_unchanged=82, runtime_database_unchanged=True,
                  production_sources_unchanged=True, unique_negative_seeds=len(negatives),
                  historical_baseline_lost=union_lost, synthetic_jobs=jobs, freeze=freeze,
                  limits=['Synthetic evidence does not establish a universal false-positive rate.',
                          'Cold single-slot replay is not the persistent/staged product gate.',
                          'Standalone head may lose native results; retain original output.',
                          'Timings exclude capture, GUI and real-time queueing; x86 not measured.'])
    destination = ROOT / 'analysis/audit.json'
    destination.write_text(json.dumps(result, indent=2)+'\n')
    print(destination)


if __name__ == '__main__':
    main()
