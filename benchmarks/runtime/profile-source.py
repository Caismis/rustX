#!/usr/bin/env python3
"""Attribute completed strace operations to ephemeral Source bytes/indexes."""
import collections,json,re,sys
from pathlib import Path
pending={};counts=collections.Counter();writes=collections.Counter();paths={};unresolved=0
for raw in Path(sys.argv[1]).open():
 m=re.match(r'^(\d+)\s+(.*)',raw)
 if not m:continue
 pid,line=m.groups()
 if '<unfinished ...>' in line:
  if pid in pending:unresolved+=1
  pending[pid]=line.split('<unfinished ...>')[0];continue
 if re.match(r'<\.\.\. \w+ resumed>',line):
  if pid not in pending:unresolved+=1;continue
  line=pending.pop(pid)+re.sub(r'^<\.\.\. \w+ resumed>','',line)
 result=re.search(r' = (-?\d+)',line)
 if not result:continue
 n=int(result[1]);name=line.split('(',1)[0]
 if n<0:counts['failed/'+name]+=1;continue
 descriptor=re.search(r'^\w+\(\d+<([^>]+)>',line)
 path=descriptor[1] if descriptor else ''
 if name=='openat':
  opened=re.search(r' = \d+<([^>]+)>',line);path=opened[1] if opened else ''
 category='source-index' if path.endswith('/index.sqlite') else 'source-bytes' if path.endswith('/bytes') else 'wal' if path.endswith('-wal') else 'database' if path.endswith('.sqlite') else 'other'
 counts[category+'/'+name]+=1
 if name in ('write','pwrite64'):
  writes[category]+=n
  if category.startswith('source-'):paths.setdefault(path,0);paths[path]+=n
sizes=[v for k,v in paths.items() if k.endswith('/bytes')]
print(json.dumps({'completed_calls':dict(counts),'successful_write_bytes':dict(writes),'source_bytes_files_written':len(sizes),'source_bytes_files_at_most_4096':sum(v<=4096 for v in sizes),'source_index_files_written':sum(k.endswith('/index.sqlite') for k in paths),'unresolved':unresolved+len(pending),'limitations':'Syscall return bytes, not physical device writes. Source index openat counts approximate connection work; not SQL statement counts or CPU attribution. Profile is separate from formal timing.'},indent=2))
