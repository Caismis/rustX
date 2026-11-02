"""Portable retained-counter replay. Never executes product or build commands."""
import argparse,hashlib,json,subprocess,sys,tarfile,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--archive',type=Path,required=True);p.add_argument('--manifest',type=Path,required=True);p.add_argument('--output',type=Path,required=True);args=p.parse_args()
assert not args.output.exists()
manifest=json.loads(args.manifest.read_text());digest=hashlib.sha256(args.archive.read_bytes()).hexdigest();assert digest==manifest['archive']['sha256'] and args.archive.stat().st_size==manifest['archive']['bytes']
with tempfile.TemporaryDirectory(prefix='rustx-observation-replay-') as temporary:
 root=Path(temporary);entries=manifest['members']
 with tarfile.open(args.archive,'r:xz') as t:
  assert t.getnames()==[e['path'] for e in entries]
  for member,e in zip(t,entries):
   relative=Path(member.name);assert member.isfile() and not relative.is_absolute() and '..' not in relative.parts
   body=t.extractfile(member).read();assert len(body)==e['bytes'] and hashlib.sha256(body).hexdigest()==e['sha256']
   target=root/relative;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(body)
 command=[sys.executable,str(root/'audit-results-corrected.py'),'--raw-only','--output',str(root/'replayed.json')]
 done=subprocess.run(command,cwd=root,text=True,capture_output=True);assert done.returncode==0,done.stderr+done.stdout
 actual=json.loads((root/'replayed.json').read_text());recorded=json.loads((root/'audit.json').read_text());assert actual.pop('raw_only') is True and recorded.pop('raw_only') is False and actual==recorded and actual['accepted']
 result={'accepted':True,'archive_sha256':digest,'verified_members':len(entries),'counter_replay_equals_retained_strict_audit':True,'scope':'All archived members verified and retained counters recomputed. No products, builds, current source or absent binaries executed/verified. Binary identities are retained build records.','audit_stdout':done.stdout}
 args.output.write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
