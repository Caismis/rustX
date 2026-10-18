#!/usr/bin/env python3
"""Summarize sibling-provider samples without combining unrelated medians."""
import argparse
import json
from pathlib import Path
import statistics
import os
from controller import validate_lifecycle


def counters(point):
    p = point['process']
    cpu = None
    if p['stat'] is not None:
        fields = p['stat'].rsplit(')', 1)[1].split()
        cpu = (int(fields[11]) + int(fields[12])) / os.sysconf('SC_CLK_TCK') * 1000
    io = dict(line.split(':', 1) for line in p['io'].splitlines()) if p['io'] else {}
    mem = dict(line.split(':', 1) for line in p['smaps_rollup'].splitlines() if ':' in line) if p['smaps_rollup'] else {}
    return {'cpu_ms':cpu,'write_bytes':int(io['write_bytes']) if 'write_bytes' in io else None,
            'rss_kib':int(mem['Rss'].split()[0]) if 'Rss' in mem else None,
            'pss_kib':int(mem['Pss'].split()[0]) if 'Pss' in mem else None,
            'anon_pss_kib':int(mem['Pss_Anon'].split()[0]) if 'Pss_Anon' in mem else None}


def delta(a,b,key):
    x,y=counters(a)[key],counters(b)[key]
    return None if x is None or y is None else y-x


def scaled(value, divisor):
    return None if value is None else value/divisor


def sample(path):
    s=json.loads(path.read_text())
    lifecycle=validate_lifecycle(s)
    samples=s['samples']
    start=samples[s['history']-1] if s['history'] else lifecycle['after_initialization']
    end=samples[-1]
    initial=lifecycle['before_initialization']
    final=lifecycle['after_cleanup']
    measured=[p for p in samples if p['measured']]
    memory=[counters(p) for p in [*samples,*s['lifecycle']]]
    result={'file':str(path),'mean_turn_ms':statistics.mean(p['settled_us']/1000 for p in measured),
        'turn_cpu_ms_per_turn':scaled(delta(start,end,'cpu_ms'),s['measured']),
        'turn_write_mib_per_turn':scaled(delta(start,end,'write_bytes'),s['measured']*2**20),
        'lifecycle_cpu_ms':delta(initial,final,'cpu_ms'),
        'lifecycle_write_mib':scaled(delta(initial,final,'write_bytes'),2**20),
        'lifecycle_ms':(final['elapsed_us']-initial['elapsed_us'])/1000,
        'max_sampled_rss_mib':scaled(max((p['rss_kib'] for p in memory if p['rss_kib'] is not None),default=None),1024),
        'max_sampled_pss_mib':scaled(max((p['pss_kib'] for p in memory if p['pss_kib'] is not None),default=None),1024),
        'max_sampled_anon_pss_mib':scaled(max((p['anon_pss_kib'] for p in memory if p['anon_pss_kib'] is not None),default=None),1024),
        'phases':[]}
    previous=initial
    for current in s['lifecycle'][1:]:
        result['phases'].append({'through':current['phase'],'wall_ms':(current['elapsed_us']-previous['elapsed_us'])/1000,
                                'cpu_ms':delta(previous,current,'cpu_ms'),'write_bytes':delta(previous,current,'write_bytes')})
        previous=current
    return result


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('directory',type=Path);args=p.parse_args()
    groups={}
    for path in sorted(args.directory.glob('*.process.json')):
        r=json.loads(path.read_text())
        if not r['accepted']:
            groups.setdefault('failed',[]).append(str(path));continue
        raw=path.with_name(path.name.replace('.process.json','.json'))
        engine=raw.stem.rsplit('-',1)[1]
        groups.setdefault(r['case']+'/'+engine,[]).append(sample(raw))
    print(json.dumps({'clock_ticks':os.sysconf('SC_CLK_TCK'),'scope':'CPU self user+system ticks; process-attributed writes (not device writes); runtime plus driver; verification phases explicit; evidence output after cleanup excluded; maximum sampled RSS/PSS, not peak; allocation count unavailable','groups':groups},indent=2))

if __name__=='__main__':main()
