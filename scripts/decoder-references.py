#!/usr/bin/env python3
"""Inspect, verify and compare versioned FT8 decoder reference data (no compiler).

Generation is deliberately separate: scripts/generate-ft8-reference.py.
No subcommand changes the accepted fixtures or the original decoder checkout.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import wave

DEFAULT = Path(__file__).resolve().parents[1] / 'crates/ft8rs-core/tests/decoder/data'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def rows(path):
    return [line.split('\t') for line in path.read_text().splitlines()
            if line and not line.startswith('#')]


def verify(root, require_retention=True):
    for path in sorted((root / 'ft8/upstream').glob('*/provenance.json')):
        meta = json.loads(path.read_text())
        for name, expected in meta['files'].items():
            assert digest(path.parent / name) == expected, f'corrupt upstream reference {name}'
        assert digest(root / 'ft8/protocol/wsjtx.tsv') == meta['legacy_protocol_sha256']
        print(f'current wsjtx reference: revision={meta["revision"]} verified')
    backends = sorted((root / 'ft8/provenance').glob('*.json'))
    assert backends, f'no provenance in {root}'
    for path in backends:
        meta = json.loads(path.read_text())
        name = meta['decoder']
        assert digest(root / f'ft8/protocol/{name}.tsv') == meta['protocol_sha256'], f'{name}: corrupt protocol vectors'
        assert digest(root / f'ft8/waveforms/{name}.wav') == meta['waveform_sha256'], f'{name}: corrupt clean waveform'
        vectors = rows(root / f'ft8/protocol/{name}.tsv')
        assert len(vectors) == meta['protocol_cases'], f'{name}: missing protocol cases'
        assert [row[0] for row in vectors] == (root / 'ft8/protocol/messages.txt').read_text().splitlines(), f'{name}: message corpus mismatch'
        assert len({row[0] for row in vectors}) == len(vectors), f'{name}: duplicate protocol inputs'
        for row in vectors:
            assert len(row) == 6 and len(row[2]) == 77 and len(row[3]) == 174 and len(row[4]) == 79
            assert set(row[2] + row[3]) <= {'0', '1'} and set(row[4]) <= set('01234567')
        audio = rows(root / f'ft8/audio/{name}.tsv')
        assert len(audio) == meta['audio_cases'], f'{name}: missing audio cases'
        for row in audio:
            assert len(row) == 8
            assert digest(root / 'ft8/audio' / row[1]) == row[7], f'{name}: corrupt {row[1]}'
        print(f'{name}: revision={meta["revision"]} protocol={len(vectors)} audio={len(audio)} verified')
    extended = sorted((root / 'ft8/extended-provenance').glob('*.json'))
    if extended:
        assert {p.stem for p in extended} == {p.stem for p in backends}, 'incomplete extended reference backends'
        inputs = (root / 'ft8/calls/inputs.txt').read_text().splitlines()
        for path in extended:
            meta = json.loads(path.read_text())
            name = meta['decoder']
            for relative, expected in meta['artifacts'].items():
                assert digest(root / relative) == expected, f'{name}: corrupt extended artifact {relative}'
            calls = rows(root / f'ft8/calls/{name}.tsv')
            assert len(calls) == meta['calls_cases']
            assert [r[1] for r in calls if r[0] == 'pack'] == inputs
            for row in calls:
                assert len(row) == 7 and row[0] in ['pack', 'unpack']
                assert 0 <= int(row[2]) < (1 << 28) and row[4] in ['T','F'] and row[6] in ['T','F']
            channels = rows(root / f'ft8/channels/{name}.tsv')
            assert len(channels) == meta['channel_cases']
            for row in channels:
                assert len(row) == 11 and row[9] in ['yes','no']
                audio_path = root / 'ft8/channels' / row[1]
                assert digest(audio_path) == row[10]
                with wave.open(str(audio_path)) as wav:
                    assert (wav.getnchannels(), wav.getsampwidth(), wav.getframerate(), wav.getnframes()) == (1, 2, 12000, 180000)
            print(f'{name}: extended calls={len(calls)} Watterson={len(channels)} verified')
        recordings = rows(root / 'ft8/degraded/recordings.tsv')
        assert len(recordings) == 6
        for row in recordings:
            assert len(row) == 8 and float(row[3]) > 0
            assert digest(root / 'ft8/degraded' / row[1]) == row[7]
        print(f'real recordings: {len(recordings)} fixed degraded fixtures verified')
        if require_retention:
            meta = json.loads((root / 'ft8/degraded/retention.json').read_text())
            assert meta['kind'] == 'Rust decoder regression baseline, not a native decoder oracle'
            for name, expected in meta['artifacts'].items():
                assert digest(root / 'ft8/degraded' / name) == expected, f'corrupt retention baseline {name}'
            required = rows(root / 'ft8/degraded/retention.tsv')
            cases = rows(root / 'ft8/degraded/retention-cases.tsv')
            assert len(required) == meta['required_rows'] and len(cases) == meta['cases']
            keys = {(adapter, row[1]) for adapter in meta['adapters'] for row in recordings}
            assert {(row[0], row[1]) for row in cases} == keys and len(cases) == len(keys)
            for case in cases:
                assert len(case) == 5
                assert sum(row[:2] == case[:2] for row in required) == int(case[2])
            assert {row[1]:row[7] for row in recordings} == meta['native_fixture_hashes']
            print(f'retention: {len(cases)} calibrated cases, {len(required)} required message rows verified')
    current_audio = root / 'ft8/waveforms/rr73-current.json'
    if current_audio.exists():
        meta = json.loads(current_audio.read_text())
        native = json.loads((root / 'ft8/upstream/wsjtx-3.3/provenance.json').read_text())
        assert meta['revision'] == native['revision']
        assert meta['rr73_field'] == int(meta['bits77'][59:74], 2) == 32403
        assert digest(current_audio.with_suffix('.wav')) == meta['waveform_sha256']
        with wave.open(str(current_audio.with_suffix('.wav'))) as wav:
            assert (wav.getnchannels(), wav.getsampwidth(), wav.getframerate(), wav.getnframes()) == (1, 2, 12000, 180000)
        print('Current native RR73 AWGN waveform: 32403 verified')
    for stem, field in [('dx-current-payloads', 32403)]:
        dx_meta = root / f'ft8/waveforms/{stem}.provenance.json'
        if not dx_meta.exists():
            continue
        dx = json.loads(dx_meta.read_text())
        payloads = dx_meta.with_name(stem + '.tsv')
        assert digest(payloads) == dx['payload_sha256']
        fixed = rows(payloads)
        assert len(fixed) == dx['cases'] == 11
        assert len({r[0] for r in fixed}) == 11
        for message, bits, tones in fixed:
            assert len(bits) == 77 and set(bits) <= {'0', '1'}
            assert len(tones) == 79 and set(tones) <= set('01234567')
            if message == 'K1JT BG5ATV RR73':
                assert int(bits[59:74], 2) == field == dx['rr73_field']
        if field == 32403:
            native = json.loads((root / 'ft8/upstream/wsjtx-3.3/provenance.json').read_text())
            assert dx['revision'] == native['revision']
            assert all(native['sources'][name] == sha for name, sha in dx['sources'].items())
        print(f'DX waveform inputs: {stem}, 11 native payloads verified, RR73={field}')
    policy_path = root / 'ft8/upstream-retention/policy.json'
    if policy_path.exists():
        policy = json.loads(policy_path.read_text())
        native = json.loads((root / 'ft8/upstream/wsjtx-3.3/provenance.json').read_text())
        assert policy['revision'] == native['revision'], 'retention exceptions need review for this upstream revision'
        for name, expected_hash in policy['files'].items():
            assert digest(policy_path.parent / name) == expected_hash, f'corrupt retention policy {name}'
        exceptions = rows(policy_path.parent / 'exceptions.tsv')
        assert len(exceptions) == policy['observations'] == 10
        assert len({tuple(r) for r in exceptions}) == len(exceptions)
        assert len({r[4] for r in exceptions}) == policy['unique_messages'] == 5
        recordings = {r[1]: r[7] for r in rows(root / 'ft8/degraded/recordings.tsv')}
        required = rows(root / 'ft8/degraded/retention.tsv') if require_retention else None
        for adapter, recording, recording_hash, slot, message, reason in exceptions:
            assert adapter in ['wsjtx', 'hybrid', 'hybrid-exp']
            assert recording_hash == recordings[recording]
            assert reason in ['legacy_rr73_32373', 'a7_qu1rk_runner_up']
            base_adapter = 'hybrid' if adapter == 'hybrid-exp' else adapter
            if required is not None:
                assert any(r[:4] == [base_adapter, recording, slot, message] for r in required), 'exception is not a frozen required identity'
        assert sum(r[0] != 'hybrid-exp' for r in exceptions) == policy['calibrated_observations'] == 8
        print('upstream retention: 5 approved identities, 8 calibrated + 2 supplemental observations; all other rows remain mandatory')
    return backends


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    check = sub.add_parser('verify')
    check.add_argument('--data', type=Path, default=DEFAULT)
    check.add_argument('--fixtures-only', action='store_true', help='Check native generation before Rust retention calibration')
    diff = sub.add_parser('diff')
    diff.add_argument('--baseline', type=Path, default=DEFAULT)
    diff.add_argument('--candidate', type=Path, required=True)
    status = sub.add_parser('status')
    status.add_argument('--data', type=Path, default=DEFAULT)
    status.add_argument('--decoder', choices=['wsjtx', 'jtdx'], required=True)
    status.add_argument('--source', type=Path, required=True)
    upstream = sub.add_parser('audit-upstream', help='Verify the pinned current WSJT-X source layer')
    upstream.add_argument('--data', type=Path, default=DEFAULT)
    upstream.add_argument('--source', type=Path, required=True)
    runs = sub.add_parser('compare-runs', help='Fail if any previously decoded message is lost')
    runs.add_argument('--before', type=Path, required=True)
    runs.add_argument('--after', type=Path, required=True)
    args = parser.parse_args()
    if args.command == 'verify':
        verify(args.data, require_retention=not args.fixtures_only)
    elif args.command == 'diff':
        verify(args.baseline)
        verify(args.candidate)
        old = {p.relative_to(args.baseline): digest(p) for p in args.baseline.rglob('*') if p.is_file()}
        new = {p.relative_to(args.candidate): digest(p) for p in args.candidate.rglob('*') if p.is_file()}
        for path in sorted(old.keys() | new.keys()):
            if old.get(path) == new.get(path):
                continue
            kind = 'added' if path not in old else 'removed' if path not in new else 'changed'
            print(f'{kind}: {path}')
            if str(path).startswith('ft8/protocol/') and path.suffix == '.tsv' and path in old and path in new:
                before = {r[0]: r[1:] for r in rows(args.baseline / path)}
                after = {r[0]: r[1:] for r in rows(args.candidate / path)}
                for message in sorted(before.keys() | after.keys()):
                    if before.get(message) != after.get(message):
                        print(f'  protocol changed: {message!r}')
    elif args.command == 'compare-runs':
        before_conditions = args.before / 'conditions.json'
        after_conditions = args.after / 'conditions.json'
        if before_conditions.exists() or after_conditions.exists():
            assert before_conditions.exists() and after_conditions.exists(), 'one report is missing conditions.json'
            assert json.loads(before_conditions.read_text()) == json.loads(after_conditions.read_text()), 'different build profile, input or CallDB'
        old_files = {p.name: p for p in args.before.glob('*.tsv')}
        new_files = {p.name: p for p in args.after.glob('*.tsv')}
        assert old_files and old_files.keys() == new_files.keys(), 'recording reports are incomplete'
        total_lost = 0
        for name, old_path in sorted(old_files.items()):
            before = rows(old_path)
            after = rows(new_files[name])
            old_headers = [line for line in old_path.read_text().splitlines() if line.startswith('#')]
            new_headers = [line for line in new_files[name].read_text().splitlines() if line.startswith('#')]
            assert old_headers == new_headers, f'{name}: different config/input/CallDB'
            used = set()
            lost = []
            for old in before:
                match = next((i for i, new in enumerate(after) if i not in used and
                              old[:2] == new[:2] and abs(float(old[2])-float(new[2])) <= 5 and
                              abs(float(old[3])-float(new[3])) <= 0.3), None)
                if match is None:
                    lost.append(old)
                else:
                    used.add(match)
            gains = [row for i, row in enumerate(after) if i not in used]
            print(f'{name}: before={len(before)} after={len(after)} lost={len(lost)} added={len(gains)}')
            for row in lost:
                print(f'  LOST: {row}')
            for row in gains:
                print(f'  ADDED (requires truth validation): {row}')
            total_lost += len(lost)
        if total_lost:
            raise SystemExit(1)
    elif args.command == 'audit-upstream':
        verify(args.data)
        meta = json.loads((args.data / 'ft8/upstream/wsjtx-3.3/provenance.json').read_text())
        sources = {**meta['sources'], **meta['decoder_sources']}
        current_audio = args.data / 'ft8/waveforms/rr73-current.json'
        if current_audio.exists():
            extra = json.loads(current_audio.read_text())
            for name, expected in extra['sources'].items():
                assert name not in sources or sources[name] == expected
                sources[name] = expected
        changed = [name for name, expected in sources.items()
                   if not (args.source / name).is_file() or digest(args.source / name) != expected]
        if changed:
            raise SystemExit('Pinned WSJT-X sources differ:\n' + '\n'.join(changed))
        print(f'Pinned current WSJT-X sources match {meta["revision"]}: {len(sources)} files')
    else:
        meta = json.loads((args.data / f'ft8/provenance/{args.decoder}.json').read_text())
        revision = subprocess.check_output(['git', '-C', args.source, 'rev-parse', 'HEAD'], text=True).strip()
        extension = args.data / f'ft8/extended-provenance/{args.decoder}.json'
        sources = dict(meta['sources'])
        if extension.exists():
            extra = json.loads(extension.read_text())
            for name, expected in extra['sources'].items():
                if name in sources:
                    assert sources[name] == expected, f'{name}: base and extended data use different sources'
                sources[name] = expected
        changed = [name for name, expected in sources.items()
                   if not (args.source / name).is_file() or digest(args.source / name) != expected]
        print(f'accepted revision: {meta["revision"]}\ncheckout revision: {revision}')
        if changed:
            print('FT8 reference sources changed:\n' + '\n'.join(changed))
            raise SystemExit(1)
        print('Recorded FT8 sources unchanged; regeneration is unnecessary.')


if __name__ == '__main__':
    main()
