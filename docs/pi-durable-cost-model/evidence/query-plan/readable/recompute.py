#!/usr/bin/env python3
"""Recompute every paired ratio/median and the reported runtime intervals.

No third-party dependencies. Optional --raw-root verifies archived raw inputs
and rederives the input metrics with the repository's original summarizer.
"""
import argparse
import csv
import hashlib
import importlib.util
import json
import math
from pathlib import Path
import statistics
import sys

HERE = Path(__file__).resolve().parent
STAGES = ('runtime', 'append-warm', 'append-cold')
T_DF2 = 2.919985580355516  # Two-sided 90%, equivalently one-sided 95% upper.


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--raw-root', type=Path,
                        help='directory containing extracted pi-parity-qpsg/')
    args = parser.parse_args()
    provenance = json.loads((HERE / 'provenance.json').read_text())
    for name, proof in provenance['readable_inputs'].items():
        assert sha(HERE / name) == proof['sha256'], name
    all_results = {}
    csv_rows = []
    sample_count = 0
    ratio_count = 0
    for stage in STAGES:
        data = json.loads((HERE / (stage + '-summary.json')).read_text())
        medians = {}
        for case, pairs in data['samples'].items():
            assert sorted(p['pair'] for p in pairs) == [0, 1, 2]
            sample_count += 2 * len(pairs)
            reconstructed = []
            for pair in pairs:
                before, after = pair['before'], pair['after']
                ratios = {key: after[key] / value for key, value in before.items()
                          if isinstance(value, (int, float)) and value != 0
                          and isinstance(after.get(key), (int, float))}
                assert ratios == pair['after_over_before'], (stage, case, pair['pair'])
                ratio_count += len(ratios)
                reconstructed.append({'before': before, 'after': after,
                                      'after_over_before': ratios})
                for metric in sorted(set(before) | set(after)):
                    a, b = before.get(metric), after.get(metric)
                    if all(v is None or isinstance(v, (int, float)) for v in (a, b)):
                        csv_rows.append([stage, case, pair['pair'], metric, a, b,
                                         ratios.get(metric)])
            medians[case] = {}
            for side in ('before', 'after', 'after_over_before'):
                values = {}
                for metric in sorted(set().union(*(r[side] for r in reconstructed))):
                    nums = [r[side].get(metric) for r in reconstructed]
                    if all(n is None or isinstance(n, (int, float)) for n in nums):
                        values[metric] = (statistics.median(nums)
                                          if all(n is not None for n in nums) else None)
                medians[case][side] = values
            assert medians[case] == data['medians'][case], (stage, case)
        all_results[stage] = medians
    assert sample_count == 84
    runtime = json.loads((HERE / 'runtime-summary.json').read_text())
    uncertainty = {}
    for case, pairs in runtime['samples'].items():
        uncertainty[case] = {}
        for metric in ('mean_turn_ms', 'turn_cpu_ms_per_turn'):
            ratios = [p['after'][metric] / p['before'][metric] for p in pairs]
            logs = list(map(math.log, ratios))
            mean = statistics.mean(logs)
            width = T_DF2 * statistics.stdev(logs) / math.sqrt(len(logs))
            uncertainty[case][metric] = {
                'paired_ratios': ratios, 'geometric_mean': math.exp(mean),
                'lower_90': math.exp(mean - width), 'upper_90': math.exp(mean + width),
                'n': len(logs)}
    assert uncertainty == json.loads((HERE / 'runtime-uncertainty.json').read_text())
    if args.raw_root:
        for name, proof in provenance['raw_samples'].items():
            assert sha(args.raw_root / name) == proof['sha256'], name
        repo = HERE.parents[4]
        sys.path.insert(0, str(repo / 'benchmarks/runtime'))
        spec = importlib.util.spec_from_file_location(
            'original_summary', repo / 'benchmarks/runtime/summarize-comparisons.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        for stage in STAGES:
            regenerated = module.summarize(
                [args.raw_root / 'pi-parity-qpsg' / stage], append=stage != 'runtime')
            expected = json.loads((HERE / (stage + '-summary.json')).read_text())
            # Absolute artifact paths are provenance, not measurement inputs.
            for document in (regenerated, expected):
                for pairs in document['samples'].values():
                    for pair in pairs:
                        for side in ('before', 'after'):
                            pair[side].pop('file', None)
            assert regenerated == expected, stage
    with (HERE / 'paired-inputs.csv').open('w', newline='') as output:
        writer = csv.writer(output, lineterminator='\n')
        writer.writerow(['stage', 'case_or_history', 'pair', 'metric', 'before', 'after',
                         'after_over_before'])
        writer.writerows(csv_rows)
    (HERE / 'recomputed.json').write_text(json.dumps({
        'samples': sample_count, 'checked_ratios': ratio_count,
        'medians': all_results, 'runtime_intervals': uncertainty}, indent=2) + '\n')
    print(json.dumps({'samples': sample_count, 'paired_ratios_checked': ratio_count,
                      'all_medians_match': True, 'all_12_intervals_match': True,
                      'raw_inputs_verified': args.raw_root is not None}))


if __name__ == '__main__':
    main()
