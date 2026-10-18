#!/usr/bin/env python3
"""Paired, alternating production revisions over the same local provider scripts."""
import argparse
import json
import hashlib
import os
from pathlib import Path
import subprocess
from controller import CASES, TEST, run

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--before',type=Path,required=True);p.add_argument('--after',type=Path,required=True)
p.add_argument('--output',type=Path,required=True);p.add_argument('--pairs',type=int,default=3)
p.add_argument('--case',choices=CASES,action='append');p.add_argument('--pair-offset',type=int,default=0);args=p.parse_args()
args.output.mkdir(parents=True,exist_ok=False)
env={k:v for k,v in os.environ.items() if not any(s in k.upper() for s in ('TOKEN','SECRET','CREDENTIAL','API_KEY'))}
env['UV_CACHE_DIR']='/workspace/.uv-cache'
manifest={'clock_ticks':os.sysconf('SC_CLK_TCK'),'pairs':args.pairs,'pair_offset':args.pair_offset,'cases':args.case or CASES,'executables':{name:{'path':str(getattr(args,name).resolve()),'sha256':hashlib.file_digest(getattr(args,name).open('rb'),'sha256').hexdigest()} for name in ('before','after')},'head':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'scope':'runtime plus driver; sibling provider; evidence output after cleanup; see baseline identity for fixed build and hardware'}
(args.output/'manifest.json').write_text(json.dumps(manifest,indent=2))
failed=False
for pair in range(args.pair_offset,args.pair_offset+args.pairs):
 for case in args.case or CASES:
  for name in (['before','after'] if pair%2==0 else ['after','before']):
   stem=(args.output/f'{pair}-{case}-{name}').resolve()
   executable=getattr(args,name).resolve()
   child_env=env|{'RUSTX_BENCH_CASE':case,'RUSTX_BENCH_OUTPUT':str(stem.with_suffix('.json'))}
   record=run([str(executable),TEST,'--exact','--ignored','--nocapture'],stem,case,child_env,Path('test-support/fake-provider').resolve())
   failed|=not record['accepted'];print(json.dumps({k:v for k,v in record.items() if k not in ('provider_report','provider_stdout')}),flush=True)
  comparison=subprocess.run(['node','benchmarks/runtime/compare-http.mjs',str(args.output/f'{pair}-{case}-before.json'),str(args.output/f'{pair}-{case}-after.json'),str(args.output/f'{pair}-{case}-comparison.json')],capture_output=True,text=True)
  if comparison.returncode:
   failed=True;(args.output/f'{pair}-{case}-comparison-error.log').write_text(comparison.stderr)
raise SystemExit(int(failed))
