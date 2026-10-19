#!/usr/bin/env python3
"""Verify retained cancellation pairs directly from raw process counters."""
import argparse
import csv
import hashlib
import io
import json
import math
from pathlib import Path
import statistics
import tarfile

PREFIX = 'observer-demand-v3-cancellation'
METRICS = ['mean_turn_ms', 'turn_cpu_ms_per_turn',
           'through_cleanup_self_cpu_ms', 'through_cleanup_process_write_mib',
           'max_sampled_anon_pss_mib']


def recompute(archive):
    with tarfile.open(archive, 'r:xz') as tar:
        manifest = json.load(tar.extractfile('MANIFEST.json'))
        for name, entry in manifest.items():
            data = tar.extractfile(name).read()
            assert len(data) == entry['bytes']
            assert hashlib.sha256(data).hexdigest() == entry['sha256'], name

        def read(name):
            return json.load(tar.extractfile(f'{PREFIX}/{name}'))

        def cpu(point):
            fields = point['process']['stat'].rsplit(')', 1)[1].split()
            return (int(fields[11]) + int(fields[12])) * 10

        def metrics(data):
            life = {row['phase']: row for row in data['lifecycle']}
            points = [*data['lifecycle'], *data['samples']]
            assert data['history'] == 0 and data['measured'] == 1
            assert data['oracle'] == 'passed' and data['model_time'] == '2026-10-19 00:00:00'
            assert data['workload_version'] == 3
            final = life['after_cleanup']
            writes = dict(row.split(':', 1) for row in final['process']['io'].splitlines())
            memories = [dict(row.split(':', 1) for row in p['process']['smaps_rollup'].splitlines() if ':' in row) for p in points]
            return dict(zip(METRICS, [
                statistics.mean(p['settled_us'] / 1000 for p in data['samples'] if p['measured']),
                cpu(data['samples'][-1]) - cpu(life['after_initialization']),
                cpu(final), int(writes['write_bytes']) / 2**20,
                max(int(m['Pss_Anon'].split()[0]) for m in memories) / 1024,
            ]))

        rows, groups = [], []
        for mode in ['no-live-subscriber', 'live-subscriber']:
            assert read(f'{mode}/manifest.json')['clock_ticks'] == 100
            values = {metric: [] for metric in METRICS}
            for pair in range(3):
                costs = {}
                for side in ['before', 'after']:
                    stem = f'{mode}/{pair}-cancel-{side}'
                    process = read(stem + '.process.json')
                    assert process['accepted'] and process['product_exit'] == 0 and process['provider_exit'] == 0
                    costs[side] = metrics(read(stem + '.json'))
                comparison = read(f'{mode}/{pair}-cancel-comparison.json')
                assert len(comparison['checked']) == 1
                assert all(p['coreSemanticEqual'] and p['fullSemanticRequestEqual'] for p in comparison['checked'])
                for metric in METRICS:
                    before, after = costs['before'][metric], costs['after'][metric]
                    assert before > 0 and after > 0
                    ratio = after / before
                    rows.append(dict(mode=mode, pair=pair, metric=metric,
                                     before=before, after=after, ratio=ratio))
                    values[metric].append(ratio)
            for metric, ratios in values.items():
                logs = list(map(math.log, ratios))
                mean = statistics.mean(logs)
                width = 2.919985580355516 * statistics.stdev(logs) / math.sqrt(3)
                groups.append(dict(mode=mode, metric=metric, ratios=ratios,
                                   median=statistics.median(ratios),
                                   geometric_mean=math.exp(mean), lower90=math.exp(mean-width),
                                   upper90_one_sided95=math.exp(mean+width)))
        return manifest, rows, groups


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--archive', type=Path, required=True)
    p.add_argument('--readable', type=Path, required=True)
    p.add_argument('--write', action='store_true')
    args = p.parse_args()
    manifest, rows, groups = recompute(args.archive)
    csv_text = io.StringIO()
    writer = csv.DictWriter(csv_text, fieldnames=list(rows[0]), lineterminator='\n')
    writer.writeheader()
    writer.writerows(rows)
    results = {'paired-costs.csv': csv_text.getvalue(),
               'paired-uncertainty.json': json.dumps(groups, indent=2) + '\n'}
    for name, value in results.items():
        path = args.readable / name
        if args.write:
            path.write_text(value)
        else:
            assert path.read_text() == value, name
    print(json.dumps(dict(verified=True, archive_files=len(manifest),
                          accepted_samples=12, paired_rows=len(rows), intervals=len(groups))))


if __name__ == '__main__':
    main()
