#!/usr/bin/env python3
"""Link the identical external inspector host against a verified ordinary library."""
import argparse, hashlib, json, os, subprocess
from pathlib import Path

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--build',type=Path,required=True)
p.add_argument('--output',type=Path,required=True)
a=p.parse_args();a.output.mkdir(parents=True,exist_ok=False)
root=Path(__file__).resolve().parent
identity=json.loads((a.build/'identity.json').read_text())
assert identity['verified'] is True
rows=[json.loads(line) for line in (a.build/'build.stdout').read_text().splitlines() if line.startswith('{')]
required=['rustx','tokio','tokio_util','serde_json','chrono'];extern={}
for name in required:
    matches={str(Path(f).resolve()) for row in rows if row.get('reason')=='compiler-artifact' and row['target']['name']==name for f in row['filenames'] if f.endswith('.rlib')}
    assert len(matches)==1,(name,matches)
    extern[name]=matches.pop()
digest=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
assert digest(extern['rustx'])==identity['library_sha256']
cmd=['rustc','--edition=2024','-C','opt-level=3','-C','debuginfo=0','-L','dependency='+str(Path(extern['rustx']).parent)]
for name,path in extern.items():cmd+=['--extern',name+'='+path]
cmd += [str(root/'host.rs'),'-o',str(a.output/'inspector-host')]
record={'native_library_build':identity,'extern':{k:{'path':v,'sha256':digest(v)} for k,v in extern.items()},'source':{p.name:digest(p) for p in sorted(root.glob('*.rs'))},'command':cmd,'ordinary_optimized':True,'no_product_source_changes':True}
with (a.output/'stdout').open('w') as out,(a.output/'stderr').open('w') as err:
    result=subprocess.run(cmd,stdout=out,stderr=err)
record['exit_code']=result.returncode
if result.returncode==0:record['binary_sha256']=digest(a.output/'inspector-host')
(a.output/'identity.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps({'exit_code':result.returncode,'output':str(a.output)}))
raise SystemExit(result.returncode)
