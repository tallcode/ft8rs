#!/usr/bin/env python3
"""Prepare a candidate per-message retention baseline from an explicit replay.

Only rows matching existing, profile-appropriate CSV reference targets become
requirements. Other-reference and unlabelled observations stay in the report;
they are never automatically promoted to truth or counted as confirmed gains.
"""
import argparse
import csv
from datetime import datetime, timedelta
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CORE = ROOT/'crates/ft8rs-core'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def table(path):
    return [line.split('\t') for line in path.read_text().splitlines()
            if line and not line.startswith('#')]


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--report',type=Path,required=True)
    parser.add_argument('--candidate',type=Path,required=True)
    parser.add_argument('--calldb',type=Path,required=True)
    parser.add_argument('--profile',choices=['fast','release'],required=True)
    args=parser.parse_args()
    candidate=args.candidate.resolve()
    accepted=(CORE/'tests/decoder/data').resolve()
    if candidate==accepted or candidate.is_relative_to(accepted):
        parser.error('prepare a candidate outside accepted data')
    output=candidate/'ft8/degraded'
    if (output/'retention.tsv').exists():
        parser.error('candidate already has retention expectations')
    conditions=json.loads((args.report/'conditions.json').read_text())
    assert conditions['profile']==args.profile
    assert conditions['calldb_sha256']==digest(args.calldb), 'different calibration CallDB'
    assert conditions['recordings_manifest_sha256']==digest(output/'recordings.tsv'), 'different native fixtures'
    manifest=table(output/'recordings.tsv')
    files={row[1] for row in manifest}
    curves=[row for row in table(args.report/'curves.tsv') if row[1] in files]
    registry={row[0]:row[1]=='true' for row in table(args.report/'adapters.tsv')}
    adapters=set(registry)
    assert adapters, 'missing decoder registry'
    keys={(name,file) for name in adapters for file in files}
    assert {(row[0],row[1]) for row in curves}==keys, 'incomplete replay matrix'
    assert len(curves)==len(keys), 'duplicate replay cases'
    required=['# adapter\tfile\tslot\tmessage\tfreq\tdt']
    counts={key:0 for key in keys}
    def normal(text):
        return ' '.join(token if token=='<...>' else token.strip('<>').upper() for token in text.split())
    originals={}
    for manifest_row in manifest:
        with (CORE/f'tests/ft8/{manifest_row[2]}.csv').open() as stream:
            originals[manifest_row[1]]=list(csv.DictReader(stream))
    sources={row[1]:row[2] for row in manifest}
    for row in table(args.report/'observations.tsv'):
        assert len(row)==7 and row[6] in ['target','other-reference','unlabelled']
        key=(row[0],row[1])
        if key in keys:
            timestamp=(datetime.strptime(sources[row[1]],'%y%m%d_%H%M%S')+timedelta(seconds=15*int(row[2]))).strftime('%y%m%d_%H%M%S')
            candidates=[r for r in originals[row[1]] if r['Date-Time']==timestamp and normal(r['Msg'])==row[3]]
            target='UA3QNA' if sources[row[1]]=='230208_140300' else 'F5RXL'
            def included(reference):
                marker=(reference.get('Extra') or '').strip()
                allowed=marker not in (['J','E'] if row[0]=='wsjtx' else ['W','E'] if row[0]=='jtdx' else ['E'])
                return allowed and (not registry[row[0]] or target in row[3].split())
            classification='target' if any(included(r) for r in candidates) else 'other-reference' if candidates else 'unlabelled'
            assert classification==row[6], f'incorrect reference classification: {row}'
        if key in keys and row[6]=='target':
            required.append('\t'.join(row[:6]))
            counts[key]+=1
    cases=['# adapter\tfile\trequired_rows\ttotal_reference_targets\tunlabelled_observations']
    for row in curves:
        key=(row[0],row[1])
        assert counts[key]==int(row[4])
        cases.append('\t'.join([row[0],row[1],str(counts[key]),row[5],row[6]]))
    (output/'retention.tsv').write_text('\n'.join(required)+'\n')
    (output/'retention-cases.tsv').write_text('\n'.join(cases)+'\n')
    metadata={
        'kind':'Rust decoder regression baseline, not a native decoder oracle',
        'profile':args.profile,'calldb_sha256':digest(args.calldb),
        'report_observations_sha256':digest(args.report/'observations.tsv'),
        'report_curves_sha256':digest(args.report/'curves.tsv'),
        'reference_csvs':{f'{name}.csv':digest(CORE/f'tests/ft8/{name}.csv')
                          for name in {row[2] for row in manifest}},
        'native_fixture_hashes':{row[1]:row[7] for row in manifest},
        'adapters':sorted(adapters),'cases':len(curves),'required_rows':len(required)-1,
        'artifacts':{name:digest(output/name) for name in ['retention.tsv','retention-cases.tsv']},
        'selection':'only profile-appropriate existing CSV targets; exact rows cannot be offset by additions',
    }
    (output/'retention.json').write_text(json.dumps(metadata,indent=2)+'\n')
    print(f'prepared {len(curves)} candidate cases, {len(required)-1} required rows; review curves and unlabelled observations before adoption')


if __name__=='__main__':
    main()
