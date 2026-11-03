"""Authenticate and recompute archived measurements. No product/build/git/network execution."""
import argparse,hashlib,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--archive',type=Path,required=True);p.add_argument('--manifest',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();assert not a.output.exists();load=lambda p:json.loads(p.read_text());sha=lambda b:hashlib.sha256(b).hexdigest();manifest=load(a.manifest)
assert a.archive.stat().st_size==manifest['archive']['bytes'] and sha(a.archive.read_bytes())==manifest['archive']['sha256']
with tempfile.TemporaryDirectory(prefix='rustx-native-repair-replay-') as tmp:
 root=Path(tmp)
 with tarfile.open(a.archive,'r:xz') as archive:
  members=archive.getmembers();assert [m.name for m in members]==[v['path'] for v in manifest['members']]
  for m,v in zip(members,manifest['members']):
   rel=Path(m.name);assert m.isfile() and not rel.is_absolute() and '..' not in rel.parts;body=archive.extractfile(m).read();assert len(body)==v['bytes'] and sha(body)==v['sha256'];target=root/rel;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(body)
 def run(script,*args):
  result=subprocess.run([sys.executable,str(root/script),*args],cwd=root,capture_output=True,text=True,env=os.environ|{'PYTHONDONTWRITEBYTECODE':'1'});assert result.returncode==0,(script,result.stdout,result.stderr)
 old=load(root/'native-audit.json');run('audit-native.py','--raw-only');new=load(root/'native-audit.json');assert old.pop('raw_only') is False and new.pop('raw_only') is True;assert old==new
 for script,target in [('audit-functional.py','functional-audit.json'),('audit-source.py','source-audit.json'),('audit-cpu.py','cpu-audit.json'),('audit-pi-memory.py','pi-memory-audit.json'),('audit-host.py','host-audit.json'),('summarize.py','summary.json')]:
  old=load(root/target);run(script);assert load(root/target)==old,target
 old=(root/'native-repair-tables.md').read_bytes();run('make-tables.py');assert (root/'native-repair-tables.md').read_bytes()==old
 proof=load(root/'prearchive-proof.json');source=load(root/'source-audit.json');native=load(root/'native-audit.json');assert proof['accepted'] and proof['source_inputs']==source['source_inputs']==972;assert proof['source_head']==source['head'];assert len(proof['executables'])==4;assert proof['pi']['products']==1
 for row in proof['tcp_wrappers']:assert sha((root/(row['label']+'.sh')).read_bytes())==row['wrapper_sha256'] and row['successful_fd_settings']==8
 assert sha((root/'tcp-nodelay.c').read_bytes())==proof['tcp_helper']['source_sha256'];assert sha((root/'pi128-memory/identity.json').read_bytes())==proof['pi']['identity_sha256']
 result={'accepted':True,'archive_sha256':manifest['archive']['sha256'],'verified_members':len(manifest['members']),'native_products_reparsed':len(native['products']),'ordinary_native_products':sum(not v['diagnostic'] for v in native['products']),'native_closed_images':sum(v['sessions'] for v in native['products']),'functional_products':len(load(root/'functional-audit.json')['products']),'cpu_profiles':len(load(root/'cpu-audit.json')['profiles']),'pi_memory_products':1,'pi_native_images':len(load(root/'pi-memory-audit.json')['native_proofs']),'tests':source['tests'],'tables_sha256':sha(old),'tables_reproduced':True,'scope':'Authenticates every selected member and independently rechecks raw native counters/full wire histories/closed images/cancellation absence, functional requests, CPU sample denominators, Pi memory/API/SQLite records, source maps, host observations and all tables. No product, compiler, git, symbolizer or network execution. Uses stored prearchive executable/library/Pi-runtime proofs for omitted artifacts; does not freshly verify their bytes.'}
 a.output.write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
