#!/usr/bin/env python3
"""Diagnostic paired runs with external wait-channel/cgroup sampling; not headline timings."""
import argparse
import collections
import hashlib
import json
import os
from pathlib import Path
import subprocess
import threading
import time
from controller import run, TEST

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--before',type=Path,required=True);p.add_argument('--after',type=Path,required=True)
p.add_argument('--workspace-output',type=Path,required=True);p.add_argument('--tmpfs-output',type=Path,required=True)
p.add_argument('--warm-executables',action='store_true');p.add_argument('--only-workspace',action='store_true')
a=p.parse_args()
outputs={'workspace':a.workspace_output.resolve(),'tmpfs':a.tmpfs_output.resolve()}
if a.only_workspace:outputs.pop('tmpfs')
for root in outputs.values():root.mkdir(parents=True,exist_ok=False)
executables={n:str(getattr(a,n).resolve()) for n in ('before','after')}
def read(path):
    try:return Path(path).read_text()
    except OSError:return None

def system():
    vm=read('/proc/vmstat')
    return {'cpu_max':read('/sys/fs/cgroup/cpu.max'),'cpu_stat':read('/sys/fs/cgroup/cpu.stat'),
            'vmstat':{k:int(v) for k,v in (line.split() for line in vm.splitlines()) if k in ('nr_dirty','nr_writeback','nr_dirtied','nr_written','pgpgout')} if vm else None}

manifest={'scope':'Outside formal timings. Same immutable executables/adapters/provider; only output/store filesystem differs. Evidence also resides on that filesystem, so this does not isolate DB from background evidence writeback. External sampler can perturb scheduling. Wait-channel observations are samples, not time percentages.','period_seconds':0.01,'warm_executables':a.warm_executables,'executables':{n:{'path':path,'sha256':hashlib.file_digest(Path(path).open('rb'),'sha256').hexdigest()} for n,path in executables.items()},'system':system()}
for root in outputs.values():(root/'manifest.json').write_text(json.dumps(manifest,indent=2))
original_popen=subprocess.Popen
for pair in range(3):
    for filesystem in (('workspace',) if a.only_workspace else (('workspace','tmpfs') if pair%2==0 else ('tmpfs','workspace'))):
        for name in (('before','after') if pair%2==0 else ('after','before')):
            stem=outputs[filesystem]/f'{pair}-short-small-{name}'
            warm_start=time.monotonic_ns()
            if a.warm_executables:
                for path in executables.values():
                    with open(path,'rb') as executable:
                        while executable.read(1024*1024):pass
            diagnostic={'before':system(),'samples':[],'executable_warm_ns':time.monotonic_ns()-warm_start if a.warm_executables else None};samplers=[]
            def observe(product):
                began=time.monotonic_ns()
                while product.poll() is None:
                    tasks={}
                    try:
                        for task in Path(f'/proc/{product.pid}/task').iterdir():
                            tasks[task.name]={'wchan':read(task/'wchan'),'schedstat':read(task/'schedstat')}
                    except OSError:pass
                    point={'elapsed_ns':time.monotonic_ns()-began,'tasks':tasks,'system':system()}
                    if a.warm_executables and len(diagnostic['samples'])%10==0:
                        raw=read(f'/proc/{product.pid}/smaps')
                        maps={};current=None
                        for line in (raw or '').splitlines():
                            parts=line.split(None,5)
                            if len(parts)>=5 and '-' in parts[0] and len(parts[1])==4:
                                current=parts[5] if len(parts)==6 else '[anonymous]'
                                maps.setdefault(current,{'Rss':0,'Pss':0,'Anonymous':0})
                            elif current and ':' in line:
                                key,value=line.split(':',1)
                                if key in maps[current]:maps[current][key]+=int(value.split()[0])
                        point['mappings']=maps
                    diagnostic['samples'].append(point)
                    time.sleep(0.01)
            def start(argv,*args,**kwargs):
                product=original_popen(argv,*args,**kwargs)
                if argv[0]==executables[name]:
                    thread=threading.Thread(target=observe,args=(product,),daemon=True);samplers.append(thread);thread.start()
                return product
            subprocess.Popen=start
            env={k:v for k,v in os.environ.items() if not any(s in k.upper() for s in ('TOKEN','SECRET','CREDENTIAL','API_KEY'))}
            env.update(RUSTX_BENCH_CASE='short-small',RUSTX_BENCH_OUTPUT=str(stem.with_suffix('.json')))
            record=run([executables[name],TEST,'--exact','--ignored','--nocapture'],stem,'short-small',env,Path('test-support/fake-provider').resolve())
            for thread in samplers:thread.join()
            subprocess.Popen=original_popen
            diagnostic['after']=system()
            diagnostic['wchan_samples']=dict(collections.Counter(t['wchan'] for s in diagnostic['samples'] for t in s['tasks'].values()))
            stem.with_suffix('.diagnostic.json').write_text(json.dumps(diagnostic,indent=2))
            print(filesystem,stem.name,record['accepted'],diagnostic['wchan_samples'],flush=True)
            assert record['accepted']
        comparison=subprocess.run(['node','benchmarks/runtime/compare-http.mjs',str(outputs[filesystem]/f'{pair}-short-small-before.json'),str(outputs[filesystem]/f'{pair}-short-small-after.json'),str(outputs[filesystem]/f'{pair}-short-small-comparison.json')],capture_output=True,text=True)
        assert comparison.returncode==0,comparison.stderr
