#!/usr/bin/env python3
"""Index preserved Hybrid specialist runs; never invokes a decoder or overwrites runs."""
import argparse
import collections
import csv
import json
import pathlib
import re
import statistics

ROOT = pathlib.Path('/tmp/ft8rs-single-slot-lab/runs')


def identity(a, b):
    return a['message'] == b['message'] and abs(a['f']-b['f']) <= 5 and abs(a['dt']-b['dt']) <= .3


def signals(text):
    result=[]
    for message, f, dt in re.findall(r'(?:^|;)([^;]+)@([-\d.]+)/([-\d.]+)', text):
        result.append({'message':message,'f':float(f),'dt':float(dt)})
    return result


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('output',type=pathlib.Path)
    parser.add_argument('names',nargs='+')
    args=parser.parse_args()
    if args.output.exists(): raise SystemExit('Choose a fresh analysis filename')
    report={'scope':'Finite known-truth synthetic paired comparison; fresh sessions; no external training data. Raw input/row evidence stays in each immutable run directory.','runs':[]}
    for name in args.names:
        folder=ROOT/name
        metadata=json.loads((folder/'run.json').read_text())
        if metadata.get('returncode') != 0: raise SystemExit('Run is not complete: '+name)
        with (folder/'rows.tsv').open() as stream: rows=list(csv.DictReader(stream,delimiter='\t'))
        truth={}; callbacks=collections.defaultdict(list); current=None
        for line in (folder/'stderr.txt').read_text().splitlines():
            fields=line.split('\t')
            if fields[0]=='TRUTH':
                current=(fields[1],fields[2]); truth[current]={'message':fields[3],'f':float(fields[4]),'dt':float(fields[5])}
            elif fields[0]=='CALLBACK' and current is not None:
                callbacks[(*current,'blind-'+fields[1])].append({'ms':float(fields[2]),'message':fields[3],'f':float(fields[4]),'dt':float(fields[5])})
        cases=collections.defaultdict(dict); groups=collections.defaultdict(list)
        for row in rows:
            key=(row['seed'],row['snr'])
            row={**row,'correct':int(row['correct']),'wrong':int(row['wrong']),'ms':float(row['ms']),'signals':signals(row['message'])}
            cases[key][row['method']]=row
            groups[(row['method'],row['snr'])].append(row)
        summary=[]
        for (method,snr),items in sorted(groups.items()):
            times=sorted(row['ms'] for row in items)
            target_callbacks=[]
            for row in items:
                key=(row['seed'],row['snr'])
                matches=[cb['ms'] for cb in callbacks[(*key,method)] if identity(cb,truth[key])]
                if row['correct'] and matches: target_callbacks.append(min(matches))
            summary.append({'method':method,'snr':snr,'frames':len(items),'correct':sum(x['correct'] for x in items),'wrong':sum(x['wrong'] for x in items),'mean_ms':statistics.mean(times),'p95_ms':times[min(len(times)-1,int(.95*(len(times)-1)))],'max_ms':max(times),'target_callback_max_ms':max(target_callbacks) if target_callbacks else None})
        paired=[]; mismatches=[]
        for baseline in ['native','blind-jtdx','blind-hybrid']:
            for snr in sorted({key[1] for key in cases},key=float,reverse=True):
                counts=collections.Counter()
                for key,methods in cases.items():
                    if key[1] != snr: continue
                    base=methods[baseline]; own=methods['blind-hybrid-specialist']
                    counts['frames']+=1
                    counts['retained_target']+=bool(base['correct'] and own['correct'])
                    counts['gained_target']+=bool(not base['correct'] and own['correct'])
                    counts['lost_target']+=bool(base['correct'] and not own['correct'])
                    lost=[x for x in base['signals'] if not any(identity(x,y) for y in own['signals'])]
                    added=[x for x in own['signals'] if not any(identity(x,y) for y in base['signals'])]
                    counts['lost_rows']+=len(lost); counts['added_rows']+=len(added)
                    if lost: mismatches.append({'seed':key[0],'snr':snr,'baseline':baseline,'lost':lost})
                paired.append({'baseline':baseline,'snr':snr,**counts})
        errors=[]; incremental_errors=[]
        for key,methods in cases.items():
            for method,row in methods.items():
                if row['wrong']: errors.append({'seed':key[0],'snr':key[1],'method':method,'wrong':row['wrong'],'signals':row['signals']})
            enhanced=methods['blind-hybrid-specialist']
            base=methods['blind-hybrid']
            scenario=enhanced['scenario']
            expected=[] if scenario in ['noise','cwonly','blocker','mix2only'] else [truth[key]]
            if scenario in ['mix','blocker','mixfade','mix2','mix2only']:
                expected.append({'message':'CQ K1ABC FN42','f':truth[key]['f']+24.63,'dt':truth[key]['dt']-.093})
            if scenario in ['mix2','mix2only']:
                expected.append({'message':'CQ W9XYZ EN50','f':truth[key]['f']-31.17,'dt':truth[key]['dt']+.061})
            wrong=[signal for signal in enhanced['signals'] if not any(identity(signal,target) for target in expected)]
            for signal in wrong:
                if not any(identity(signal,old) for old in base['signals']):
                    incremental_errors.append({'seed':key[0],'snr':key[1],**signal})
        report['runs'].append({'name':name,'metadata':metadata,'distinct_cases':len(cases),'summary':summary,'paired':paired,'errors':errors,'incremental_errors':incremental_errors,'losses':mismatches})
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(report,indent=2)+'\n')
    print(args.output)
    for run in report['runs']:
        print(run['name'])
        for row in run['summary']: print('\t'.join(str(row[k]) for k in ['method','snr','correct','wrong','frames','max_ms']))
        print('paired_hybrid',json.dumps([r for r in run['paired'] if r['baseline']=='blind-hybrid']))


if __name__=='__main__': main()
