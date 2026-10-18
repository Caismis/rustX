#!/usr/bin/env python3
"""Summarize same-process heap-profile deltas using verified ELF symbol addresses.
Usage: analyze-heap.py --binary /path/to/executable nm-symbols.txt before.heap after.heap
The executable must be PIE with its first PT_LOAD at virtual address zero.
"""
import argparse,bisect,collections,json,re
from pathlib import Path
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument("--binary",type=Path,required=True)
parser.add_argument("symbols",type=Path)
parser.add_argument("before",type=Path)
parser.add_argument("after",type=Path)
args=parser.parse_args()
symbols=[]
for l in args.symbols.open():
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
   fields=l.split(None,5)
   if len(fields)==6 and re.fullmatch(r'[0-9a-f]+-[0-9a-f]+',fields[0]):
    mapped=fields[5].strip().removesuffix(' (deleted)')
    if Path(mapped).resolve()==args.binary.resolve():
     start,end=(int(x,16) for x in fields[0].split('-'))
     maps.append((start,end,int(fields[2],16)))
 if not maps:
  parser.error(f'{path}: no mappings for executable {args.binary.resolve()}; use the executable path recorded in this profile')
 if not any(offset==0 for _,_,offset in maps):
  parser.error(f'{path}: executable mapping at file offset zero is missing; cannot establish PIE load base')
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
b,a=map(analyze,(args.before,args.after));out={'before_header':b['header'],'after_header':a['header'],'method':'Exact cumulative requested allocation totals differenced at two boundaries in one heap-profile run. Nearest executable ELF function symbol, no inline expansion. First stack frame is allocation call site; owner is innermost symbol containing rustx:: or conformance::. Owner and leaf tables overlap; never add them.'}
for k in ['bytes','counts','innermost_named_owner_bytes']:
 d=collections.Counter({name:a[k][name]-b[k][name] for name in a[k].keys()|b[k].keys()});out[k]=d.most_common()
print(json.dumps(out,indent=2))
