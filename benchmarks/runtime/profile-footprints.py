#!/usr/bin/env python3
"""Separate external-controller sampling of product-owned Source temp files."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import threading
import time
from controller import run, TEST

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--rust',type=Path,required=True)
p.add_argument('--output',type=Path,required=True)
a=p.parse_args()
a.output.mkdir(parents=True,exist_ok=False)
stem=(a.output/'short-small').resolve()
original_popen=subprocess.Popen
samplers=[]
result={'period_seconds':0.01,'samples':0,'paths':{},'maximum_sampled_logical':0,'maximum_sampled_allocated':0,
        'scope':'Only product-owned /tmp Source bytes/index.sqlite discovered through its file descriptors. Independent controller thread. Sampling can miss short-lived files; maxima are observed, not true peaks. Outside formal timing.'}

def observe(product):
    active=set()
    while product.poll() is None:
        try:
            for fd in Path(f'/proc/{product.pid}/fd').iterdir():
                try:
                    path=Path(os.readlink(fd))
                    if path.parent.parent==Path('/tmp') and path.name in ('bytes','index.sqlite'):
                        active.add(path)
                except (OSError,ValueError):
                    pass
        except OSError:
            pass
        logical=allocated=0
        for path in tuple(active):
            try:
                stat=path.stat()
            except FileNotFoundError:
                active.remove(path);continue
            entry=result['paths'].setdefault(str(path),{'kind':path.name,'maximum_sampled_logical':0,'maximum_sampled_allocated':0})
            entry['maximum_sampled_logical']=max(entry['maximum_sampled_logical'],stat.st_size)
            entry['maximum_sampled_allocated']=max(entry['maximum_sampled_allocated'],stat.st_blocks*512)
            logical+=stat.st_size;allocated+=stat.st_blocks*512
        result['samples']+=1
        result['maximum_sampled_logical']=max(result['maximum_sampled_logical'],logical)
        result['maximum_sampled_allocated']=max(result['maximum_sampled_allocated'],allocated)
        time.sleep(result['period_seconds'])

def start(argv,*args,**kwargs):
    process=original_popen(argv,*args,**kwargs)
    if argv[0]==str(a.rust.resolve()):
        thread=threading.Thread(target=observe,args=(process,),daemon=True)
        samplers.append(thread);thread.start()
    return process

subprocess.Popen=start
env={k:v for k,v in os.environ.items() if not any(s in k.upper() for s in ('TOKEN','SECRET','CREDENTIAL','API_KEY'))}
env.update(RUSTX_BENCH_CASE='short-small',RUSTX_BENCH_OUTPUT=str(stem.with_suffix('.json')))
record=run([str(a.rust.resolve()),TEST,'--exact','--ignored','--nocapture'],stem,'short-small',env,Path('test-support/fake-provider').resolve())
for thread in samplers:
    thread.join()
assert record['accepted']
assert result['samples'] and result['paths']
(a.output/'temp-footprints.json').write_text(json.dumps(result,indent=2))
print(json.dumps({k:v for k,v in result.items() if k!='paths'}))
