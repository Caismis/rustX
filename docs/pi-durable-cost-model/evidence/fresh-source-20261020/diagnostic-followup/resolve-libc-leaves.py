#!/usr/bin/env python3
"""Offline exact-build-ID leaf resolution; leaves outside matching libc stay unknown."""
import argparse
import collections
import csv
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--profiles',type=Path,default=Path('/workspace/pr485-study/evidence/minimal-attribution-20261020'))
parser.add_argument('--output',type=Path,default=Path('/workspace/pr485-study/evidence/libc-symbol-resolution-20261020'))
args=parser.parse_args();out=args.output;debug=out/'libc.debug'
identity=json.loads((out/'debug-identity.json').read_text())
assert hashlib.file_digest(debug.open('rb'),'sha256').hexdigest()==identity['debug_sha256']
result=subprocess.run(['readelf','-n',str(debug)],capture_output=True,text=True,check=True)
assert re.search(r'Build ID: (\w+)',result.stdout)[1]==identity['debug_build_id']=='c495b62edadd6c356265942ec1282d98058a7b41'
all_rows=[];summaries={};commands=[]
for label in ['baseline','geometry']:
    path=args.profiles/(label+'-cpu')/'before_measured_turns.cpu';raw=path.read_bytes();offset=40;leaves=collections.Counter()
    assert struct.unpack_from('<5Q',raw)[:3]==(0,3,0)
    while True:
        count,depth=struct.unpack_from('<QQ',raw,offset);offset+=16
        stack=struct.unpack_from('<'+'Q'*depth,raw,offset);offset+=8*depth
        if count==0 and depth==1 and stack[0]==0:break
        leaves[stack[0]]+=count
    maps=[]
    for line in raw[offset:].decode().splitlines():
        m=re.match(r'([0-9a-f]+)-([0-9a-f]+) (\S+) ([0-9a-f]+) [0-9a-f:]+ (\d+)\s*(/.*)',line)
        if m and m[6]=='/usr/lib/x86_64-linux-gnu/libc.so.6':maps.append((int(m[1],16),int(m[2],16),int(m[4],16),m[3]))
    rows=[]
    for pc,count in sorted(leaves.items()):
        match=next((m for m in maps if m[0]<=pc<m[1]),None)
        if match:
            start,end,file_offset,permissions=match
            # Recorded executable mapping uses ELF PT_LOAD file_offset=vaddr=0x28000.
            assert permissions=='r-xp' and file_offset==0x28000
            elf=pc-start+file_offset;assert 0x28000<=elf<0x18a8fd
            rows.append({'variant':label,'absolute_pc':hex(pc),'elf_vaddr':hex(elf),'weighted_leaf_samples':count})
    argv=['addr2line','-a','-f','-C','-i','-e',str(debug),*[r['elf_vaddr'] for r in rows]];commands.append(argv)
    completed=subprocess.run(argv,capture_output=True,text=True,check=True)
    (out/(label+'-addr2line.stdout')).write_text(completed.stdout)
    (out/(label+'-addr2line.stderr')).write_text(completed.stderr)
    blocks={};current=None
    for line in completed.stdout.splitlines():
        if re.fullmatch(r'0x[0-9a-fA-F]+',line):current=int(line,16);blocks[current]=[]
        else:assert current is not None;blocks[current].append(line)
    functions=collections.Counter();resolved=0
    for row in rows:
        lines=blocks[int(row['elf_vaddr'],16)];assert len(lines)%2==0
        frames=[{'function':lines[i],'location':lines[i+1]} for i in range(0,len(lines),2)]
        row['frames']=frames;row['resolved']=bool(frames and frames[0]['function']!='??' and not frames[0]['location'].startswith('??'))
        if row['resolved']:resolved+=row['weighted_leaf_samples'];functions[frames[0]['function']]+=row['weighted_leaf_samples']
    old=json.loads((args.profiles/'steady-symbolization.json').read_text())[label+'-cpu'];total=sum(leaves.values());assert total==old['total_samples']
    assert sum(r['weighted_leaf_samples'] for r in rows)==(296 if label=='baseline' else 261)
    summaries[label]={'profile_sha256':hashlib.sha256(raw).hexdigest(),'total_weighted_samples':total,
                     'original_unresolved_leaf_samples':old['unresolved_leaf_samples'],'mapped_libc_leaf_samples':sum(r['weighted_leaf_samples'] for r in rows),
                     'exact_debug_resolved_leaf_samples':resolved,'remaining_unresolved_leaf_samples':old['unresolved_leaf_samples']-resolved,
                     'remaining_unresolved_fraction':(old['unresolved_leaf_samples']-resolved)/total,
                     'resolved_libc_functions':[{'function':name,'leaf_samples':count,'fraction_all_samples':count/total} for name,count in functions.most_common()]}
    all_rows.extend(rows)
(out/'address-resolution.json').write_text(json.dumps(all_rows,indent=2)+'\n')
(out/'summary.json').write_text(json.dumps({'debug_identity':identity,'profiles':summaries,'scope':'Existing steady CPU leaves only. Unknowns outside exact matching libc remain unknown; no product rerun, overhead or causal speed claim. Inline frame chains retained; no nearest-export guess.'},indent=2)+'\n')
(out/'symbolization-commands.json').write_text(json.dumps(commands,indent=2)+'\n')
(out/'debug-readelf-stderr.txt').write_text(result.stderr)
with (out/'resolved-libc-functions.csv').open('w') as f:
    w=csv.writer(f,lineterminator='\n');w.writerow(['variant','function','weighted_leaf_samples','fraction_all_samples'])
    for variant,summary in summaries.items():
        for row in summary['resolved_libc_functions']:w.writerow([variant,row['function'],row['leaf_samples'],row['fraction_all_samples']])
print(json.dumps(summaries,indent=2))
