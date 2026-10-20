#!/usr/bin/env python3
"""Separate diagnostic CPU/heap runs. Product-only preload; sibling provider unchanged."""
import argparse, hashlib, json, os, sys
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from controller import run, TEST, OBSERVER_MODES
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--binary',type=Path,required=True);p.add_argument('--profiler-root',type=Path,required=True)
p.add_argument('--output',type=Path,required=True);p.add_argument('--mode',choices=['cpu','heap','control'],required=True)
p.add_argument('--case',choices=['short-small','short-large'],required=True)
p.add_argument('--observer-mode',choices=OBSERVER_MODES,default='native-workflow')
a=p.parse_args();root=a.output.resolve();root.mkdir(parents=True,exist_ok=False)
env={k:v for k,v in os.environ.items() if not any(s in k.upper() for s in ('TOKEN','SECRET','CREDENTIAL','API_KEY'))}
env.update(UV_CACHE_DIR='/workspace/.uv-cache',RUSTX_BENCH_CASE=a.case,RUSTX_BENCH_OUTPUT=str(root/'sample.json'),RUSTX_BENCH_OBSERVER_MODE=a.observer_mode)
args=[]
if a.mode!='control':
 lib=a.profiler_root/'unpacked/usr/lib/x86_64-linux-gnu'/('libprofiler.so.0' if a.mode=='cpu' else 'libtcmalloc.so.4')
 bridge=a.profiler_root/'bridge.so'
 args=['env',f'LD_PRELOAD={bridge}:{lib}',f'RUSTX_BENCH_PROFILE_SOCKET={root}/control.sock',f'RUSTX_BENCH_PROFILE_DIR={root}']
 if a.mode=='cpu':args+=['CPUPROFILE_FREQUENCY=250']
 else:args += [f'HEAPPROFILE={root}/alloc','HEAP_PROFILE_ALLOCATION_INTERVAL=0','HEAP_PROFILE_INUSE_INTERVAL=0','HEAP_PROFILE_TIME_INTERVAL=0']
args += [str(a.binary.resolve()),TEST,'--exact','--ignored','--nocapture']
manifest={'mode':a.mode,'case':a.case,'observer_mode':a.observer_mode,'binary_sha256':hashlib.file_digest(a.binary.open('rb'),'sha256').hexdigest(),'argv':args,'scope':'Diagnostics only. CPU signal samples include all product threads and driver; heap mode replaces system allocator. Phase boundary flush/dump/handshake overhead is present. Provider unchanged and controller-owned sibling. No device-write or exclusive elapsed-percentage claims.'}
(root/'manifest.json').write_text(json.dumps(manifest,indent=2))
r=run(args,root/'sample',a.case,env,Path('test-support/fake-provider').resolve())
print(json.dumps({'accepted':r['accepted'],'error':r.get('error')}),flush=True)
assert r['accepted']
