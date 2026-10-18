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
    }
    points = [*value['lifecycle'], *({'process':op[side]}
              for op in value.get('operation_counters',[]) for side in ('before','after'))]
    memory = [summary.counters(point) for point in points]
    for counter, field in [('rss_kib','max_sampled_rss_mib'),('pss_kib','max_sampled_pss_mib'),
                           ('anon_pss_kib','max_sampled_anon_pss_mib')]:
        result[field] = summary.scaled(max((p[counter] for p in memory if p[counter] is not None),default=None),1024)
    result['lifecycle_ms'] = (phases['after_cleanup']['elapsed_us']-phases['before_initialization']['elapsed_us'])/1000
    if value.get('cold',False):
        operations = value['operation_counters']
        if len(operations) != value['measured']:
            raise ValueError('cold operation counters incomplete')
        # Preserve whole-phase counters, including reopen and sampling overhead.
        result['append_phase_with_reopens_cpu_ms_per_append'] = result['append_cpu_ms_per_append']
        result['append_phase_with_reopens_write_mib_per_append'] = result['append_write_mib_per_append']
        for key,field,divisor in [('cpu_ms','append_cpu_ms_per_append',value['measured']),
                                  ('write_bytes','append_write_mib_per_append',value['measured']*2**20)]:
            deltas = [summary.delta({'process':op['before']},{'process':op['after']},key) for op in operations]
            result[field] = sum(deltas)/divisor if all(x is not None for x in deltas) else None
    for phase in ('before_close','closed_footprint'):
        for kind,suffix in (('db','.sqlite'),('wal','-wal'),('shm','-shm')):
            for size in ('logical','allocated'):
                result[f'{phase}_{kind}_{size}'] = sum(f[size] for f in value[phase] if f['path'].endswith(suffix))
    return result


def summarize(directories, append=False):
    groups = {}
    seen = set()
    identities = set()
    append_modes = set()
    for directory in directories:
        manifest = directory / 'manifest.json'
        if manifest.exists():
            value = json.loads(manifest.read_text())
            if 'executables' in value:
                identities.add(tuple(value['executables'][side]['sha256'] for side in ('before','after')))
        before_paths = {path.stem.removesuffix('-before'):path for path in directory.glob('*-before.json')}
        after_paths = {path.stem.removesuffix('-after'):path for path in directory.glob('*-after.json')}
        if before_paths.keys() != after_paths.keys():
            raise ValueError(f'unmatched before/after samples in {directory}')
        for stem,before in sorted(before_paths.items()):
            pair,case = stem.split('-',1)
            identity = (int(pair),case)
            if identity in seen:
                raise ValueError(f'duplicate pair {identity}')
            seen.add(identity)
            records = {}
            workload = []
            for name,path in (('before',before),('after',after_paths[stem])):
                status = json.loads(path.with_name(path.stem+'.process.json').read_text())
                assert status['accepted'], f'failed sample retained: {path}'
                raw = json.loads(path.read_text())
                workload.append((raw['history'], raw['measured'], raw.get('cold',False) if append else False))
                if append:
                    cold = raw.get('cold',False)
                    if type(cold) is not bool:
                        raise ValueError('append mode must be boolean')
                    append_modes.add(cold)
                    if len(append_modes) > 1:
                        raise ValueError('mixed warm/cold append scopes across samples')
                    assert raw['history'] == int(case), f'history differs from pair label: {path}'
                else:
                    assert raw['case'] == case, f'case differs from pair label: {path}'
                records[name] = append_sample(path) if append else summary.sample(path)
            if workload[0] != workload[1]:
                raise ValueError(f'workload differs within pair {identity}')
            ratios = {key:records['after'][key]/value for key,value in records['before'].items()
                      if isinstance(value,(int,float)) and value != 0 and isinstance(records['after'][key],(int,float))}
            groups.setdefault(case,[]).append({'pair':int(pair),**records,'after_over_before':ratios})
    if not groups:
        raise ValueError('no paired samples')
    if len(identities) > 1:
        raise ValueError('executable identities differ between paired directories')
    medians = {}
    for case,records in groups.items():
        medians[case] = {}
        for side in ('before','after','after_over_before'):
            keys = set().union(*(record[side] for record in records))
            medians[case][side] = {}
            for key in sorted(keys):
                values = [record[side].get(key) for record in records]
                if all(value is None or isinstance(value,(int,float)) for value in values):
                    medians[case][side][key] = statistics.median(values) if all(value is not None for value in values) else None
    append_mode = ('cold' if True in append_modes else 'warm') if append else None
    return {'append_mode':append_mode,
            'append_counter_scope':('per-operation brackets, including counter-sampling overhead; reopen excluded; whole phase with reopens retained separately' if append_mode=='cold' else 'whole append phase after seed, including driver overhead' if append else None),
            'memory_scope':'all observed lifecycle and operation counters; sampled maxima, not peaks',
            'scope':'See baseline.md: CPU self plus driver; process-attributed writes, not device writes; sampled memory, not peak; same-pair ratios; no unrelated median subtraction',
            'samples':groups,'medians':medians}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('directory',type=Path,nargs='+')
    p.add_argument('--append',action='store_true')
    a = p.parse_args()
    print(json.dumps(summarize(a.directory,a.append),indent=2))


if __name__ == '__main__':
    main()
