#!/usr/bin/env python3
"""Read-only audit of frozen sources, raw callbacks and retained local waveforms."""
import collections
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess

HERE=Path(__file__).resolve().parent
CAMPAIGN=HERE.parent
REPO=next(p for p in HERE.parents if (p/'crates/ft8rs-core').is_dir())


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    spec=importlib.util.spec_from_file_location('measurement_runner',HERE/'run.py')
    runner=importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runner)
    results=dict(frozen_builds=[],runs=[],runtime_source_changes=[],test_or_document_changes=[],
                 generated_waveforms=0,baseline_rows=0,baseline_losses=0)
    snapshots={}
    for record in sorted((CAMPAIGN/'frozen').glob('*/freeze.json')):
        freeze=json.loads(record.read_text())
        folder=record.parent
        for relative,digest in freeze['source_sha256'].items():
            if relative.startswith('crates/ft8rs-core/src/'):
                file=folder/'core/src'/relative.removeprefix('crates/ft8rs-core/src/')
                data=file.read_bytes()
                if relative.endswith('/src/lib.rs'):data=data.removesuffix(b'\npub mod lab_bridge;\n')
            else:
                data=(folder/'product-source'/relative).read_bytes()
            assert hashlib.sha256(data).hexdigest()==digest,(folder,relative)
        if freeze['build_complete']:
            binary=folder/'target/release/ft8rs-experiment-measure'
            assert sha(binary)==freeze['binary_sha256']
            assert sha(binary.parent/'ALLCALL7.TXT')==freeze['database_sha256']
            assert sha(folder/'lab/src/main.rs')==freeze['generator_sha256']
            for relative,digest in freeze['source_sha256'].items():
                if sha(REPO/relative)==digest:continue
                if not relative.endswith('.rs') or relative.endswith('tests.rs') or '/tests/' in relative:
                    results['test_or_document_changes'].append(relative)
                else:results['runtime_source_changes'].append(relative)
        results['frozen_builds'].append(dict(name=folder.name,complete=freeze['build_complete'],freeze_sha256=sha(record)))
        snapshots[folder.name]=(freeze,record)
    assert not results['runtime_source_changes'],results['runtime_source_changes']
    for folder in sorted((CAMPAIGN/'runs').iterdir()):
        manifest=json.loads((folder/'manifest.json').read_text())
        assert manifest['complete'],folder
        freeze,record=snapshots[manifest['snapshot']]
        assert sha(record)==manifest['freeze_sha256']
        baseline_rows=0
        for job in manifest['jobs']:
            case=folder/job['name']
            process=json.loads((case/'process.json').read_text())
            assert process['returncode']==0
            assert sha(case/'stdout.tsv')==process['stdout_sha256']
            assert sha(case/'stderr.txt')==process['stderr_sha256']
            parsed=runner.parse((case/'stdout.tsv').read_text())
            stored=json.loads((case/'result.json').read_text())
            for method,value in parsed['methods'].items():
                assert value['rows']==stored['methods'][method]['rows']
                assert value['elapsed_ms']==stored['methods'][method]['elapsed_ms']
            assert parsed['truth']==stored['truth']
            old=collections.Counter(map(runner.identity,parsed['methods']['hybrid']['rows']))
            new=collections.Counter(map(runner.identity,parsed['methods']['exp']['rows']))
            assert not old-new,case
            baseline_rows+=sum(old.values())
            if manifest['stage']!='recordings':
                assert sha(case/'waveforms/input.wav')==stored['audio_sha256']
                results['generated_waveforms']+=1
            else:
                assert sha(REPO/job['file'])==job['input_sha256']
                assert all(m['wrong'] is None and m['correct'] is None for m in stored['methods'].values())
        results['baseline_rows']+=baseline_rows
        results['runs'].append(dict(name=folder.name,cases=len(manifest['jobs']),baseline_rows=baseline_rows,stage=manifest['stage']))
    protected=['crates/ft8rs-core/src/decode/lib_wsjtx','crates/ft8rs-core/src/decode/lib_jtdx',
               'experiments/single-slot/campaigns/01-single-slot-research',
               'experiments/single-slot/campaigns/02-hybrid-specialist']
    diff=subprocess.check_output(['git','diff','--name-only','e692a16','--',*protected],cwd=REPO,text=True)
    assert not diff,diff
    results['prior_campaign_and_native_changes']=[]
    report=CAMPAIGN/'analysis/completion-audit.json'
    with report.open('x') as f:f.write(json.dumps(results,indent=2)+'\n')
    print(json.dumps(results,indent=2))


if __name__=='__main__':
    main()
