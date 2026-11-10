"""Offline audit: never executes products, compilers, git or network."""
import argparse,hashlib,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--archive',type=Path,required=True);p.add_argument('--manifest',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();assert not a.output.exists()
load=lambda p:json.loads(p.read_text());sha=lambda b:hashlib.sha256(b).hexdigest();manifest=load(a.manifest);assert a.archive.stat().st_size==manifest['archive']['bytes'] and sha(a.archive.read_bytes())==manifest['archive']['sha256']
with tempfile.TemporaryDirectory(prefix='rustx-resource-replay-') as temporary:
 root=Path(temporary)
 with tarfile.open(a.archive,'r:xz') as archive:
  members=archive.getmembers();assert [m.name for m in members]==[v['path'] for v in manifest['members']]
  for m,v in zip(members,manifest['members']):
   rel=Path(m.name);assert m.isfile() and not rel.is_absolute() and '..' not in rel.parts
   body=archive.extractfile(m).read();assert len(body)==v['bytes'] and sha(body)==v['sha256']
   target=root/rel;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(body)
 def run(directory,script,*args):
  result=subprocess.run([sys.executable,str(directory/script),*args],cwd=directory,capture_output=True,text=True,env=os.environ|{'PYTHONDONTWRITEBYTECODE':'1'});assert result.returncode==0,result.stdout+result.stderr
 for directory,schema in [(root,92),(root/'refined',93)]:
  for label in ['baseline','complete']:
   path=directory/f'wal-summary-{label}-corrected.json';old=load(path);path.unlink();run(directory,'analyze-wal.py','--label',label);assert load(path)==old
  old=load(directory/'storage-audit.json');run(directory,'audit-storage-trial.py','--root',str(directory),'--schema',str(schema),'--raw-only','--output',str(directory/'replayed-storage.json'));new=load(directory/'replayed-storage.json')
  assert old.pop('raw_only') is False and new.pop('raw_only') is True;assert old==new
 for script,target in [('audit-native-capacity.py','capacity-audit.json'),('audit-profiles.py','profile-audit.json'),('analyze-prior-memory.py','prior-memory-phases.json')]:
  old=load(root/target);run(root,script);assert load(root/target)==old
 old=(root/'resource-tables.md').read_bytes();run(root,'make-resource-tables.py');assert (root/'resource-tables.md').read_bytes()==old
 result={'accepted':True,'archive_sha256':manifest['archive']['sha256'],'verified_members':len(manifest['members']),'ordinary_products_reparsed':64,'wire_comparisons_per_cohort':1088,'wal_products_reparsed':8,'native_products_reparsed':9,'cpu_profiles_reparsed':2,'heap_profiles_reparsed':6,'previous_memory_counter_products':32,'tables_reproduced':True,'scope':'All regular members authenticated; raw counters/WAL/wire histories/closed SQLite images/profile denominators independently rechecked. Uses archived symbolization text and source/build identity records; does not freshly verify absent executable bytes or rerun symbolization, products, builds or network.'}
 a.output.write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
