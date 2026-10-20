#!/usr/bin/env python3
"""Independent raw-counter arithmetic; no producer summary imports."""
import argparse
import csv
import hashlib
import json
import math
from pathlib import Path
import statistics

ROOT = Path('/workspace/pr485-study')
CASES = ['short-small', 'short-large', 'stream', 'large', 'tool', 'cancel']
PHASES = ['before_initialization', 'after_provider_start', 'after_initialization',
          'after_turns', 'after_late_snapshot', 'after_verification', 'after_shutdown',
          'after_driver_drop', 'after_final_writer_close', 'after_reopen_verification', 'after_cleanup']

def counters(point):
    value = point['process']
    stat = value['stat']
    fields = stat.rsplit(')', 1)[1].split() if stat is not None else None
    io = dict(line.split(':', 1) for line in value['io'].splitlines()) if value['io'] else {}
    memory = dict(line.split(':', 1) for line in value['smaps_rollup'].splitlines() if ':' in line) if value['smaps_rollup'] else {}
    return {'cpu_ms': (int(fields[11]) + int(fields[12])) * 10 if fields else None,
            'write_bytes': int(io['write_bytes']) if 'write_bytes' in io else None,
            **{name: int(memory[field].split()[0]) / 1024 if field in memory else None
               for name, field in [('rss_mib', 'Rss'), ('pss_mib', 'Pss'), ('anon_pss_mib', 'Pss_Anon')]}}

def difference(before, after, field):
    a, b = counters(before)[field], counters(after)[field]
    return None if a is None or b is None else b - a

def divide(value, denominator):
    return None if value is None else value / denominator

