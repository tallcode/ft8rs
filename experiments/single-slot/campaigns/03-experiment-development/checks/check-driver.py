from pathlib import Path
import os,subprocess,json,time
root=Path('/Volumes/ZHITAI/Workspace/ft8rs')
out=root/'experiments/single-slot/campaigns/03-experiment-development/checks'
commands=[
('fmt',['cargo','fmt','--all','--check']),
('clippy',['cargo','clippy','--workspace','--all-targets','--','-D','warnings']),
('core-fast',['bash','scripts/test-decoders.sh','fast']),
]
results=[]
for name,cmd in commands:
    start=time.monotonic()
    with (out/(name+'.log')).open('w') as log:
        result=subprocess.run(cmd,cwd=root,env=dict(os.environ,RUSTC_WRAPPER=''),stdout=log,stderr=subprocess.STDOUT)
    results.append(dict(name=name,command=cmd,returncode=result.returncode,wall_seconds=time.monotonic()-start))
    (out/'local-checks.json').write_text(json.dumps(results,indent=2)+'\n')
    print(name,result.returncode,flush=True)
    if result.returncode:raise SystemExit(result.returncode)
