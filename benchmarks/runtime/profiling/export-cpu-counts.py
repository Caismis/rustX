#!/usr/bin/env python3
"""Expose CPU numerators/denominators, independently checking the raw profile count."""
import argparse,hashlib,json,re,struct
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('root',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
records=[]
for case in ['small','large']:
    directory=a.root/f'attr-cpu-{case}';raw=directory/'before_measured_turns.cpu';report=directory/'steady-cumulative.txt'
    data=raw.read_bytes();header=struct.unpack_from('<5Q',data)
    assert header[0:3]==(0,3,0),'unsupported CPU profile layout'
    offset=40;total=0
    while True:
        count,depth=struct.unpack_from('<2Q',data,offset);offset+=16
        assert 0<depth<=1024,'invalid stack depth'
        pcs=struct.unpack_from(f'<{depth}Q',data,offset);offset+=8*depth
        if count==0 and pcs==(0,):break
        total+=count
    lines=report.read_text().splitlines();reported=int(re.fullmatch(r'Total: (\d+) samples',lines[0])[1]);assert total==reported
    rows=[]
    for line in lines[1:]:
        m=re.match(r'\s*(\d+)\s+[\d.]+%\s+[\d.]+%\s+(\d+)\s+[\d.]+%\s+(.*)',line)
        if m:rows.append({'symbol':m[3],'flat_samples':int(m[1]),'cumulative_samples':int(m[2])})
    selected=rows[:40]+[row for row in rows[40:] if any(name in row['symbol'] for name in ('read_layout::','process_sample','footprint','verify_requests'))]
    digest=lambda path:hashlib.file_digest(path.open('rb'),'sha256').hexdigest()
    manifest=json.loads((directory/'manifest.json').read_text())
    records.append({'case':manifest['case'],'phase':'before_measured_turns -> after_turns','measured_turns':32,'binary_sha256':manifest['binary_sha256'],'denominator_samples':total,'requested_interval_us':header[3], 'raw_profile':{'path':str(raw.relative_to(a.root)),'sha256':digest(raw)},'symbolized_report':{'path':str(report.relative_to(a.root)),'sha256':digest(report),'command':'google-pprof --text --cum --no_strip_temp --nodecount=200 (small), 100 (large) BINARY PROFILE'},'rows':selected})
a.output.write_text(json.dumps({'scope':'Sample counts, not elapsed timers. Cumulative rows overlap and must not be summed. Divide the selected row numerator by denominator_samples to recompute its proportion. Reports may omit low-count functions; denominator independently checked against complete raw profile records.','profiles':records},indent=2))
