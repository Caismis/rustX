"""Portable raw-only replay; never executes products, builds or network calls."""
import argparse,hashlib,json,subprocess,sys,tarfile,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--archive',type=Path,required=True);p.add_argument('--manifest',type=Path,required=True);p.add_argument('--output',type=Path,required=True);args=p.parse_args();assert not args.output.exists()
manifest=json.loads(args.manifest.read_text());digest=hashlib.sha256(args.archive.read_bytes()).hexdigest();assert digest==manifest['archive']['sha256'] and args.archive.stat().st_size==manifest['archive']['bytes']
with tempfile.TemporaryDirectory(prefix='rustx-statistics-replay-') as temporary:
 root=Path(temporary);entries=manifest['members']
 with tarfile.open(args.archive,'r:xz') as t:
  assert t.getnames()==[e['path'] for e in entries]
  for member,e in zip(t,entries):
   relative=Path(member.name);assert member.isfile() and not relative.is_absolute() and '..' not in relative.parts
   body=t.extractfile(member).read();assert len(body)==e['bytes'] and hashlib.sha256(body).hexdigest()==e['sha256']
   target=root/relative;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(body)
 def run(path,*args):
  done=subprocess.run([sys.executable,str(path),*args],cwd=path.parent,text=True,capture_output=True);assert done.returncode==0,done.stderr+done.stdout
 original=json.loads((root/'explanation-analysis.json').read_text());(root/'explanation-analysis.json').unlink();run(root/'analyze-explanation.py');assert json.loads((root/'explanation-analysis.json').read_text())==original
 for base,script in [(root,'audit-results-corrected.py'),(root/'refined','audit-results.py')]:
  for label in ['baseline','complete']:
   path=base/('wal-summary-'+label+'-corrected.json');original=json.loads(path.read_text());path.unlink();run(base/'analyze-wal.py','--label',label);assert json.loads(path.read_text())==original
  run(base/script,'--raw-only','--output',str(base/'replayed.json'));actual=json.loads((base/'replayed.json').read_text());recorded=json.loads((base/'audit.json').read_text());assert actual.pop('raw_only') is True and recorded.pop('raw_only') is False and actual==recorded and actual['accepted']
 result={'accepted':True,'archive_sha256':digest,'verified_members':len(entries),'explanation_and_both_candidate_counter_replays_equal_retained':True,'all_four_wal_summaries_reparsed':True,'scope':'All members verified, then read-only explanation, WAL and both counter audits recomputed. No products/builds/network/current source/absent binaries executed or freshly verified; binary identities are retained build records.'}
 args.output.write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
