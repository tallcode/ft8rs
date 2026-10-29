#!/usr/bin/env python3
"""Derive summaries from completed raw runs. Never change evidence or thresholds."""
import collections
import csv
import datetime
import json
import math
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
REPO = next(p for p in HERE.parents if (p / "crates/ft8rs-core").is_dir())
BASE = REPO / "experiments/single-slot"
ROOT = pathlib.Path('/tmp/ft8rs-single-slot-lab')


def percentile(values, q):
    if not values:
        return None
    a = sorted(values)
    pos = (len(a)-1)*q
    lo = int(pos)
    return a[lo] + (a[min(lo+1, len(a)-1)]-a[lo])*(pos-lo)


def wilson(k, n):
    if not n:
        return [None, None]
    z = 1.959963984540054
    p = k/n
    center = (p+z*z/(2*n))/(1+z*z/n)
    half = z*math.sqrt(p*(1-p)/n+z*z/(4*n*n))/(1+z*z/n)
    return [center-half, center+half]


def callback_records(path):
    current = None
    result = collections.defaultdict(list)
    for line in path.read_text().splitlines():
        parts = line.split('\t')
        if parts[0] == 'TRUTH':
            current = (parts[1], parts[2], parts[3], float(parts[4]), float(parts[5]))
        elif parts[0] == 'CALLBACK' and current and len(parts) >= 6:
            seed, snr, text, freq, dt = current
            if parts[3] == text and abs(float(parts[4])-freq) <= 5 and abs(float(parts[5])-dt) <= .3:
                result[(seed, snr, parts[1])].append(float(parts[2]))
    return result


def synthetic_runs():
    results = {}
    for folder in sorted((ROOT / 'runs').iterdir()):
        if folder.name.startswith('hybrid-own-') != (HERE.parent.name == '02-hybrid-specialist'):
            continue
        if not (folder / 'rows.tsv').exists() or not (folder / 'run.json').exists():
            continue
        meta = json.loads((folder / 'run.json').read_text())
        if meta.get('returncode') != 0:
            continue
        rows = list(csv.DictReader((folder / 'rows.tsv').open(), delimiter='\t'))
        groups = collections.defaultdict(list)
        by_method = collections.defaultdict(dict)
        for row in rows:
            method, snr = row['method'], row['snr']
            groups[(method, snr)].append(row)
            by_method[method][(row['seed'], snr)] = row
        callbacks = callback_records(folder / 'stderr.txt')
        summaries = []
        for (method, snr), group in sorted(groups.items()):
            correct = sum(int(r['correct']) for r in group)
            times = [float(r['ms']) for r in group]
            cb = [callbacks[(r['seed'], r['snr'], method.removeprefix('blind-'))]
                  for r in group]
            cb = [min(x) for x in cb if x]
            summaries.append(dict(method=method, snr=float(snr), n=len(group), correct=correct,
                                  wrong=sum(int(r['wrong']) for r in group),
                                  error_slots=sum(int(r['wrong']) > 0 for r in group),
                                  interval_95=wilson(correct, len(group)),
                                  wall_ms_p50=percentile(times, .5), wall_ms_p95=percentile(times, .95),
                                  wall_ms_max=max(times), target_callback_ms_p50=percentile(cb, .5),
                                  target_callback_ms_max=max(cb) if cb else None))
        paired = []
        for base in ['native', 'blind-hybrid', 'blind-hybrid-swl']:
            for method, observations in by_method.items():
                if method == base or base not in by_method:
                    continue
                shared = observations.keys() & by_method[base].keys()
                for snr in sorted({key[1] for key in shared}, key=float):
                    subset = [key for key in shared if key[1] == snr]
                    b = [int(by_method[base][k]['correct']) for k in subset]
                    c = [int(observations[k]['correct']) for k in subset]
                    paired.append(dict(baseline=base, method=method, snr=float(snr), n=len(subset),
                                       baseline_correct=sum(b), method_correct=sum(c),
                                       retained=sum(x and y for x, y in zip(b, c)),
                                       lost=sum(x and not y for x, y in zip(b, c)),
                                       gained=sum(y and not x for x, y in zip(b, c)),
                                       union_correct=sum(x or y for x, y in zip(b, c))))
        results[folder.name] = dict(metadata=meta, summary=summaries, paired=paired)
    return results


def same(a, b):
    return ' '.join(a['msg'].split()) == ' '.join(b['msg'].split()) and abs(a['freq']-b['freq']) <= 5 and abs(a['dt']-b['dt']) <= .3


