#!/usr/bin/env python3
"""Summarize same-process heap-profile deltas using verified ELF symbol addresses.
Usage: analyze-heap.py nm-symbols.txt before.heap after.heap
The executable must be PIE with its first PT_LOAD at virtual address zero.
"""
import bisect,collections,json,re,sys
from pathlib import Path
symbols=[]
for l in Path(sys.argv[1]).open():
 p=l.strip().split(None,2)
 if len(p)==3 and p[1] in 'tTwW':
  try:symbols.append((int(p[0],16),p[2]))
  except ValueError:pass
symbols.sort();addresses=[p[0] for p in symbols]

def analyze(path):
 totals=collections.Counter();owners=collections.Counter();counts=collections.Counter()
 maps=[]
 with open(path) as f:
  for l in f:
   if 'attribution-conformance' in l:
    p=l.split();start,end=map(lambda x:int(x,16),p[0].split('-'));maps.append((start,end,start-int(p[2],16)))
 base=min(start for start,end,old_base in maps)
 maps=[(start,end,base) for start,end,old_base in maps]
 cache={}
 def sym(pc):
  if pc not in cache:
   name='external:'+hex(pc)
   for start,end,base in maps:
    if start<=pc<end:
     idx=bisect.bisect_right(addresses,pc-base-1)-1
     if idx>=0:name=symbols[idx][1]
     break
   cache[pc]=name
  return cache[pc]
 with open(path) as f:
  header=f.readline().strip()
  for l in f:
   m=re.match(r'\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\]\s*@ (.*)',l)
   if not m:continue
   live,livebytes,n,b=map(int,m.groups()[:4]);stack=[sym(int(p,16)) for p in m[5].split()]
   totals[stack[0]]+=b;counts[stack[0]]+=n
   owner=next((x for x in stack if 'rustx::' in x or 'conformance::' in x),'unclassified')
   owners[owner]+=b
 return {'header':header,'bytes':totals,'counts':counts,'innermost_named_owner_bytes':owners}
b,a=map(analyze,sys.argv[2:4]);out={'before_header':b['header'],'after_header':a['header'],'method':'Exact cumulative requested allocation totals differenced at two boundaries in one heap-profile run. Nearest executable ELF function symbol, no inline expansion. First stack frame is allocation call site; owner is innermost symbol containing rustx:: or conformance::. Owner and leaf tables overlap; never add them.'}
for k in ['bytes','counts','innermost_named_owner_bytes']:
 d=collections.Counter({name:a[k][name]-b[k][name] for name in a[k].keys()|b[k].keys()});out[k]=d.most_common()
print(json.dumps(out,indent=2))
