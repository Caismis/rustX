#!/usr/bin/env python3
"""Independently recover paired costs from original counters and verify all intervals."""
import csv
import hashlib
import json
import math
from pathlib import Path
import re
import statistics
import tarfile

HERE = Path(__file__).resolve().parent
ARCHIVE = HERE.parents[1] / 'observer-demand-results.tar.xz'
T_DF2 = 2.919985580355516

def close(actual, expected):
    assert math.isclose(actual, expected, rel_tol=1e-12, abs_tol=1e-12), (actual, expected)

def counters(point, ticks):
    process = point['process']
    fields = process['stat'].rsplit(')', 1)[1].split()
    cpu_ms = (int(fields[11]) + int(fields[12])) * 1000 / ticks
    io = dict(line.split(':', 1) for line in process['io'].splitlines())
    memory = dict(line.split(':', 1) for line in process['smaps_rollup'].splitlines() if ':' in line)
    return cpu_ms, int(io['write_bytes']) / 2**20, int(memory['Pss_Anon'].split()[0]) / 1024

with tarfile.open(ARCHIVE, 'r:xz') as archive:
    manifest = json.load(archive.extractfile('MANIFEST.json'))
# One sequential archive pass; do not repeatedly inflate the large original samples.
raw = {}
manifest_files = 0
with tarfile.open(ARCHIVE, 'r|xz') as archive:
    for member in archive:
        if member.name == 'MANIFEST.json':
            continue
        data = archive.extractfile(member).read()
        expected = manifest[member.name]
        assert len(data) == expected['bytes']
        assert hashlib.sha256(data).hexdigest() == expected['sha256']
        manifest_files += 1
        match = re.fullmatch(r'(no-live-subscriber|live-subscriber)/(\d+)-(.+)-(before|after)\.json', member.name)
        if match:
            mode, pair, case, side = match.groups()
            value = json.loads(data)
            assert value['oracle'] == 'passed' and value['observer_mode'] == mode
            assert all(point['observer_mode'] == mode for point in value['samples'])
            phases = {point['phase']: point for point in value['lifecycle']}
            # Both recorded manifests state 100 Hz; the raw stat counters are Linux ticks.
            ticks = 100
            first = value['samples'][value['history'] - 1] if value['history'] else phases['after_initialization']
            end = counters(phases['after_cleanup'], ticks)
            raw[(mode, case, pair, side)] = {
                'mean_turn_ms': statistics.mean(point['settled_us'] / 1000 for point in value['samples'] if point['measured']),
                'turn_cpu_ms_per_turn': (counters(value['samples'][-1], ticks)[0] - counters(first, ticks)[0]) / value['measured'],
                'through_cleanup_self_cpu_ms': end[0],
                'through_cleanup_process_write_mib': end[1],
                'max_sampled_anon_pss_mib': max(counters(point, ticks)[2] for point in [*value['samples'], *value['lifecycle']]),
            }
        elif member.name.endswith('/manifest.json'):
            metadata = json.loads(data)
            assert metadata['clock_ticks'] == 100
        elif member.name.endswith('.process.json'):
            assert json.loads(data)['accepted']
        elif member.name.endswith('-comparison.json'):
            assert all(row['fullRequestEqual'] for row in json.loads(data)['checked'])
assert manifest_files == len(manifest) and len(raw) == 72

ratios = {}
rows = 0
with (HERE / 'paired-costs.csv').open() as source:
    for row in csv.DictReader(source):
        identity = (row['observer_mode'], row['case'], row['pair'])
        metric = row['metric']
        before, after, ratio = (float(row[key]) for key in ('before', 'after', 'ratio'))
        close(before, raw[(*identity, 'before')][metric])
        close(after, raw[(*identity, 'after')][metric])
        close(ratio, after / before)
        ratios.setdefault((identity[0], identity[1], metric), []).append(ratio)
        rows += 1

expected = json.loads((HERE / 'paired-uncertainty.json').read_text())
for (mode, case, metric), values in ratios.items():
    assert len(values) == 3
    record = expected['modes'][mode][case][metric]
    assert record['n'] == 3 and record['ratios'] == values
    logs = list(map(math.log, values))
    mean = statistics.mean(logs)
    width = T_DF2 * statistics.stdev(logs) / math.sqrt(3)
    for key, value in {
        'geometric_mean': math.exp(mean), 'two_sided_90_lower': math.exp(mean - width),
        'one_sided_95_upper': math.exp(mean + width), 'paired_median': statistics.median(values),
        'min': min(values), 'max': max(values),
    }.items():
        close(value, record[key])
assert rows == 180 and len(ratios) == 60
print(json.dumps({'verified_archive_files': manifest_files, 'raw_samples': len(raw),
                  'raw_counter_values': 2 * rows, 'ratios': rows, 'intervals': len(ratios),
                  'all_match': True}))
