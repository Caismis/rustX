#!/usr/bin/env python3
"""Rotate four common provider workloads across rustX, Pi, and the native DSH SDK.

DSH SDK idle is not a final persistence barrier. Never interpret its turn
latency or write counters as equal-durability speedups over the other engines.
The native SDK close/resume continuation is retained but not cross-ranked.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import statistics
import subprocess

from controller import TEST, run


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


dsh = load('deepseek_reference', 'deepseek-reference.py')
summarizer = load('runtime_summary', 'summarize-http.py')


def native_totals(tree, ticks):
    """Sum raw process counters, keeping waited children in CPU accounting."""
    self_cpu, waited_cpu, writes, pss, anon = 0, 0, 0, 0, 0
    for raw in tree.values():
        stat = raw['stat'].rsplit(')', 1)[1].split()
        self_cpu += sum(int(stat[index]) for index in (11, 12)) / ticks * 1000
        waited_cpu += sum(int(stat[index]) for index in (13, 14)) / ticks * 1000
        io = dict(line.split(':', 1) for line in raw['io'].splitlines())
        memory = dict(line.split(':', 1) for line in raw['smaps_rollup'].splitlines() if ':' in line)
        writes += int(io['write_bytes'])
        pss += int(memory['Pss'].split()[0])
        anon += int(memory['Pss_Anon'].split()[0])
    return {'cpu_ms': self_cpu + waited_cpu, 'extant_process_self_cpu_ms': self_cpu,
            'waited_child_cpu_ms': waited_cpu, 'write_bytes': writes, 'pss_mib': pss / 1024, 'anon_pss_mib': anon / 1024}


def tree_identity(tree):
    return {pid: raw['stat'].rsplit(')', 1)[1].split()[19] for pid, raw in tree.items()}


def dsh_summary(result, ticks):
    dsh.verify_result(result)
    measured = [sample for sample in result['samples'] if sample['measured']]
    # The same contiguous window as the existing benchmark: after the last
    # seed turn through the last measured turn. Observation/driver pauses are
    # included in CPU/write deltas but excluded from mean per-turn wall.
    start = result['samples'][result['history'] - 1]['after'] if result['history'] else measured[0]['before']
    end = measured[-1]['after']
    a, b = native_totals(start, ticks), native_totals(end, ticks)
    memory = [native_totals(sample['after'], ticks) for sample in measured]
    unavailable = {}
    def delta(key):
        if tree_identity(start) != tree_identity(end):
            unavailable[key] = 'extant PID/start-time set changed; exited-process counters cannot be reconstructed from snapshots'
            return None
        value = b[key] - a[key]
        if value < 0:
            unavailable[key] = 'process counter decreased across measurement window'
            return None
        return value
    cpu, writes = delta('cpu_ms'), delta('write_bytes')
    return {'mean_turn_ms': statistics.mean(sample['settled_ns'] / 1e6 for sample in measured),
            'turn_cpu_ms_per_turn': None if cpu is None else cpu / len(measured),
            'turn_write_mib_per_turn': None if writes is None else writes / len(measured) / 2**20,
            'raw_window_totals': {'start': a, 'end': b}, 'unavailable_metrics': unavailable,
            'max_sampled_turn_pss_mib': max(point['pss_mib'] for point in memory),
            'max_sampled_turn_anon_pss_mib': max(point['anon_pss_mib'] for point in memory),
            'completion_boundary': result['completion_boundary'],
            'first_cli_complete_cpu_ms': (result['first_exit']['wait4']['ru_utime'] + result['first_exit']['wait4']['ru_stime']) * 1000,
            'first_cli_close_ms': result['first_exit']['close_ns'] / 1e6,
            'first_cli_wait4_output_blocks': result['first_exit']['wait4']['ru_oublock'],
            'scope': 'direct SDK CLI plus extant-child snapshots and waited-child CPU; process-attributed I/O is not complete process-tree/device I/O; Python driver excluded; idle does not await final fsync; close separate'}


def existing_summary(path):
    result = summarizer.sample(path)
    data = json.loads(path.read_text())
    memory = [summarizer.counters(sample) for sample in data['samples'] if sample['measured']]
    return {key: result[key] for key in ('mean_turn_ms', 'turn_cpu_ms_per_turn', 'turn_write_mib_per_turn')} | {
        'max_sampled_turn_pss_mib': max(point['pss_kib'] for point in memory) / 1024,
        'max_sampled_turn_anon_pss_mib': max(point['anon_pss_kib'] for point in memory) / 1024,
        'completion_boundary': 'native durable settlement plus live subscriber completion',
        'scope': 'product and in-process benchmark driver; separate sibling provider excluded'}


def admit_case(rows, comparison_returncode):
    """Keep a rotation out of aggregate metrics if any product/oracle failed."""
    if comparison_returncode != 0 or len(rows) != 3:
        return []
    if {row['engine'] for row in rows} != {'rust', 'pi', 'deepseek'} or not all(row['accepted'] for row in rows):
        return []
    return list(rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rust', type=Path, required=True)
    parser.add_argument('--pi', type=Path, required=True)
    parser.add_argument('--deepseek', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repetitions', type=int, default=3)
    parser.add_argument('--case', action='append', choices=dsh.CASES)
    args = parser.parse_args()
    assert args.repetitions >= 1
    assert Path.cwd() == dsh.REPO, f'run from {dsh.REPO}'
    args.output.mkdir(parents=True, exist_ok=False)
    env = {key: value for key, value in os.environ.items()
           if not any(word in key.upper() for word in ('TOKEN', 'SECRET', 'CREDENTIAL', 'API_KEY'))
           and not key.startswith(('DSH_', 'DEEPSEEK_'))}
    env.setdefault('UV_CACHE_DIR', str(Path.home() / '.cache/uv'))
    env.update(DSH_TELEMETRY_DISABLED='1')
    manifest = {'clock_ticks': os.sysconf('SC_CLK_TCK'), 'repetitions': args.repetitions,
                'cases': args.case or dsh.CASES, 'order': 'rotate rust/pi/deepseek by repetition',
                'rust': {'path': str(args.rust.resolve()), 'sha256': dsh.sha256(args.rust)},
                'pi': {'path': str(args.pi.resolve()), 'revision': subprocess.check_output(['git', '-C', str(args.pi), 'rev-parse', 'HEAD'], text=True).strip()},
                'deepseek': dsh.identity(args.deepseek.resolve()), 'deepseek_profile': 'sdk',
                'harness': {'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
                            'files': {path.name: dsh.sha256(path) for path in Path(__file__).parent.glob('deepseek-*') if path.is_file()}},
                'durability_equivalence': False, 'request_equivalence': 'rust/Pi core oracle only; DSH native exact oracle plus declared extras',
                'cross_engine_lifecycle_ratios': 'intentionally absent: source CLI, native profile, and extra verification continuation differ'}
    assert manifest['pi']['revision'] == '90d863f7c1183b965f099855715d6e88c7b8a52c'
    subprocess.run(['git', '-C', str(args.pi), 'diff', '--exit-code', 'HEAD'], check=True, capture_output=True)
    (args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2))
    records, metrics, product_metrics = [], [], []
    for repetition in range(args.repetitions):
        order = ['rust', 'pi', 'deepseek']
        offset = repetition % 3
        order = order[offset:] + order[:offset]
        for case in args.case or dsh.CASES:
            case_metrics = []
            for engine in order:
                stem = (args.output / f'{repetition}-{case}-{engine}').resolve()
                if engine == 'deepseek':
                    result = dsh.run_case(args.deepseek.resolve(), case, stem,
                                          env | {'BENCHMARK_API_KEY': 'conformance-secret'}, 'sdk')
                    record = {'engine': engine, 'case': case, 'repetition': repetition, 'accepted': result['accepted'], 'error': result.get('error')}
                    if result['accepted']:
                        metric = dsh_summary(result, manifest['clock_ticks'])
                else:
                    child_env = env | {'RUSTX_BENCH_OBSERVER_MODE': 'live-subscriber', 'RUSTX_BENCH_CASE': case,
                                       'RUSTX_BENCH_OUTPUT': str(stem.with_suffix('.json'))}
                    argv = [str(args.rust.resolve()), TEST, '--exact', '--ignored', '--nocapture'] if engine == 'rust' else [
                        'node', '--conditions=source', 'benchmarks/runtime/pi-http.mjs', str(args.pi.resolve()), case, str(stem.with_suffix('.json'))]
                    raw = run(argv, stem, case, child_env, dsh.REPO / 'test-support/fake-provider')
                    record = {'engine': engine, 'case': case, 'repetition': repetition, 'accepted': raw['accepted'], 'error': raw.get('error')}
                    if raw['accepted']:
                        metric = existing_summary(stem.with_suffix('.json'))
                records.append(record)
                print(json.dumps(record), flush=True)
                if record['accepted']:
                    case_metrics.append(record | metric)
                    product_metrics.append(record | metric)
                (args.output / 'records.json').write_text(json.dumps(records, indent=2))
                (args.output / 'product-metrics.json').write_text(json.dumps(product_metrics, indent=2))
            comparison = subprocess.run(['node', 'benchmarks/runtime/compare-http.mjs',
                                         str(args.output / f'{repetition}-{case}-rust.json'),
                                         str(args.output / f'{repetition}-{case}-pi.json'),
                                         str(args.output / f'{repetition}-{case}-rust-pi-comparison.json')], capture_output=True, text=True)
            if comparison.returncode:
                records.append({'accepted': False, 'case': case, 'repetition': repetition, 'error': comparison.stderr})
                (args.output / f'{repetition}-{case}-comparison-error.log').write_text(comparison.stderr)
            metrics.extend(admit_case(case_metrics, comparison.returncode))
            (args.output / 'metrics.json').write_text(json.dumps(metrics, indent=2))
    (args.output / 'records.json').write_text(json.dumps(records, indent=2))
    groups = []
    for case in args.case or dsh.CASES:
        for engine in ('rust', 'pi', 'deepseek'):
            rows = [row for row in metrics if row['engine'] == engine and row['case'] == case]
            if rows:
                keys = ('mean_turn_ms', 'turn_cpu_ms_per_turn', 'turn_write_mib_per_turn',
                        'max_sampled_turn_pss_mib', 'max_sampled_turn_anon_pss_mib')
                values = {key: [row[key] for row in rows if row[key] is not None] for key in keys}
                groups.append({'engine': engine, 'case': case, 'samples': len(rows),
                               'available_samples': {key: len(values[key]) for key in keys},
                               'medians': {key: statistics.median(values[key]) if values[key] else None for key in keys},
                               'ranges': {key: [min(values[key]), max(values[key])] if values[key] else None for key in keys},
                               'completion_boundary': rows[0]['completion_boundary'], 'scope': rows[0]['scope']})
    (args.output / 'summary.json').write_text(json.dumps({'durability_equivalence': False, 'groups': groups}, indent=2))
    raise SystemExit(int(any(not row['accepted'] for row in records)))


if __name__ == '__main__':
    main()
