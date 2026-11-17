import json,pathlib,subprocess,collections
root=pathlib.Path('/tmp/pr556-step-experiments/cpu');root.mkdir(exist_ok=True)
for file in root.glob('*.cpuprofile'):
 profile=json.loads(file.read_text());nodes={n['id']:n for n in profile['nodes']};parents={child:n['id']for n in profile['nodes']for child in n.get('children',[])};self_us=collections.Counter();inclusive=collections.Counter();counts=collections.Counter()
 for identity,delta in zip(profile.get('samples',[]),profile.get('timeDeltas',[])):
  name=nodes[identity]['callFrame']['functionName'];self_us[name]+=delta;counts[name]+=1;current=identity;seen=set()
  while current in nodes:
   name=nodes[current]['callFrame']['functionName']
   if name not in seen:inclusive[name]+=delta;seen.add(name)
   current=parents.get(current)
 names=list(set(self_us)|set(inclusive));demangled=subprocess.check_output(['/tmp/pr556-emsdk/upstream/bin/llvm-cxxfilt'],input='\n'.join(names)+'\n',text=True).splitlines();mapping=dict(zip(names,demangled));total=sum(profile.get('timeDeltas',[]));rows=[{'function':name,'demangled':mapping.get(name,name),'selfSamples':counts[name],'selfWeightedMs':self_us[name]/1000,'selfPercent':self_us[name]/total*100,'inclusiveWeightedMs':inclusive[name]/1000,'inclusivePercent':inclusive[name]/total*100}for name in names];report={'scope':'V8 1ms native-read CPU sampling; weights use actual sample time deltas and are not exclusive phase clocks','samples':len(profile.get('samples',[])),'weightedTotalMs':total/1000,'topSelf':sorted(rows,key=lambda r:r['selfWeightedMs'],reverse=True)[:35],'topInclusive':sorted(rows,key=lambda r:r['inclusiveWeightedMs'],reverse=True)[:35],'allFunctions':rows};file.with_suffix('.summary.json').write_text(json.dumps(report,indent=2)+'\n');print(file.name,'samples',report['samples']);print('\n'.join(f"{r['selfPercent']:.1f}% {r['demangled']}"for r in report['topSelf'][:12]))
