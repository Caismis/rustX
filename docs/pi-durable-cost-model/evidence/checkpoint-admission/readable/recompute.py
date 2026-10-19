#!/usr/bin/env python3
"""Independent raw-counter verification; no imports from benchmark summarizers."""
import argparse,csv,hashlib,json,math,re,statistics,tarfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--root',type=Path);p.add_argument('--archive',type=Path);p.add_argument('--readable',type=Path,required=True);a=p.parse_args()
assert bool(a.root)!=bool(a.archive)
rows=list(csv.DictReader((a.readable/'paired-costs.csv').open()))
intervals=json.loads((a.readable/'paired-uncertainty.json').read_text())['records']
hashes=json.loads((a.readable/'input-hashes.json').read_text())
wanted={}
for row in rows:
 folder='checkpoint-admission-comparison'
 for side in ['before','after']:
  name=f"{folder}/{row['scope']}/{row['pair']}-{row['case']}-{side}.json";wanted[name]=(row['scope'],row['case'])
def counters(point):
 v=point['process'];f=v['stat'].rsplit(')',1)[1].split();io=dict(line.split(':',1) for line in v['io'].splitlines());mem=dict(line.split(':',1) for line in v['smaps_rollup'].splitlines() if ':' in line)
 return (int(f[11])+int(f[12]))*10,int(io['write_bytes']),int(mem['Pss_Anon'].split()[0])/1024

def derive(value,scope,case):
 assert value['oracle']=='passed'
 if scope.endswith('append'):assert value['measured']==32
 phases={x['phase']:x for x in value['lifecycle']};first=phases['before_initialization'];last=phases['after_cleanup'];append=scope.endswith('append')
 points=list(value['lifecycle'])
 if append:
  assert value['history']==int(case) and value['cold']==scope.startswith('cold')
  start,end=phases['after_seed'],phases['after_appends']
  cpu=counters(end)[0]-counters(start)[0]
  if value['cold']:
   assert len(value['operation_counters'])==32
   cpu=sum(counters({'process':r['after']})[0]-counters({'process':r['before']})[0] for r in value['operation_counters'])
  points += [{'process':r[side]} for r in value['operation_counters'] for side in ['before','after']]
  result={'mean_append_ms':statistics.mean(value['append_ns'])/1000000,'append_cpu_ms_per_append':cpu/32,'lifecycle_cpu_ms':counters(last)[0]-counters(first)[0],'lifecycle_write_mib':(counters(last)[1]-counters(first)[1])/1048576}
 else:
  assert value['workload_version']==3 and value['model_time']=='2026-10-19 00:00:00' and value['observer_mode']==scope and value['case']==case
  expected_history={'short-small':8,'short-large':64}.get(case,0);assert value['history']==expected_history
  assert value['measured']==(32 if expected_history else 1)
  assert len(value['samples'])==expected_history+value['measured']
  start=value['samples'][expected_history-1] if expected_history else phases['after_initialization'];end=value['samples'][-1]
  points+=value['samples']
  result={'mean_turn_ms':statistics.mean(r['settled_us']/1000 for r in value['samples'] if r['measured']),'turn_cpu_ms_per_turn':(counters(end)[0]-counters(start)[0])/value['measured'],'through_cleanup_self_cpu_ms':counters(last)[0],'through_cleanup_process_write_mib':counters(last)[1]/1048576}
 result['max_sampled_anon_pss_mib']=max(counters(r)[2] for r in points)
 return result
raw={};statuses={};checked=0
if a.root:
 for name in wanted:
  data=(a.root/name).read_bytes();assert hashlib.sha256(data).hexdigest()==hashes[name];scope,case=wanted[name];raw[name]=derive(json.loads(data),scope,case)
  status=name.removesuffix('.json')+'.process.json';data=(a.root/status).read_bytes();assert hashlib.sha256(data).hexdigest()==hashes[status];assert json.loads(data)['accepted']
else:
 with tarfile.open(a.archive,'r:xz') as archive:manifest=json.load(archive.extractfile('MANIFEST.json'))
 with tarfile.open(a.archive,'r|xz') as archive:
  for member in archive:
   if member.name=='MANIFEST.json':continue
   data=archive.extractfile(member).read();entry=manifest[member.name];assert len(data)==entry['bytes'] and hashlib.sha256(data).hexdigest()==entry['sha256'];checked+=1
   if member.name in wanted:
    assert hashlib.sha256(data).hexdigest()==hashes[member.name];scope,case=wanted[member.name];raw[member.name]=derive(json.loads(data),scope,case)
   elif member.name in hashes and member.name.endswith('.process.json'):statuses[member.name]=json.loads(data)['accepted']
 assert checked==len(manifest)
 for name in wanted:assert statuses[name.removesuffix('.json')+'.process.json'] is True
assert len(raw)==120
close=lambda x,y: math.isclose(x,y,rel_tol=1e-12,abs_tol=1e-12)
groups={}
for row in rows:
 cohort,scope,case,pair,metric=(row[k] for k in ['cohort','scope','case','pair','metric']);folder='checkpoint-admission-comparison'
 before=raw[f'{folder}/{scope}/{pair}-{case}-before.json'][metric];after=raw[f'{folder}/{scope}/{pair}-{case}-after.json'][metric]
 assert close(float(row['before']),before) and close(float(row['after']),after)
 ratio=after/before if before>0 and after>0 else None
 assert (not row['ratio']) if ratio is None else close(float(row['ratio']),ratio)
 groups.setdefault((cohort,scope,case,metric),[]).append(ratio)
verified=0
for rec in intervals:
 key=tuple(rec[k] for k in ['cohort','scope','case','metric']);values=groups[key];assert values==rec['ratios'] and len(values)==rec['n']
 if len(values)!=3 or any(v is None for v in values):assert rec['state'].startswith('incomplete_or_quantized');continue
 logs=list(map(math.log,values));mean=statistics.mean(logs);width=2.919985580355516*statistics.stdev(logs)/math.sqrt(3)
 expected={'paired_median':statistics.median(values),'geometric_mean':math.exp(mean),'two_sided_90_lower':math.exp(mean-width),'one_sided_95_upper':math.exp(mean+width),'min':min(values),'max':max(values)}
 assert all(close(rec[k],v) for k,v in expected.items());verified+=1
assert len(rows)==300 and len(groups)==100 and verified==100
print(json.dumps({'raw_samples':len(raw),'raw_counter_values':len(rows)*2,'paired_rows':len(rows),'metric_groups':len(groups),'verified_positive_three_pair_intervals':verified,'incomplete_or_quantized_groups':len(groups)-verified,'archive_members_verified':checked,'all_match':True}))
