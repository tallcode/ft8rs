#!/usr/bin/env python3
"""Audit the frozen second campaign against its source, binary, raw inputs and output."""
import hashlib
import json
import pathlib
import re

HERE=pathlib.Path(__file__).resolve().parent
REPO=next(p for p in HERE.parents if (p / "crates/ft8rs-core").is_dir())
BASE=REPO/"experiments/single-slot"
ROOT=pathlib.Path('/tmp/ft8rs-single-slot-lab')


def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()


def frozen_source(relative):
    campaign=BASE/'campaigns/02-hybrid-specialist'
    if relative.startswith('crates/ft8rs-core/src/'):
        data=(campaign/'frozen/src'/relative.removeprefix('crates/ft8rs-core/src/')).read_bytes()
        if relative.endswith('/src/lib.rs'):data=data.removesuffix(b'\npub mod lab_bridge;\n')
        return data
    if relative=='experiments/single-slot/receiver_hybrid.rs':return (campaign/'sources/receiver_hybrid.rs').read_bytes()
    return (campaign/'frozen/product-source'/relative).read_bytes()


def main():
    destination=BASE/'campaigns/02-hybrid-specialist/analysis/hybrid-specialist-v1-audit.json'
    if destination.exists():raise SystemExit('Audit already exists; do not overwrite')
    freeze=json.loads((BASE/'campaigns/02-hybrid-specialist/freeze-hybrid-specialist-v1.json').read_text())
    for relative,digest in freeze['source_sha256'].items():assert hashlib.sha256(frozen_source(relative)).hexdigest()==digest,relative
    environment=json.loads((ROOT/'environment.json').read_text())
    for relative,digest in environment['native_sha256'].items():
        assert sha(REPO/'crates/ft8rs-core'/relative)==digest,relative
        assert sha(ROOT/'core'/relative)==digest,relative
    assert sha(ROOT/'core/target/release/ft8rs-receiver-lab')==freeze['binary_sha256']
    assert sha(ROOT/'core/target/release/ALLCALL7.TXT')==freeze['database_sha256']
    analysis=json.loads((BASE/'campaigns/02-hybrid-specialist/analysis/hybrid-specialist-v1.json').read_text())
    files=json.loads((BASE/'campaigns/02-hybrid-specialist/analysis/hybrid-specialist-files-v1.json').read_text())
    cases=0;waveforms=0;exact_losses=[];errors=0;gains=0
    for run in analysis['runs']:
        assert run['metadata']['binary_sha256']==freeze['binary_sha256']
        assert run['metadata']['returncode']==0
        folder=ROOT/'runs'/run['name']
        cases+=run['distinct_cases']
        waves=list((folder/'waveforms').glob('*.wav'))
        assert len(waves)==run['distinct_cases'];waveforms+=len(waves)
        rows={'hybrid':set(),'hybrid-specialist':set()}
        for line in (folder/'stderr.txt').read_text().splitlines():
            p=line.split('\t')
            if p[0]=='ROW' and p[3] in rows:rows[p[3]].add(tuple(p[1:3]+p[4:]))
        missing=rows['hybrid']-rows['hybrid-specialist']
        if missing:exact_losses.append({'run':run['name'],'rows':list(missing)})
        for item in run['paired']:
            if item['baseline']=='blind-hybrid':
                assert item['lost_rows']==0 and item['lost_target']==0
                gains+=item['gained_target']
        errors+=len(run['incremental_errors'])
    assert cases==freeze['confirmation_cases']==192
    assert not exact_losses and not errors
    file_tasks=0
    for name,report in files.items():
        metadata=json.loads((ROOT/'runs'/name/'run.json').read_text())
        assert metadata['binary_sha256']==freeze['binary_sha256']
        assert len(metadata['tasks'])==metadata['expected_tasks']
        assert all(task['returncode']==0 for task in metadata['tasks'])
        file_tasks+=len(metadata['tasks'])
        assert report['statistics']['paired']['lost']==0
        assert not report['exact_stored_baseline_losses']
        for task in metadata['tasks']:
            assert sha(pathlib.Path(task['command'][2]))==task['input_sha256']
    assert file_tasks==freeze['file_tasks']==36
    log=(ROOT/'specialist-tests-release-v1.log').read_text()
    assert 'test result: FAILED' not in log
    results=[(int(a),int(b),int(c)) for a,b,c in re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored',log)]
    assert results and sum(x[1] for x in results)==0
    assert (ROOT/'checks-release-v1/COMPLETE').exists(),'Need successful complete release runner'
    result={'frozen_source_files':len(freeze['source_sha256']),'protected_native_files':len(environment['native_sha256']),'source_binary_database_hashes_match':True,'confirmation_cases':cases,'saved_float32_inputs':waveforms,'additional_correct_targets_vs_hybrid':gains,'incremental_wrong_outputs':errors,'exact_stored_synthetic_baseline_losses':exact_losses,'file_tasks':file_tasks,'historical_exact_stored_rows_retained':files['hybrid-own-confirm-v1-historical']['statistics']['paired']['retained'],'historical_added_reference_matches':files['hybrid-own-confirm-v1-historical']['statistics']['paired']['annotated_additions'],'historical_unannotated_additions':files['hybrid-own-confirm-v1-historical']['statistics']['paired']['added']-files['hybrid-own-confirm-v1-historical']['statistics']['paired']['annotated_additions'],'core_release_passed':sum(x[0] for x in results),'release_log':str(ROOT/'specialist-tests-release-v1.log'),'limitations':'Finite synthetic cases; repeated seeds across SNR/scenarios are correlated. Unannotated real additions are not verified gain or a zero-false guarantee. No GPU/i5 benchmark. No standalone specialist profile.'}
    destination.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result,indent=2))


if __name__=='__main__':main()
