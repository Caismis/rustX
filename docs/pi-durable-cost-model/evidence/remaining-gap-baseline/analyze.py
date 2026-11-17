import ast,collections,json,re,struct
from pathlib import Path
root=Path(__file__).resolve().parent
summary={}
for engine in ['rust','pi']:
 path=root/('complete-'+engine);before=json.loads((path/'before_measured_turns-database.json').read_text())[0];after=json.loads((path/'after_turns-database.json').read_text())[0]
 owners=collections.defaultdict(set)
 for snapshot in [before,after]:
  for name,page,*_ in snapshot['pages']:owners[page].add(name)
 frames=collections.Counter();commits=0;pending={};phase='startup';allcommits=collections.Counter();unresolved=0;datawrites=collections.Counter();txpages=[];tx=collections.Counter();active=False
 for line in (path/'calls.strace').read_text().splitlines():
  m=re.match(r'(\d+)\s+(.*)',line)
  if not m:continue
  pid,call=m.groups()
  if '<unfinished ...>' in call:
   assert pid not in pending;pending[pid]=call.split('<unfinished ...>')[0];continue
  m=re.match(r'<\.\.\. \w+ resumed>(.*)',call)
  if m:
   assert pid in pending,(pid,call);call=pending.pop(pid)+m[1]
  if 'UNIX-STREAM' in call and 'before_measured_turns' in call:phase='steady';active=True
  if 'UNIX-STREAM' in call and 'after_turns' in call:phase='after_turns';active=False
  m=re.match(r'pwrite64\(\d+<([^>]+-wal)>, ("(?:\\.|[^"\\])*"), (\d+), (\d+)\) = (\d+)$',call)
  if not m:continue
  fn,quoted,n,offset,ret=m.groups();n,offset,ret=map(int,(n,offset,ret));assert ret==n
  if n!=24:continue
  data=ast.literal_eval('b'+quoted);assert len(data)==24
  page,size=struct.unpack('>II',data[:8]);assert (offset-32)%4120==0
  if active:
   frames[page]+=1;tx[page]+=1
  if size:
   allcommits[phase]+=1
   if active:commits+=1;txpages.append(dict(tx));tx.clear()
 assert not pending;assert not tx;assert phase=='after_turns';assert commits>0
 bytable=collections.Counter()
 for page,n in frames.items():bytable['|'.join(sorted(owners[page])) or '<unmapped>']+=n
 delta={k:after['counts'][k]-before['counts'].get(k,0) for k in after['counts'] if after['counts'][k]!=before['counts'].get(k,0)}
 s={'steady_turns':32,'wal_write_commits':commits,'wal_frames':sum(frames.values()),'wal_frame_bytes':sum(frames.values())*4120,'commits_by_phase':allcommits,'wal_frames_by_snapshot_page_owner':dict(bytable.most_common()),'net_row_count_changes':delta,'transaction_page_counts':[sum(t.values()) for t in txpages],'limitations':'WAL write-commit markers, not read-only SQL commits. Snapshot dbstat ownership at endpoints; mixed owners or unmapped pages explicit, intermediate page reuse not excluded. No exclusive timing or semantic parity inference.'}
 if engine=='rust':
  old={n['id'] for n in before['source_nodes']};leaves=[n for n in after['source_nodes'] if n['id'] not in old and n['metadata']['content']['type']=='leaf'];geometry=collections.Counter((len(n['metadata']['content']['leaf']['variants']),len(n['metadata']['content']['leaf']['templates']),len(set(n['metadata']['content']['leaf']['variants']))) for n in leaves)
  s['new_source_leaves']=len(leaves);s['new_leaf_geometry']=[{'width_classes':g[0],'templates':g[1],'selected_unique':g[2],'leaves':c} for g,c in geometry.items()];s['source_derived_width_loads']=sum(len(n['metadata']['content']['leaf']['variants']) for n in leaves);s['source_derived_path_loads']=sum(len(n['metadata']['content']['leaf']['templates']) for n in leaves);s['source_derived_base_hole_encodes']=s['source_derived_width_loads'];s['template_frequency_scope']='Derived from successful persisted new leaf geometry plus unconditional production loops, not function-entry instrumentation; excludes reused nodes and other template consumers.'
 else:
  a=json.loads((path/'pi-sql-before_measured_turns.json').read_text());b=json.loads((path/'pi-sql-after_turns.json').read_text());d={k:{v:b[k][v]-a.get(k,{}).get(v,0) for v in ['calls','ns']} for k in b};d={k:v for k,v in d.items() if v['calls']};s['sql_counts']=d;s['sql_totals']={method:sum(v['calls'] for k,v in d.items() if k.startswith(method+' ')) for method in ['prepare','exec','run','get','all']};s['sql_native_elapsed_ms']=sum(v['ns'] for v in d.values())/1e6
  profile=json.loads((path/'pi-steady.cpuprofile').read_text());nodes={n['id']:n for n in profile['nodes']};parents={c:n['id'] for n in profile['nodes'] for c in n.get('children',[])};leaf=collections.Counter();cum=collections.Counter()
  for sample in profile['samples']:
   leaf[sample]+=1;seen=set()
   while sample not in seen:
    seen.add(sample);cum[sample]+=1
    if sample not in parents:break
    sample=parents[sample]
  s['v8_cpu']={'total_samples':len(profile['samples']),'duration_us':profile['endTime']-profile['startTime'],'top_leaf':[dict(nodes[i]['callFrame'],samples=n) for i,n in leaf.most_common(35)],'top_cumulative':[dict(nodes[i]['callFrame'],samples=n) for i,n in cum.most_common(40)],'limitation':'V8 isolate samples include idle/native wrapper time; not process CPU percentages. Cumulative rows overlap.'}
 summary[engine]=s
(root/'summary.json').write_text(json.dumps(summary,indent=2));print(json.dumps({e:{k:v for k,v in s.items() if k not in ['sql_counts','v8_cpu','transaction_page_counts']} for e,s in summary.items()},indent=2))
