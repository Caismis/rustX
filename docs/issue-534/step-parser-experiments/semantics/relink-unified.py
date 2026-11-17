from pathlib import Path
import json,subprocess,os,hashlib
r=Path('/tmp/pr556-step-semantics');e=json.loads((r/'build-evidence.json').read_text());e['scannerFlagsRevision']='Final sources capture actual scanner-owned yyin flags in diagnostic subclass destructor, no control-flow changes';e['finalSourcesSha256']={str(p.relative_to(r)):hashlib.sha256(p.read_bytes()).hexdigest()for p in (r/'src').iterdir()};e['finalObjectsSha256']={p.name:hashlib.sha256(p.read_bytes()).hexdigest()for p in (r/'objects').glob('*.o')}
for v in e['variants']:
 if v['name'] in ['original','tail']:
  subprocess.run(v['link'],cwd='/tmp/pr556-build-O3/build',env={**os.environ,'BINARYEN_CORES':'2','EMCC_CORES':'2'},check=True);print(v['name'],'unified scanner flags linked',flush=True)
(r/'build-evidence.json').write_text(json.dumps(e,indent=2)+'\n')
