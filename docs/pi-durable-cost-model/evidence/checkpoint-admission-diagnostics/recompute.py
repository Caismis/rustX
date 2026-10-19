#!/usr/bin/env python3
"""Verify archived diagnostic bytes and recompute counts/costs; no producer imports."""
import hashlib,json,math,statistics,tarfile
from pathlib import Path
here=Path(__file__).resolve().parent
with tarfile.open(here.parent/'checkpoint-admission-diagnostics.tar.xz','r:xz') as tar:
    manifest=json.load(tar.extractfile('MANIFEST.json'))
    blobs={}
    for name,entry in manifest.items():
        data=tar.extractfile(name).read()
        assert len(data)==entry['bytes'] and hashlib.sha256(data).hexdigest()==entry['sha256'],name
        if name.startswith('checkpoint-admission-diagnostics/') and name.endswith('.json'):
            blobs[Path(name).name]=json.loads(data)
ticks=blobs['provenance.json']['clock_ticks']
assert type(ticks) is int and ticks==100
def cpu(point):
    fields=point['process']['stat'].rsplit(')',1)[1].split()
    return (int(fields[11])+int(fields[12]))*1000/ticks
def metrics(value):
    assert value['oracle']=='passed' and value['workload_version']==3
    assert value['model_time']=='2026-10-19 00:00:00'
    life={row['phase']:row for row in value['lifecycle']}
    final=life['after_cleanup'];history=value['history']
    first=value['samples'][history-1] if history else life['after_initialization']
    io=dict(row.split(':',1) for row in final['process']['io'].splitlines())
    memories=[dict(row.split(':',1) for row in p['process']['smaps_rollup'].splitlines() if ':' in row) for p in [*value['samples'],*value['lifecycle']]]
    return dict(mean_turn_ms=statistics.mean(row['settled_us']/1000 for row in value['samples'] if row['measured']),
        turn_cpu_ms_per_turn=(cpu(value['samples'][-1])-cpu(first))/value['measured'],
        through_cleanup_self_cpu_ms=cpu(final),through_cleanup_process_write_mib=int(io['write_bytes'])/2**20,
        max_sampled_anon_pss_mib=max(int(row['Pss_Anon'].split()[0]) for row in memories)/1024)
counts=[];requests=0
controls=blobs['overhead-controls.json']['pairs']
for row in controls:
    stem=f"{row['mode']}-{row['case']}"
    actual={}
    for side in ['control','diagnostic']:
        raw=blobs[f'{stem}-{side}.json'];actual[side]=metrics(raw)
        assert actual[side]==row[side]
        status=blobs[f'{stem}-{side}.process.json']
        assert status['accepted'] and status['product_exit']==status['provider_exit']==0
    ratios={key:actual['diagnostic'][key]/value for key,value in actual['control'].items()}
    assert ratios==row['diagnostic_over_control']
    decision=blobs[f'{stem}-diagnostic.json']['checkpoint_admission_diagnostics']
    counts.append(dict(mode=row['mode'],case=row['case'],**decision))
    checked=blobs[f'{stem}-comparison.json']['checked'];requests+=len(checked)
    assert all(r['fullSemanticRequestEqual'] and r['coreSemanticEqual'] for r in checked)
assert counts==blobs['proof-counts.json']==json.loads((here/'proof-counts.json').read_text())
assert sum(row['proved'] for row in counts)==3135 and sum(row['deferred'] for row in counts)==0
assert len(controls)==12 and requests==282
for filename in ['predicate-micro.json','predicate-micro-quiet.json']:
    micro=blobs[filename];groups={}
    assert len(micro['samples'])==48 and sum(row['operations'] for row in micro['samples'])==12288
    for row in micro['samples']:
        assert row['operations']==256
        key=(row['wide'],row['case'],row['pair'])
        groups.setdefault(key,{})[row['legacy']]=row
    ratios={}
    for (wide,case,pair),sides in groups.items():
        assert sides[True]['admitted']==sides[False]['admitted']
        ratios.setdefault((wide,case),[]).append(sides[False]['elapsed_ns']/sides[True]['elapsed_ns'])
    if filename.endswith('-quiet.json'):
        result=[dict(wide=wide,case=case,ratios=values,median=statistics.median(values)) for (wide,case),values in ratios.items()]
        assert result==blobs['micro-quiet-ratios.json']==json.loads((here/'micro-quiet-ratios.json').read_text())
assert controls==json.loads((here/'overhead-controls.json').read_text())['pairs']
print(json.dumps(dict(archive_files=len(manifest),products=24,retained_semantic_comparison_pairs=requests,proved=3135,deferred=0,micro_runs=2,micro_batches=96,verified=True,semantic_oracle_independently_rerun=False)))
