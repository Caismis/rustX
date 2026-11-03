"""Portable read-only replay. Never runs products, builds, git or network."""
import argparse,hashlib,json,subprocess,sys,tarfile,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--archive',type=Path,required=True);p.add_argument('--manifest',type=Path,required=True);p.add_argument('--output',type=Path,required=True);args=p.parse_args();assert not args.output.exists()
load=lambda p:json.loads(p.read_text());sha=lambda b:hashlib.sha256(b).hexdigest()
manifest=load(args.manifest);assert sha(args.archive.read_bytes())==manifest['archive']['sha256'];assert args.archive.stat().st_size==manifest['archive']['bytes']
with tempfile.TemporaryDirectory(prefix='rustx-hotpath-replay-') as temporary:
 root=Path(temporary)
 with tarfile.open(args.archive,'r:xz') as archive:
  assert archive.getnames()==[v['path'] for v in manifest['members']]
  for member,expected in zip(archive,manifest['members']):
   relative=Path(member.name);assert member.isfile() and not relative.is_absolute() and '..' not in relative.parts
   body=archive.extractfile(member).read();assert len(body)==expected['bytes'] and sha(body)==expected['sha256']
   target=root/relative;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(body)
 def run(script,*arguments):
  done=subprocess.run([sys.executable,str(root/script),*arguments],cwd=root,capture_output=True,text=True,env=__import__('os').environ|{'PYTHONDONTWRITEBYTECODE':'1'});assert done.returncode==0,done.stdout+done.stderr
 for label in ['baseline','complete']:
  f=root/f'wal-summary-{label}-corrected.json';old=load(f);f.unlink();run('analyze-wal.py','--label',label);assert load(f)==old
 old=load(root/'final-counter-analysis.json');run('analyze-counters.py');assert load(root/'final-counter-analysis.json')==old
 old=load(root/'support-audit.json');run('audit-support.py');assert load(root/'support-audit.json')==old
 old=load(root/'final-audit.json');run('audit-hotpath.py','--root',str(root),'--raw-only','--output',str(root/'replayed-audit.json'));new=load(root/'replayed-audit.json')
 assert old.pop('mode')=='actual-binary-and-raw' and new.pop('mode')=='raw-only'
 old.pop('created_utc');new.pop('created_utc');assert old==new and new['accepted']
 result={'accepted':True,'archive_sha256':manifest['archive']['sha256'],'verified_members':len(manifest['members']),'wal_reparsed':2,'counter_products_reparsed':8,'support_audit_equal':True,'formal_audit_equal':True,'scope':'Checks all regular members, then reparses raw WAL, diagnostic counters, native recorded oracles and counter arithmetic. Does not execute products/builds/network or freshly verify absent executable bytes.'}
 args.output.write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
