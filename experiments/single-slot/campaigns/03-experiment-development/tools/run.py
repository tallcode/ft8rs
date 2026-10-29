#!/usr/bin/env python3
"""Serial immutable synthetic or file measurements; raw callbacks always saved."""
import argparse
import collections
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

HERE=Path(__file__).resolve().parent
CAMPAIGN=HERE.parent
REPO=next(p for p in HERE.parents if (p/'crates/ft8rs-core').is_dir())


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def parse(stdout):
    result={'truth':[], 'methods':{}}
    for line in stdout.splitlines():
        f=line.split('\t')
        if f[0]=='TRUTH':
            result['truth'].append(dict(msg=f[1],freq=float(f[2]),dt=float(f[3]),target=f[4]=='true'))
        elif f[0]=='ROW':
            method=result['methods'].setdefault(f[1],dict(rows=[]))
            method['rows'].append(dict(msg=f[2],freq=f[3],dt=f[4],snr=f[5],sync=f[6],tones=f[7],provenance=f[8],callback_ms=float(f[9])))
        elif f[0]=='END':
            method=result['methods'].setdefault(f[1],dict(rows=[]))
            method.update(correct=int(f[2]),wrong=int(f[3]),elapsed_ms=float(f[4]),row_count=int(f[5]))
            assert len(method['rows'])==method['row_count']
    assert result['methods']
    return result


def identity(row):
    return tuple((k,v) for k,v in row.items() if k!='callback_ms')