def metrics(sample, process):
    points = sample['lifecycle']
    assert [p['phase'] for p in points] == PHASES
    assert [p['elapsed_us'] for p in points] == sorted(p['elapsed_us'] for p in points)
    phases = {p['phase']: p for p in points}
    turns = sample['samples']
    assert [p['turn'] for p in turns] == list(range(len(turns)))
    assert len(turns) == sample['history'] + sample['measured']
    measured = [p for p in turns if p['measured']]
    assert len(measured) == sample['measured']
    start = turns[sample['history'] - 1] if sample['history'] else phases['after_initialization']
    end = turns[-1]
    first, final = points[0], points[-1]
    result = {'turn_wall_ms': statistics.mean(p['settled_us'] for p in measured) / 1000,
        'turn_cpu_ms': divide(difference(start, end, 'cpu_ms'), sample['measured']),
        'turn_write_mib': divide(difference(start, end, 'write_bytes'), sample['measured'] * 2**20),
        'complete_self_cpu_ms_since_exec': counters(final)['cpu_ms'],
        'complete_process_write_mib_since_exec': divide(counters(final)['write_bytes'], 2**20),
        'initial_self_cpu_ms_since_exec': counters(first)['cpu_ms'],
        'initialization_to_cleanup_cpu_ms': difference(first, final, 'cpu_ms'),
        'initialization_to_cleanup_write_mib': divide(difference(first, final, 'write_bytes'), 2**20),
        'initialization_to_cleanup_wall_ms': (final['elapsed_us'] - first['elapsed_us']) / 1000,
        'post_turn_to_cleanup_cpu_ms': difference(phases['after_turns'], final, 'cpu_ms'),
        'post_turn_to_cleanup_write_mib': divide(difference(phases['after_turns'], final, 'write_bytes'), 2**20),
        'post_turn_to_cleanup_wall_ms': (final['elapsed_us'] - phases['after_turns']['elapsed_us']) / 1000}
    memory = [counters(p) for p in [*turns, *points]]
    for field in ['rss_mib', 'pss_mib', 'anon_pss_mib']:
        result['max_sampled_' + field] = max((p[field] for p in memory if p[field] is not None), default=None)
    for before, after in zip(points, points[1:]):
        prefix = 'phase_' + after['phase'] + '_'
        result[prefix + 'wall_ms'] = (after['elapsed_us'] - before['elapsed_us']) / 1000
        result[prefix + 'cpu_ms'] = difference(before, after, 'cpu_ms')
        result[prefix + 'write_mib'] = divide(difference(before, after, 'write_bytes'), 2**20)
    for field in ['product_startup_ns', 'provider_startup_ns', 'through_product_exit_ns', 'provider_teardown_ns', 'complete_lifecycle_ns']:
        result['external_' + field.removesuffix('_ns') + '_ms'] = process[field] / 1e6
    return result

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('directories', nargs='+', type=Path)
    p.add_argument('--output', required=True, type=Path)
    p.add_argument('--build-root', type=Path, default=ROOT / 'evidence',
                   help='directory containing build-baseline/build-geometry/build-scoped identities')
    a = p.parse_args()
    a.output.mkdir(parents=True, exist_ok=False)
    identities = {label: json.loads((a.build_root / ('build-' + label) / 'identity.json').read_text())
                  for label in ['baseline', 'geometry', 'scoped']}
    records, groups, hashes = [], {}, {}
    for directory in a.directories:
        manifest = json.loads((directory / 'manifest.json').read_text())
        assert manifest['clock_ticks'] == 100 and manifest['pairs'] == 3 and manifest['pair_offset'] == 0
        assert manifest['cases'] == CASES
        assert manifest['executables']['before']['sha256'] == identities['baseline']['sha256']
        candidates = [name for name in ['geometry', 'scoped'] if identities[name]['sha256'] == manifest['executables']['after']['sha256']]
        assert len(candidates) == 1
        candidate = candidates[0]
        mode = manifest['observer_mode']
        assert mode in ['no-live-subscriber', 'live-subscriber']
        assert len(list(directory.glob('*.process.json'))) == 36
        for pair in range(3):
            for case in CASES:
                values = {}
                comparison = json.loads((directory / f'{pair}-{case}-comparison.json').read_text())
                assert all(r['coreSemanticEqual'] and r['fullSemanticRequestEqual'] for r in comparison['checked'])
                expected_requests = {'short-small': 40, 'short-large': 96, 'tool': 2}.get(case, 1)
                assert len(comparison['checked']) == expected_requests
                for side in ['before', 'after']:
                    path = directory / f'{pair}-{case}-{side}.json'
                    status_path = directory / f'{pair}-{case}-{side}.process.json'
                    sample = json.loads(path.read_text())
                    process = json.loads(status_path.read_text())
                    assert process['accepted'] and process['product_exit'] == process['provider_exit'] == 0
                    assert sample['oracle'] == 'passed' and sample['measurement_version'] == 4
                    assert sample['workload_version'] == 3 and sample['model_time'] == '2026-10-19 00:00:00'
                    assert sample['observer_mode'] == mode and sample['case'] == case
                    assert sample['wire_requests'] == process['provider_report']['requests']
                    assert len(sample['wire_requests']) == expected_requests
                    assert sample['verified_live_requests'] == sample['verified_reopened_requests'] == expected_requests
                    values[side] = metrics(sample, process)
                    for source in [path, status_path, directory / f'{pair}-{case}-comparison.json', directory / 'manifest.json']:
                        hashes[str(source)] = hashlib.file_digest(source.open('rb'), 'sha256').hexdigest()
                for metric, before in values['before'].items():
                    after = values['after'][metric]
                    ratio = after / before if before is not None and before > 0 and after is not None else None
                    row = {'candidate': candidate, 'observer_mode': mode, 'case': case, 'pair': pair,
                           'metric': metric, 'before': before, 'after': after, 'after_over_before': ratio}
                    records.append(row)
                    groups.setdefault((candidate, mode, case, metric), []).append(row)
    summaries = []
    for key, rows in sorted(groups.items()):
        assert len(rows) == 3
        ratios = [r['after_over_before'] for r in rows]
        valid = all(x is not None for x in ratios)
        positive = valid and all(x > 0 for x in ratios)
        center = low = high = None
        if positive:
            logs = [math.log(x) for x in ratios]
            average = statistics.mean(logs)
            half = 2.919985580355516 * statistics.stdev(logs) / math.sqrt(3)
            center, low, high = math.exp(average), math.exp(average - half), math.exp(average + half)
        summaries.append(dict(zip(['candidate', 'observer_mode', 'case', 'metric'], key)) |
            {'pairs': 3, 'ratio_pairs_available': sum(x is not None for x in ratios),
             'median_before': statistics.median(r['before'] for r in rows) if all(r['before'] is not None for r in rows) else None,
             'median_after': statistics.median(r['after'] for r in rows) if all(r['after'] is not None for r in rows) else None,
             'median_paired_ratio': statistics.median(ratios) if valid else None,
             'minimum_paired_ratio': min(ratios) if valid else None, 'maximum_paired_ratio': max(ratios) if valid else None,
             'geometric_mean_paired_ratio': center, 'log_t_90_lower': low, 'log_t_90_upper': high})
    for name, rows in [('paired-costs.csv', records), ('paired-summary.csv', summaries)]:
        with (a.output / name).open('w') as f:
            writer = csv.DictWriter(f, fieldnames=list(rows[0]))
            writer.writeheader(); writer.writerows(rows)
    (a.output / 'raw-input-sha256.json').write_text(json.dumps(hashes, indent=2) + '\n')
    (a.output / 'method.json').write_text(json.dumps({'clock_ticks': 100,
        'ratio': 'Within-pair after/before. No ratio for a zero or unavailable baseline; zero after values are retained but have no finite log interval.',
        'interval': 'Two-sided 90% log-t, n=3, df=2, critical 2.919985580355516; approximately normal independent log ratios assumed. Center is geometric mean, not median.',
        'scope': 'Self user+system CPU including driver; process-attributed writes, not device writes; sampled memory maxima, not peaks. Complete self counters since exec are separate from initialization-to-cleanup deltas. All adjacent lifecycle phases and external-controller wall scopes remain separate.',
        'semantic_trust': 'Producer product/controller oracles and complete semantic comparator records are checked. This standalone reader independently reproduces counter arithmetic; it is not an independent implementation of all semantic request oracles.',
        'paired_rows': len(records), 'summary_rows': len(summaries)}, indent=2) + '\n')
    print(json.dumps({'paired_rows': len(records), 'summary_rows': len(summaries), 'output': str(a.output)}))

if __name__ == '__main__':
    main()
