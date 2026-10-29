#!/usr/bin/env python3
"""Preserve completed experimental batches between timed jobs; never decode/delete."""
import datetime
import hashlib
import json
import os
import pathlib
import shutil

HERE=pathlib.Path(__file__).resolve().parent
ROOT=pathlib.Path('/tmp/ft8rs-single-slot-lab')
CAMPAIGNS=HERE/'campaigns'
FIRST=CAMPAIGNS/'01-single-slot-research'
SECOND=CAMPAIGNS/'02-hybrid-specialist'


def copy_new(source,destination):
    if destination.exists(): return
    destination.parent.mkdir(parents=True,exist_ok=True)
    shutil.copy2(source,destination)


def main():
    FIRST.mkdir(parents=True,exist_ok=True)
    archive=HERE/'results/2026-10-29-single-slot-preserved.tar.gz'
    destination=FIRST/archive.name
    if not destination.exists(): os.link(archive,destination)
    for name in ['preservation-manifest-2026-10-29.json','checkpoint-2026-10-29.json','checkpoint-2026-10-29.md','2026-10-29-single-slot-preserved.tar.gz.sha256']:
        copy_new(HERE/'results'/name,FIRST/name)
    SECOND.mkdir(parents=True,exist_ok=True)
    frozen=ROOT/'frozen-hybrid-specialist-v1'
    if frozen.exists() and not (SECOND/'frozen').exists(): shutil.copytree(frozen,SECOND/'frozen')
    for path in sorted(HERE.glob('*.py')):
        copy_new(path,SECOND/'scripts'/path.name)
        digest=hashlib.sha256(path.read_bytes()).hexdigest()[:12]
        copy_new(path,SECOND/'tool-history'/(path.stem+'-'+digest+path.suffix))
    for path in sorted(HERE.glob('receiver*.rs')): copy_new(path,SECOND/'research-sources'/path.name)
    for name in ['freeze-hybrid-specialist-v1.json','hybrid-development-r1.json']:
        path=HERE/'results'/name
        if path.exists(): copy_new(path,SECOND/name)
    states=[]
    for folder in sorted((ROOT/'runs').glob('hybrid-own-*')):
        metadata_path=folder/'run.json'
        if not metadata_path.exists():
            states.append({'name':folder.name,'complete':False});continue
        metadata=json.loads(metadata_path.read_text())
        if 'tasks' in metadata:
            complete=len(metadata['tasks'])==metadata['expected_tasks'] and all(t['returncode']==0 for t in metadata['tasks'])
        else: complete=metadata.get('returncode')==0
        target=SECOND/'runs'/folder.name
        if complete and not target.exists():
            target.parent.mkdir(exist_ok=True)
            shutil.copytree(folder,target)
        states.append({'name':folder.name,'complete':complete,'preserved':target.exists()})
    # Logs are progress snapshots; immutable raw completed runs stay untouched.
    for path in sorted(ROOT.glob('*specialist*.log'))+sorted(ROOT.glob('hybrid-own-*.log')):
        target=SECOND/'logs'/path.name
        target.parent.mkdir(exist_ok=True)
        shutil.copy2(path,target)
    for name in ['specialist-workspace-fast.log','specialist-udp-retry.log']:
        path=ROOT/name
        if path.exists(): shutil.copy2(path,SECOND/'logs'/name)
    state={'recorded_at':datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),'freeze_sha256':hashlib.sha256((SECOND/'freeze-hybrid-specialist-v1.json').read_bytes()).hexdigest() if (SECOND/'freeze-hybrid-specialist-v1.json').exists() else None,'runs':states,'originals_deleted':False,'first_campaign_archive':str(FIRST/archive.name)}
    (SECOND/'status.json').write_text(json.dumps(state,indent=2)+'\n')
    print('Preserved completed second-campaign batches:',sum(r.get('preserved',False) for r in states),flush=True)


if __name__=='__main__':main()
