#!/usr/bin/env python3
"""Per-pair revision comparisons; preserve every sample and phase scope."""
import argparse
import importlib.util
import json
from pathlib import Path
import statistics

spec = importlib.util.spec_from_file_location('http_summary', Path(__file__).with_name('summarize-http.py'))
summary = importlib.util.module_from_spec(spec)
spec.loader.exec_module(summary)


def append_sample(path):
    value = json.loads(path.read_text())
    phases = {point['phase']:point for point in value['lifecycle']}
    result = {
        'mean_append_ms':statistics.mean(value['append_ns'])/1e6,
        'append_cpu_ms_per_append':summary.scaled(summary.delta(phases['after_seed'],phases['after_appends'],'cpu_ms'),value['measured']),
        'append_write_mib_per_append':summary.scaled(summary.delta(phases['after_seed'],phases['after_appends'],'write_bytes'),value['measured']*2**20),
        'lifecycle_cpu_ms':summary.delta(phases['before_initialization'],phases['after_cleanup'],'cpu_ms'),
        'lifecycle_write_mib':summary.scaled(summary.delta(phases['before_initialization'],phases['after_cleanup'],'write_bytes'),2**20),
        'max_sampled_rss_mib':max(summary.counters(p)['rss_kib'] for p in value['lifecycle'])/1024,
        'max_sampled_pss_mib':max(summary.counters(p)['pss_kib'] for p in value['lifecycle'])/1024,
    }
    for phase in ('before_close','closed_footprint'):
        for kind,suffix in (('db','.sqlite'),('wal','-wal'),('shm','-shm')):
            for size in ('logical','allocated'):
                result[f'{phase}_{kind}_{size}'] = sum(f[size] for f in value[phase] if f['path'].endswith(suffix))
    return result


p = argparse.ArgumentParser(description=__doc__)
p.add_argument('directory',type=Path,nargs='+')
p.add_argument('--append',action='store_true')
a = p.parse_args()
groups = {}
for before in sorted(path for directory in a.directory for path in directory.glob('*-before.json')):
    stem = before.stem.removesuffix('-before')
    pair,case = stem.split('-',1)
    after = before.with_name(stem+'-after.json')
    records = {}
    for name,path in (('before',before),('after',after)):
        status = json.loads(path.with_name(path.stem+'.process.json').read_text())
        assert status['accepted'], f'failed sample retained: {path}'
        records[name] = append_sample(path) if a.append else summary.sample(path)
    ratios = {key:records['after'][key]/value for key,value in records['before'].items()
              if isinstance(value,(int,float)) and value != 0 and isinstance(records['after'][key],(int,float))}
    groups.setdefault(case,[]).append({'pair':int(pair),**records,'after_over_before':ratios})
medians = {}
for case,records in groups.items():
    medians[case] = {}
    for side in ('before','after','after_over_before'):
        medians[case][side] = {key:statistics.median(r[side][key] for r in records)
            for key,value in records[0][side].items() if isinstance(value,(int,float))}
print(json.dumps({'scope':'See baseline.md: CPU self plus driver; process-attributed writes, not device writes; sampled memory, not peak; same-pair ratios; no unrelated median subtraction',
                  'samples':groups,'medians':medians},indent=2))
