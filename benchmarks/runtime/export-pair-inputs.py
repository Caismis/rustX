#!/usr/bin/env python3
"""Expose compact reviewable per-pair inputs, verified against retained manifests."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import statistics

spec=importlib.util.spec_from_file_location('summary',Path(__file__).with_name('summarize-http.py'))
summary=importlib.util.module_from_spec(spec);spec.loader.exec_module(summary)
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--root',type=Path,required=True);p.add_argument('--output',type=Path,required=True)
a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True)
assert not any(a.output.iterdir()), 'refuse to overwrite existing summaries'
archives=Path('docs/issue-481/evidence')
manifest_files=['baseline-manifest.json','source-manifest.json','combined.tar.manifest.json','reservations.tar.manifest.json']
manifests={}
for file in manifest_files:
    for path,metadata in json.loads((archives/file).read_text()).items():
        manifests.setdefault(path,set()).add(metadata['sha256'])
stages={
 'reservations':(['reservations-paired'],('before','after')),
 'source':(['source-paired'],('before','after')),
 'canonical-runtime':(['canonical-runtime-paired','canonical-runtime-confirmation'],('before','after')),
 'combined':(['combined-paired','combined-confirmation'],('before','after')),
 'canonical-append':(['canonical-append-paired','canonical-append-confirmation'],('before','after')),
 'baseline-pi':(['baseline-paired'],('rust','pi')),
 'combined-pi':(['combined-pi-paired'],('rust','pi')),
}

def load_verified(path):
    relative=str(path.relative_to(a.root));digest=hashlib.file_digest(path.open('rb'),'sha256').hexdigest()
    assert relative in manifests and digest in manifests[relative],relative
    return json.loads(path.read_text()),{'path':relative,'sha256':digest}

def inputs(path,append):
    s,identity=load_verified(path)
    status,status_identity=load_verified(path.with_name(path.stem+'.process.json'))
    assert status['accepted']
    lifecycle={p['phase']:p for p in s['lifecycle']}
    points=s['lifecycle'] if append else [*s['samples'],*s['lifecycle']]
    start=lifecycle['after_seed'] if append else s['samples'][s['history']-1] if s['history'] else lifecycle['after_initialization']
    end=lifecycle['after_appends'] if append else s['samples'][-1]
    def raw(point):
        process=point['process'];fields=process['stat'].rsplit(')',1)[1].split()
        io=dict(line.split(':',1) for line in process['io'].splitlines())
        memory=dict(line.split(':',1) for line in process['smaps_rollup'].splitlines() if ':' in line)
        return {'self_cpu_ticks':int(fields[11])+int(fields[12]),'write_bytes':int(io['write_bytes']),
                **{key:int(memory[key].split()[0]) for key in ('Rss','Pss','Pss_Anon','Pss_File')}}
    begin,end_counter=raw(start),raw(end)
    initial,final=raw(lifecycle['before_initialization']),raw(lifecycle['after_cleanup'])
    values=[raw(p) for p in points]
    elapsed=s['append_ns'] if append else [p['settled_us'] for p in s['samples'] if p['measured']]
    divisor=1e6 if append else 1000
    return {'input':identity,'status':status_identity,'history':s['history'],'measured':s['measured'],
            'elapsed_unit':'ns' if append else 'us','measured_elapsed':elapsed,
            'measured_start':begin,'measured_end':end_counter,'lifecycle_start':initial,'lifecycle_end':final,
            'max_sampled_rss_kib':max(p['Rss'] for p in values),'max_sampled_pss_kib':max(p['Pss'] for p in values),
            'derived':{'mean_ms':statistics.mean(elapsed)/divisor,
                       'cpu_ms_per_operation':(end_counter['self_cpu_ticks']-begin['self_cpu_ticks'])*10/s['measured'],
                       'measured_write_bytes_per_operation':(end_counter['write_bytes']-begin['write_bytes'])/s['measured'],
                       'lifecycle_write_bytes':final['write_bytes']-initial['write_bytes']}}

index={'clock_ticks_per_second':100,'scope':'Exactly the retained cloud samples; self CPU only. Per-operation means exclude seeds; lifecycle includes initialization through cleanup. Max sampled memory is not peak. Input/status SHA-256 verified against archive manifests. Pair ratios must be computed per pair before taking their median. Native rustX/Pi tool schemas and contracts differ; no full-request parity claim.','manifests':{file:hashlib.file_digest((archives/file).open('rb'),'sha256').hexdigest() for file in manifest_files},'files':{}}
for stage,(directories,sides) in stages.items():
    cases={}
    for directory in directories:
        for first in sorted((a.root/directory).glob('*-'+sides[0]+'.json')):
            stem=first.stem.removesuffix('-'+sides[0]);pair,case=stem.split('-',1)
            record={'pair':int(pair)}
            for side in sides:record[side]=inputs(first.with_name(stem+'-'+side+'.json'),stage=='canonical-append')
            cases.setdefault(case,[]).append(record)
    for case,records in cases.items():
        records.sort(key=lambda r:r['pair']);assert len({r['pair'] for r in records})==len(records)
        file=f'{stage}-{case}.json'
        value={'stage':stage,'case':case,'pairs':records,'clock_ticks_per_second':100}
        # One pair per line keeps text-only review bounded without duplicated wire/history JSON.
        encoded='{"stage":'+json.dumps(stage)+',"case":'+json.dumps(case)+',"clock_ticks_per_second":100,"pairs":[\n'+',\n'.join(json.dumps(record,separators=(',',':')) for record in records)+'\n]}\n'
        (a.output/file).write_text(encoded)
        index['files'][file]={'pairs':len(records),'sha256':hashlib.sha256(encoded.encode()).hexdigest(),'bytes':len(encoded.encode())}
(a.output/'index.json').write_text(json.dumps(index,indent=2))
print(len(index['files']),'files',sum(v['bytes'] for v in index['files'].values()),'bytes; all raw input hashes verified')
