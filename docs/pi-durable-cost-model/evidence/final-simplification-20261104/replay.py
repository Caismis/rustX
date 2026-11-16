"""Authenticate the finite cleanup evidence and recompute offline; no engines."""
import argparse,hashlib,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--directory',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();assert not a.output.exists();load=lambda q:json.loads(q.read_text());sha=lambda b:hashlib.sha256(b).hexdigest();manifest=load(a.directory/'manifest.json')
with tempfile.TemporaryDirectory(prefix='rustx-final-simplification-replay-') as tmp:
 root=Path(tmp);seen=set()
 for item in manifest['archives']:
  path=a.directory/item['name'];assert path.stat().st_size==item['bytes'] and sha(path.read_bytes())==item['sha256'];expected=[v for v in manifest['members'] if v['archive']==item['name']]
  with tarfile.open(path,'r:xz') as archive:
   members=archive.getmembers();assert [m.name for m in members]==[v['path'] for v in expected]
   for member,v in zip(members,expected):
    rel=Path(member.name);assert member.isfile() and not rel.is_absolute() and '..' not in rel.parts and member.name not in seen;seen.add(member.name);body=archive.extractfile(member).read();assert len(body)==v['bytes'] and sha(body)==v['sha256'];q=root/rel;q.parent.mkdir(parents=True,exist_ok=True);q.write_bytes(body)
 prior={name:(root/name).read_bytes() for name in ['summary.json','tables.md']};commands=[]
 for name,args in [('audit-source.py',[]),('audit-native.py',['--raw-only']),('audit-requests.py',[]),('audit-cache.py',[]),('audit-hot.py',[]),('audit-host.py',[]),('audit-recheck.py',[]),('arithmetic.py',[]),('make-tables.py',[])]:
  cmd=[sys.executable,str(root/name),*args];done=subprocess.run(cmd,cwd=root,env=os.environ|{'PYTHONDONTWRITEBYTECODE':'1'},capture_output=True,text=True);commands.append({'script':name,'exit':done.returncode});assert done.returncode==0,(name,done.stdout[-2000:],done.stderr[-2000:])
 for name,body in prior.items():assert (root/name).read_bytes()==body,('recomputed output differs',name)
 result={'accepted':True,'authenticated_members':len(seen),'native_products':len(load(root/'native-audit.json')['products']),'native_images':sum(len(v['native_proofs']) for v in load(root/'native-audit.json')['products']),'full_records_checked':sum(row['fact_bound_full_records'] for v in load(root/'native-audit.json')['products'] for row in v['native_proofs']),'hot_products':len(load(root/'hot-audit.json')['products']),'functional_products':len(load(root/'hot-audit.json')['functional']),'recheck_hot_products':len(load(root/'navigation-recheck-audit.json')['products']),'history_groups':len(load(root/'hot-audit.json')['history']),'source_products':len(load(root/'hot-audit.json')['source']),'commands':commands,'scope':'Standard Python offline: no product, compiler, git, symbolizer, Node or network. Filesystem artifact basenames alone normalized; no wire/native/counter/content values normalized. Omitted executables authenticated by stored prearchive byte proofs, not rehashed as if present. Same summary and tables reproduced byte-for-byte.'}
 a.output.write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
