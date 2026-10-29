#!/usr/bin/env python3
"""Summarize raw evidence without relabelling unknown recordings as true gains."""
import argparse
import collections
import csv
import datetime
import hashlib
import json
from pathlib import Path

HERE=Path(__file__).resolve().parent
CAMPAIGN=HERE.parent
REPO=next(p for p in HERE.parents if (p/'crates/ft8rs-core').is_dir())


def normal(text):
    return ' '.join(text.replace('<','').replace('>','').upper().split())


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('run')
    parser.add_argument('output')
    args=parser.parse_args()
    run=CAMPAIGN/'runs'/args.run
    manifest=json.loads((run/'manifest.json').read_text())
    assert manifest['complete']
    results=json.loads((run/'results.json').read_text())
    report=dict(stage=manifest['stage'],cases=len(results),baseline_losses=0,methods={})
    for result in results:
        report['baseline_losses']+=len(result.get('lost_baseline',[]))
        for method,item in result['methods'].items():
            group=report['methods'].setdefault(method,dict(correct=0,wrong=0,rows=0,max_ms=0,max_callback_ms=0))
            if manifest['stage']=='recordings':
                group['correct']=None
                group['wrong']=None
            else:
                group['correct']+=item['correct'] or 0
                group['wrong']+=item['wrong'] or 0
            group['rows']+=len(item['rows'])
            group['max_ms']=max(group['max_ms'],item['elapsed_ms'])
            group['max_callback_ms']=max(group['max_callback_ms'],max((r['callback_ms'] for r in item['rows']),default=0))
    if manifest['stage']=='recordings':
        additions=[]
        review_path=CAMPAIGN/'annotations/recordings-user-review.json'
        review=json.loads(review_path.read_text()) if review_path.exists() else {'entries':[]}
        report['supplemental_annotation_source']=dict(
            path=str(review_path.relative_to(CAMPAIGN)),
            sha256=hashlib.sha256(review_path.read_bytes()).hexdigest(),
            source=review.get('source')) if review_path.exists() else None
        for result in results:
            path=REPO/result['file']
            csv_path=path.with_suffix('.csv')
            start=datetime.datetime.strptime(path.stem,'%y%m%d_%H%M%S')
            slot_time=start+datetime.timedelta(seconds=15*result['slot'])
            with csv_path.open() as f:
                annotations=[r for r in csv.DictReader(f) if r['Date-Time']==slot_time.strftime('%y%m%d_%H%M%S')]
            for row in result['added']:
                matches=[r for r in annotations if normal(r['Msg'])==normal(row['msg'])
                         and abs(float(r['Freq'])-float(row['freq']))<=5.0
                         and abs(float(r['Drift'])-float(row['dt']))<=0.3]
                reviewed=[r for r in review['entries']
                          if r['file']==result['file'] and r['slot']==result['slot']
                          and r['input_sha256']==result['input_sha256']
                          and normal(r['message'])==normal(row['msg'])
                          and abs(r['freq_hz']-float(row['freq']))<=5.0
                          and abs(r['dt_seconds']-float(row['dt']))<=0.3
                          and r['status']=='annotated']
                additions.append(dict(file=result['file'],slot=result['slot'],row=row,
                                      annotations=matches,
                                      supplemental_annotations=reviewed,
                                      annotation_origin='reference-csv' if matches else 'user-review' if reviewed else None,
                                      classification='annotated-error' if any(r.get('Extra','').strip()=='E' for r in matches)
                                      else 'annotated' if matches or reviewed else 'unlabelled-candidate'))
        report['additions']=additions
        report['annotation_counts']=dict(collections.Counter(x['classification'] for x in additions))
        report['annotation_origin_counts']=dict(collections.Counter(x['annotation_origin'] for x in additions))
    else:
        added_wrong=[]
        added_correct=[]
        for result in results:
            for row in result['added']:
                matches=[t for t in result['truth'] if row['msg']==t['msg']
                         and abs(float(row['freq'])-t['freq'])<=5.0 and abs(float(row['dt'])-t['dt'])<=0.3]
                if not matches: added_wrong.append(dict(case=result['name'],row=row))
                elif any(t['target'] for t in matches): added_correct.append(dict(case=result['name'],row=row))
        report.update(added_wrong=added_wrong,added_target_rows=added_correct)
    output=CAMPAIGN/'analysis'/args.output
    with output.open('x') as f:
        json.dump(report,f,indent=2)
        f.write('\n')
    print(output)


if __name__=='__main__':
    main()