def read_file_rows(path):
    rows = collections.defaultdict(list)
    for line in path.read_text().splitlines():
        p = line.split('\t')
        if p[0] == 'FILE_ROW':
            rows[p[3]].append(dict(msg=p[4], freq=float(p[5]), dt=float(p[6]), source=p[7]))
    return rows


def annotations(path, slot):
    if path.parent.name == 'channel-degraded':
        r = json.loads((path.parent / 'manifest.json').read_text())[path.name]
        return [dict(msg=r['message'], freq=r['frequency'], dt=r['dt'], required=False)], []
    manifest = REPO / 'crates/ft8rs-core/tests/decoder/data/ft8/channels' / (path.name.split('_')[0]+'.tsv')
    if 'channels' in path.parts:
        for line in manifest.read_text().splitlines():
            if line.startswith('#'):
                continue
            p = line.split('\t')
            if p[1] == path.name:
                return [dict(msg=p[2], freq=float(p[3]), dt=float(p[4]), required=p[9] == 'yes')], []
        raise ValueError(path)
    stamp = datetime.datetime.strptime(path.stem, '%y%m%d_%H%M%S')+datetime.timedelta(seconds=15*slot)
    stamp = stamp.strftime('%y%m%d_%H%M%S')
    truth, flagged = [], []
    for r in csv.DictReader(path.with_suffix('.csv').open()):
        if r['Date-Time'] != stamp:
            continue
        row = dict(msg=r['Msg'], freq=float(r['Freq']), dt=float(r['Drift']))
        (flagged if (r.get('Extra') or '').strip() == 'E' else truth).append(row)
    return truth, flagged


def file_runs():
    results = {}
    for folder in sorted((ROOT / 'runs').iterdir()):
        if folder.name.startswith('hybrid-own-') != (HERE.parent.name == '02-hybrid-specialist'):
            continue
        if not (folder / '000.tsv').exists() or not (folder / 'run.json').exists():
            continue
        meta = json.loads((folder / 'run.json').read_text())
        tasks = []
        for i, task in enumerate(meta['tasks']):
            source = pathlib.Path(task['command'][2])
            truth, flagged = annotations(source, task['slot'])
            rows = read_file_rows(folder / f'{i:03d}.stderr')
            times = {r['method']: float(r['ms']) for r in csv.DictReader((folder / f'{i:03d}.tsv').open(), delimiter='\t')}
            methods = {}
            for method in task['command'][3].split(','):
                outputs = rows[method]
                methods[method] = dict(raw=len(outputs), annotated=sum(any(same(t, r) for r in outputs) for t in truth),
                                       flagged=sum(any(same(t, r) for t in flagged) for r in outputs),
                                       unannotated=sum(not any(same(t, r) for t in truth+flagged) for r in outputs),
                                       wall_ms=times[method], rows=outputs)
            comparisons = []
            for method in methods:
                if method == 'hybrid' or 'hybrid' not in methods:
                    continue
                old = rows['hybrid']
                new = rows[method]
                retained = sum(any(same(r, x) for x in new) for r in old)
                added = [r for r in new if not any(same(r, x) for x in old)]
                comparisons.append(dict(method=method, baseline_rows=len(old), retained=retained,
                                         lost=len(old)-retained, added=len(added),
                                         annotated_additions=sum(any(same(r, t) for t in truth) for r in added),
                                         additions=added))
            tasks.append(dict(input=source.name, slot=task['slot'], truth=len(truth),
                              methods=methods, comparisons=comparisons))
        results[folder.name] = dict(metadata=meta, tasks=tasks)
    return results


def main():
    destination = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else HERE.parent / 'analysis'
    destination.mkdir(parents=True, exist_ok=False)
    result = dict(synthetic=synthetic_runs(), files=file_runs())
    (destination / 'analysis.json').write_text(json.dumps(result, indent=2)+'\n')
    with (destination / 'matrix.tsv').open('w') as f:
        columns = ['run', 'method', 'snr', 'n', 'correct', 'wrong', 'error_slots', 'wall_ms_p50', 'wall_ms_p95', 'wall_ms_max', 'target_callback_ms_p50', 'target_callback_ms_max']
        writer = csv.DictWriter(f, fieldnames=columns, delimiter='\t', extrasaction='ignore')
        writer.writeheader()
        for name, run in result['synthetic'].items():
            for row in run['summary']:
                writer.writerow(dict(run=name, **row))
    print(destination / 'analysis.json')


if __name__ == '__main__':
    main()
