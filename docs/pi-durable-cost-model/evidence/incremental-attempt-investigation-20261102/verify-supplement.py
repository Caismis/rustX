#!/usr/bin/env python3
"""Verify and safely extract the six separate profiler products; execute no products."""
import argparse,hashlib,json,re,struct,tarfile
from pathlib import Path,PurePosixPath

def require(ok,msg):
 if not ok:raise ValueError(msg)
def sha(p):
 with Path(p).open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def load(p):return json.loads(Path(p).read_text())
def safe(name):
 p=PurePosixPath(name);require(name and not p.is_absolute() and p.as_posix()==name and '\\' not in name and '\x00' not in name and all(x not in ('','.','..') for x in name.split('/')),'unsafe member');return name
def cpu_total(path):
 data=path.read_bytes();h=struct.unpack_from('<5Q',data);require(h[:3]==(0,3,0),'CPU header');o=40;total=0
 while True:
  n,d=struct.unpack_from('<2Q',data,o);o+=16;require(0<d<=1024,'CPU depth');pcs=struct.unpack_from(f'<{d}Q',data,o);o+=8*d
  if n==0 and pcs==(0,):return total,h[3]
  total+=n

def verify(root):
 binding=load(root/'binding.json');require(binding['formal_archive_sha256']=='06d963327a1ef17af141c3fdbb5e1e4923d7936c8cbd323440fe578751f1bb91','formal pin')
 rows=load(root/'commands.json');plan=load(root/'plan.json');require(len(rows)==plan['profile_products']==6,'product scope');require(all(r['status']=='complete' and r['exit']==0 for r in rows),'product failure')
 for row,(mode,label) in zip(rows,plan['order'],strict=True):
  d=root/row['name'];m=load(d/'manifest.json');process=load(d/'sample.process.json');sample=load(d/'sample.json')
  require(m['mode']==mode and m['case']=='short-large' and m['observer_mode']=='live-subscriber','fixture mismatch')
  require(m['binary_sha256']==row['binary_sha256']==binding['binaries'][label],'binary pin')
  require(process['accepted'] is True and process['product_exit']==0,'controller acceptance')
  require(sample['schema']==(86 if label=='baseline' else 87) and sample['history']==64 and sample['measured']==32 and sample['oracle_version']==2,'sample scope')
 counts=[]
 for r in load(root/'cpu-profile-summary.json')['profiles']:
  d=root/r['name'];n,interval=cpu_total(d/'before_measured_turns.cpu');text=(d/'steady-cumulative.txt').read_text().splitlines();require(text[0]==f'Total: {n} samples' and n==r['samples'] and interval==r['interval_us'],'CPU count')
  require(sha(d/'before_measured_turns.cpu')==r['profile_sha256'],'CPU hash');counts.append({'name':r['name'],'samples':n})
 heaps={}
 for r in load(root/'heap-completeness.json')['profiles']:
  d='0-heap-baseline' if r['label']=='baseline' else '1-heap-candidate';log=root/d/'sample.log';require((r['phase'],r['phase_name']) in [(4,'before_measured_turns'),(5,'after_turns')],'heap phase');require(sha(log)==r['control_log_sha256'] and f"alloc.{r['phase']:04d}.heap ({r['phase_name']})" in log.read_text(),'heap phase log');p=root/d/f"alloc.{r['phase']:04d}.heap";lines=p.read_text().splitlines();header=list(map(int,re.fullmatch(r'heap profile:\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\]\s*@ heapprofile',lines[0]).groups()));entries=[]
  for line in lines[1:]:
   m=re.match(r'\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\]\s*@ ',line)
   if m:entries.append(list(map(int,m.groups())))
  totals=[sum(v[i] for v in entries) for i in range(4)]
  heaps[r['label'],r['phase']]=header
  require(header==r['header_totals'] and totals==r['retained_stack_totals'] and (header==totals)==r['stack_records_complete'] and p.stat().st_size==r['bytes'],'heap completeness mismatch')
 require(set(heaps)=={(label,phase) for label in ['baseline','candidate'] for phase in [4,5]},'heap inventory')
 deltas={label:{'allocated_count':heaps[label,5][2]-heaps[label,4][2],'allocated_bytes':heaps[label,5][3]-heaps[label,4][3]} for label in ['baseline','candidate']}
 require(deltas==load(root/'heap-completeness.json')['header_only_window_deltas'],'heap deltas')
 return {'heap_header_only_window_deltas':deltas,'products':6,'cpu_products':4,'heap_products':2,'cpu_samples':counts,'heap_stack_attribution_complete':False,'scope':'Hash integrity, retained controller acceptance and independently recomputed raw profile counts/completeness. No fresh execution, symbolization or causal explanation of formal latency.'}

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,default=Path(__file__).resolve().parent);p.add_argument('--work-dir',type=Path,required=True);a=p.parse_args();require(not a.work_dir.exists(),'work must be fresh');summary=load(a.evidence/'summary.json');require(summary['format']=='rustx-incremental-attempt-regression-investigation-v2','format');archive=a.evidence/'raw.tar.xz';require(sha(archive)==summary['archive_sha256'] and archive.stat().st_size==summary['archive_bytes'],'archive identity');a.work_dir.mkdir(parents=True);raw=a.work_dir/'raw';raw.mkdir();seen=set();maximum=sum(r['bytes'] for r in summary['files'].values());total=0
 with tarfile.open(archive,'r|xz') as tar:
  for m in tar:
   safe(m.name);require(m.isfile() and not m.issparse() and m.name not in seen and m.name in summary['files'],'unexpected member');record=summary['files'][m.name];require(m.size==record['bytes'],'member size');total+=m.size;require(total<=maximum,'archive exceeds declared size');seen.add(m.name);out=raw/m.name;out.parent.mkdir(parents=True,exist_ok=True)
   with tar.extractfile(m) as src,out.open('xb') as dst:
    while chunk:=src.read(1024*1024):dst.write(chunk)
   require(sha(out)==record['sha256'],'member hash')
 require(seen==set(summary['files']),'missing members');require(sha(raw/'verify-supplement.py')==sha(__file__),'verifier differs');result=verify(raw)
 require({str(x.relative_to(raw)):{'bytes':x.stat().st_size,'sha256':sha(x)} for x in raw.rglob('*') if x.is_file()}==summary['files'],'final inventory differs')
 receipt={'accepted':True,'archive_sha256':summary['archive_sha256'],'verified_files':len(seen),'verification_script_sha256':sha(__file__),'recomputed':result};(a.evidence/'portable-verification.json').open('x').write(json.dumps(receipt,indent=2)+'\n');print(json.dumps(receipt,indent=2))
if __name__=='__main__':main()
