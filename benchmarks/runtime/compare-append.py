#!/usr/bin/env python3
"""Fresh-process paired marginal appends; no provider/context construction."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

TEST = 'agent_loop::runtime_benchmark::measure_canonical_append'
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--before', type=Path, required=True)
p.add_argument('--after', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--pairs', type=int, default=3)
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=False)
env = {k:v for k,v in os.environ.items() if not any(s in k.upper() for s in ('TOKEN','SECRET','CREDENTIAL','API_KEY'))}
manifest = {'clock_ticks':os.sysconf('SC_CLK_TCK'), 'pairs':a.pairs,
            'head':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'executables':{n:{'path':str(getattr(a,n).resolve()), 'sha256':hashlib.file_digest(getattr(a,n).open('rb'),'sha256').hexdigest()} for n in ('before','after')}}
(a.output/'manifest.json').write_text(json.dumps(manifest,indent=2))
failed = False
for pair in range(a.pairs):
    for history in (0,64,512,2048):
        for name in (('before','after') if pair%2==0 else ('after','before')):
            stem = (a.output/f'{pair}-{history}-{name}').resolve()
            began = time.monotonic_ns()
            with stem.with_suffix('.log').open('w') as log:
                result = subprocess.run([str(getattr(a,name).resolve()),TEST,'--exact','--ignored','--nocapture'],
                    stdout=log,stderr=subprocess.STDOUT,env=env|{'RUSTX_BENCH_HISTORY':str(history),'RUSTX_BENCH_OUTPUT':str(stem.with_suffix('.json'))})
            record = {'exit':result.returncode, 'process_lifecycle_ns':time.monotonic_ns()-began, 'accepted':False}
            try:
                sample = json.loads(stem.with_suffix('.json').read_text())
                assert result.returncode == 0 and f'test {TEST} ... ok' in stem.with_suffix('.log').read_text()
                assert sample['history']==history and sample['measured']==32 and sample['oracle']=='passed'
                assert len(sample['append_ns'])==32 and all(isinstance(n,int) and n>=0 for n in sample['append_ns'])
                assert [x['phase'] for x in sample['lifecycle']]==['before_initialization','after_seed','after_appends','after_close','after_reopen_verification','after_cleanup']
                record['accepted']=True
            except (AssertionError, ValueError, OSError) as error:
                record['error']=repr(error);failed=True
            stem.with_suffix('.process.json').write_text(json.dumps(record,indent=2))
            print(stem.name,json.dumps(record),flush=True)
raise SystemExit(int(failed))
