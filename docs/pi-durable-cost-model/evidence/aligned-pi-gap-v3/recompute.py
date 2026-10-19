#!/usr/bin/env python3
"""Check archived bytes, raw counters and paired arithmetic without producer imports."""
import argparse
import csv
import hashlib
import json
import math
from pathlib import Path
import statistics
import tarfile

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--archive', type=Path, required=True)
p.add_argument('--readable', type=Path, required=True)
a = p.parse_args()
prefix = 'aligned-pi-gap-v3'
rows = list(csv.DictReader((a.readable/'paired-costs.csv').open()))
intervals = json.loads((a.readable/'paired-uncertainty.json').read_text())
cases = ['short-small', 'short-large', 'stream', 'large', 'tool', 'cancel']
raw, accepted, comparison = {}, {}, []


def cpu(point):
    fields = point['process']['stat'].rsplit(')', 1)[1].split()
    return (int(fields[11]) + int(fields[12])) * 10


def derive(data):
    assert data['oracle'] == 'passed' and data['workload_version'] == 3
    assert data['model_time'] == '2026-10-19 00:00:00'
    history = data['history']
    assert len(data['samples']) == history + data['measured']
    life = {row['phase']: row for row in data['lifecycle']}
    final = life['after_cleanup']
    start = data['samples'][history-1] if history else life['after_initialization']
    measured = [r for r in data['samples'] if r['measured']]
    assert len(measured) == data['measured']
    writes = dict(r.split(':', 1) for r in final['process']['io'].splitlines())
    memories = [dict(r.split(':', 1) for r in point['process']['smaps_rollup'].splitlines() if ':' in r)
                for point in [*data['lifecycle'], *data['samples']]]
    if 'measurement_version' in data:
        assert data['live_requests'] == data['reopened_requests']
        assert data['verified_live_requests'] == data['verified_reopened_requests']
    else:
        assert [r['messages'] for r in data['captures']] == data['reopenedRequests']
    return dict(mean_turn_ms=statistics.mean(r['settled_us']/1000 for r in measured),
                turn_cpu_ms_per_turn=(cpu(data['samples'][-1])-cpu(start))/data['measured'],
                through_cleanup_self_cpu_ms=cpu(final),
                through_cleanup_process_write_mib=int(writes['write_bytes'])/2**20,
                max_sampled_anon_pss_mib=max(int(m['Pss_Anon'].split()[0]) for m in memories)/1024)


with tarfile.open(a.archive, 'r:xz') as archive:
    manifest = json.load(archive.extractfile('MANIFEST.json'))
checked = 0
with tarfile.open(a.archive, 'r|xz') as archive:
    for member in archive:
        if member.name == 'MANIFEST.json':
            continue
        blob = archive.extractfile(member).read()
        expected = manifest[member.name]
        assert len(blob) == expected['bytes']
        assert hashlib.sha256(blob).hexdigest() == expected['sha256'], member.name
        checked += 1
        parts = member.name.split('/')
        if len(parts) != 3 or parts[1] not in ['no-live-subscriber', 'live-subscriber']:
            continue
        filename = parts[-1]
        if filename.endswith('.process.json'):
            value = json.loads(blob)
            assert value['product_exit'] == value['provider_exit'] == 0 and value['provider_report']['ok']
            accepted[member.name.removesuffix('.process.json')] = value['accepted']
        elif filename.endswith('-comparison.json'):
            value = json.loads(blob)
            assert all(r['coreSemanticEqual'] and not r['fullSemanticRequestEqual'] for r in value['checked'])
            assert all(len(r['rustToolNames']) == 7 and len(r['piToolNames']) == 4 for r in value['checked'])
            comparison.extend(value['checked'])
        elif filename.endswith('-rust.json') or filename.endswith('-pi.json'):
            raw[member.name.removesuffix('.json')] = derive(json.loads(blob))
assert checked == len(manifest) and len(raw) == len(accepted) == 72
assert all(accepted[name] for name in raw) and len(comparison) == 846

close = lambda x, y: math.isclose(x, y, rel_tol=1e-12, abs_tol=1e-12)
groups = {}
for row in rows:
    base = f"{prefix}/{row['mode']}/{row['pair']}-{row['case']}"
    r, pi = (raw[base+'-'+engine][row['metric']] for engine in ['rust', 'pi'])
    assert close(float(row['rust']), r) and close(float(row['pi']), pi)
    ratio = r/pi if r > 0 and pi > 0 else None
    if ratio is None:
        assert row['ratio'] == ''
    else:
        assert close(float(row['ratio']), ratio)
    groups.setdefault((row['mode'], row['case'], row['metric']), []).append(ratio)
for record in intervals:
    values = groups[(record['mode'], record['case'], record['metric'])]
    assert record['ratios'] == values and len(values) == record['n'] == 3
    if any(v is None for v in values):
        assert record['state'].startswith('indeterminate')
        continue
    logs = list(map(math.log, values))
    mean = statistics.mean(logs)
    width = 2.919985580355516 * statistics.stdev(logs) / math.sqrt(3)
    expected = dict(median=statistics.median(values), geometric_mean=math.exp(mean),
                    lower90=math.exp(mean-width), upper90_one_sided95=math.exp(mean+width))
    assert all(close(record[k], v) for k, v in expected.items())
assert len(rows) == 180 and len(groups) == len(intervals) == 60
print(json.dumps(dict(verified=True, archive_files=checked, accepted_products=72,
                      core_request_pairs=len(comparison), paired_metric_rows=len(rows), intervals=len(intervals))))
