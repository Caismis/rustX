#!/usr/bin/env python3
"""Safely extract and independently hash-check the F evidence archive.
Runs only archived raw-result auditors, never benchmarks or native products.
"""
import argparse, hashlib, json, pathlib, subprocess, sys, tarfile, time
P=argparse.ArgumentParser(); P.add_argument('--freeze',type=pathlib.Path); A=P.parse_args()
ROOT=pathlib.Path('/home/capi/.cache/rustx-pr485-cost-reduction-archive-check')
EVIDENCE=pathlib.Path('/home/capi/.codex/worktrees/pr485-pi-performance/rustX/docs/pi-durable-cost-model/evidence/write-compute-20261027')
ARCHIVE=EVIDENCE/'raw.tar.xz'; SIDECAR=EVIDENCE/'raw.tar.manifest.json'
F='7ca19d99e1a241e5e33627819fe837a81788d26609072d8c41a1a0ab805ef109'
D='8ec26a4a08bde085f46ee40159d3870f7ead675bec39f42cb4f3e55df1d1d233'
REV='2800be25a650305d5daf9f543ffbf7d8f62ea79b'
def sha(p):
 with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def load(p):return json.loads(p.read_text())
ROOT.mkdir(exist_ok=False); RAW=ROOT/'raw';RAW.mkdir()
manifest=load(SIDECAR); entries=set();payload_bytes=0
with tarfile.open(ARCHIVE,'r:xz') as t:
 for member in t:
  name=pathlib.PurePosixPath(member.name)
  assert member.isfile() and not name.is_absolute() and '..' not in name.parts and str(name)==member.name
  assert member.name not in entries; entries.add(member.name)
  assert member.name=='MANIFEST.json' or member.name in manifest
  if member.name!='MANIFEST.json':assert member.size==manifest[member.name]['bytes']
  target=RAW/member.name;target.parent.mkdir(parents=True,exist_ok=True)
  source=t.extractfile(member);assert source
  h=hashlib.sha256();n=0
  with target.open('xb') as out:
   while block:=source.read(1024*1024):out.write(block);h.update(block);n+=len(block)
  assert n==member.size
  if member.name!='MANIFEST.json':
   assert h.hexdigest()==manifest[member.name]['sha256'];payload_bytes+=n
assert entries==set(manifest)|{'MANIFEST.json'}
assert (RAW/'MANIFEST.json').read_bytes()==SIDECAR.read_bytes()
# Re-read extracted bytes independently of the extraction stream.
for name, item in manifest.items():
 p=RAW/name;assert p.stat().st_size==item['bytes'] and sha(p)==item['sha256']
assert {str(p.relative_to(RAW)) for p in RAW.rglob('*') if p.is_file()}==entries
report={'accepted':False,'archive_sha256':sha(ARCHIVE),'archive_bytes':ARCHIVE.stat().st_size,
 'manifest_sha256':sha(SIDECAR),'payload_files':len(manifest),'payload_bytes':payload_bytes,
 'safe_member_types_and_paths':True,'all_member_hashes_verified_after_extraction':True,
 'internal_external_manifest_bytes_equal':True,'external_binaries_or_checkouts_reverified':False,'steps':[]}
(ROOT/'archive-verification.json').write_text(json.dumps(report,indent=2)+'\n')
steps=[
 ('direct',[sys.executable,str(RAW/'recompute-direct.py'),'--root',str(RAW/'direct-cohorts')],None),
 ('confirmation',[sys.executable,str(RAW/'recompute-confirmation.py'),'--root',str(RAW/'confirmation-cohorts')],None),
 ('pi',[sys.executable,str(RAW/'recompute-pi.py'),'--root',str(RAW/'final-cohorts'),'--raw-only','--identity',str(RAW/'candidate-f-build/identity.json'),'--expect-rust-sha256',F],None),
 ('append',[sys.executable,str(RAW/'audit-final-append.py'),'--root',str(RAW/'final-cohorts'),'--output',str(ROOT/'append-recomputed.json'),'--binary-verification','off','--before-identity',str(RAW/'baseline-d-build-identity.json'),'--after-identity',str(RAW/'candidate-f-build/identity.json'),'--expect-before-sha256',D,'--expect-after-sha256',F],ROOT/'append-recomputed.json')]
if A.freeze:
 assert A.freeze.is_file()
 report['freeze_sidecar']={'path':str(A.freeze),'sha256':sha(A.freeze),'archived_in_raw_tar':False}
 steps.append(('three-engine',[sys.executable,str(RAW/'recompute-three-engine.py'),'--dataset',str(RAW/'final-cohorts/three-engine'),'--output',str(ROOT/'three-engine-recomputed.json'),'--freeze',str(A.freeze),'--expected-rust-sha256',F,'--expected-harness-revision',REV,'--raw-only'],ROOT/'three-engine-recomputed.json'))
for name, cmd, output in steps:
 stdout=ROOT/(name+'.stdout');stderr=ROOT/(name+'.stderr')
 with stdout.open('w') as out,stderr.open('w') as err:
  started=time.time();result=subprocess.run(cmd,cwd=RAW,stdout=out,stderr=err)
 row={'name':name,'argv':cmd,'exit':result.returncode,'seconds':time.time()-started,'stdout':str(stdout),'stderr':str(stderr),'output':str(output or stdout)}
 report['steps'].append(row)
 (ROOT/'archive-verification.json').write_text(json.dumps(report,indent=2)+'\n')
 print(name,result.returncode,flush=True)
 result.check_returncode()
report['raw_recomputation_complete']=len(steps)==5
(ROOT/'archive-verification.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({'payload_files':len(manifest),'payload_bytes':payload_bytes,'steps':len(steps)}))