def run_case(binary,out,args,synthetic):
    out.mkdir(parents=True,exist_ok=False)
    env=dict(os.environ)
    if synthetic:
        (out/'waveforms').mkdir()
        env['FT8RS_MEASURE_WAV']=str(out/'waveforms/input.wav')
    before=time.monotonic()
    with (out/'stdout.tsv').open('w') as stdout, (out/'stderr.txt').open('w') as stderr:
        command=[str(binary),*map(str,args)]
        process=subprocess.run(command,cwd=binary.parent,env=env,stdout=stdout,stderr=stderr)
    metadata=dict(command=command,returncode=process.returncode,process_ms=(time.monotonic()-before)*1000.0,
                  stdout_sha256=sha(out/'stdout.tsv'),stderr_sha256=sha(out/'stderr.txt'))
    (out/'process.json').write_text(json.dumps(metadata,indent=2)+'\n')
    assert process.returncode==0,(out/'stderr.txt').read_text()
    result=parse((out/'stdout.tsv').read_text())
    if synthetic: result['audio_sha256']=sha(out/'waveforms/input.wav')
    else:
        # Native real-recording outputs are candidates until matched to annotations.
        for method in result['methods'].values():
            method['correct']=None
            method['wrong']=None
    methods=result['methods']
    if 'hybrid' in methods and 'exp' in methods:
        baseline=collections.Counter(map(identity,methods['hybrid']['rows']))
        current=collections.Counter(map(identity,methods['exp']['rows']))
        result['lost_baseline']=[dict(row) for row in (baseline-current).elements()]
        result['added']=[row for row in methods['exp']['rows'] if identity(row) not in baseline]
        assert not result['lost_baseline'],out
        assert all(row['provenance']=='Experiment' for row in result['added']),out
        result['first_new_callback_ms']=min((row['callback_ms'] for row in result['added']),default=None)
    (out/'result.json').write_text(json.dumps(result,indent=2)+'\n')
    return result


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('snapshot')
    parser.add_argument('name')
    parser.add_argument('--stage',choices=['development','confirmation','recordings'],default='development')
    args=parser.parse_args()
    assert Path(args.snapshot).name==args.snapshot and Path(args.name).name==args.name
    snapshot=CAMPAIGN/'frozen'/args.snapshot
    binary=snapshot/'target/release/ft8rs-experiment-measure'
    freeze=json.loads((snapshot/'freeze.json').read_text())
    assert freeze['build_complete'] and sha(binary)==freeze['binary_sha256']
    assert sha(binary.parent/'ALLCALL7.TXT')==freeze['database_sha256']
    run=CAMPAIGN/'runs'/args.name
    run.mkdir(parents=True,exist_ok=False)
    plan=[]
    if args.stage=='development':
        root=2026102911001
        plan=[(s,n,snr) for s,n,snr in [('awgn',4,-22),('cw',4,-22),('mix',4,-20),('mix2',4,-20),
                                      ('noise',4,-24),('cwonly',4,-24),('blocker',4,-24),('pilotonly',4,-24)]]
    elif args.stage=='confirmation':
        root=2026102951001
        plan=[('awgn',8,snr) for snr in [-21,-22,-23]]+ [('cw',8,snr) for snr in [-20,-22]] + [
              ('mix',8,-20),('mix2',8,-20),('mixfade',4,-20),('drift',4,-20),
              ('noise',12,-24),('cwonly',12,-24),('blocker',12,-24),('mix2only',8,-24),('pilotonly',8,-24)]
    jobs=[]
    if args.stage=='recordings':
        for name,count in [('210703_133430',1),('230208_140300',19)]:
            path=REPO/'crates/ft8rs-core/tests/ft8'/f'{name}.wav'
            for slot in range(count):
                jobs.append((f'{name}-{slot:02}', ['file',str(path),slot,'hybrid,exp'],dict(file=str(path.relative_to(REPO)),slot=slot,input_sha256=sha(path))))
    else:
        for group,(scenario,count,snr) in enumerate(plan):
            # Different scenarios and negative controls use disjoint roots;
            # SNR levels also use fresh noise, payloads and coordinates.
            for trial in range(count):
                seed=root+group*1000003+trial*10007
                jobs.append((f'{group:02}-{scenario}-{snr}-{trial:02}',
                             [scenario,seed,trial,snr,'wsjtx,jtdx,hybrid,exp'],
                             dict(scenario=scenario,seed=seed,trial=trial,snr=snr)))
    manifest=dict(snapshot=args.snapshot,freeze_sha256=sha(snapshot/'freeze.json'),stage=args.stage,
                  methods='product stream defaults; cooperative_realtime=false; exp only for Hybrid',
                  jobs=[dict(name=name,arguments=argv,**meta) for name,argv,meta in jobs],complete=False)
    (run/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    results=[]
    with Path('/tmp/ft8rs-experiment-measure.lock').open('w') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX)
        for index,(name,argv,meta) in enumerate(jobs):
            result=run_case(binary,run/name,argv,args.stage!='recordings')
            results.append(dict(name=name,**meta,**result))
            methods=result['methods']
            details=', '.join('{} {}/{} {:.0f}ms'.format(k,v['correct'],v['wrong'],v['elapsed_ms']) for k,v in methods.items())
            print(f'{index+1}/{len(jobs)} {name}: '+details,flush=True)
    manifest['complete']=True
    (run/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    (run/'results.json').write_text(json.dumps(results,indent=2)+'\n')
    summary={}
    for result in results:
        for method,item in result['methods'].items():
            key=f"{result.get('scenario','recordings')}:{result.get('snr','')}:{method}"
            entry=summary.setdefault(key,dict(cases=0,correct=0,wrong=0,max_ms=0,max_callback_ms=0))
            entry['cases']+=1
            if args.stage=='recordings':
                entry['correct']=None
                entry['wrong']=None
            else:
                entry['correct']+=item['correct'] or 0
                entry['wrong']+=item['wrong'] or 0
            entry['max_ms']=max(entry['max_ms'],item['elapsed_ms'])
            entry['max_callback_ms']=max(entry['max_callback_ms'],max((r['callback_ms'] for r in item['rows']),default=0))
    (run/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    print(run/'summary.json')


if __name__=='__main__':
    main()
